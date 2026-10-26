//! Transfer job manager (Stage C).
//!
//! Jobs move whole manifests with partial files, conflict choices,
//! cancellation and restartable progress. Each side polls its own
//! records; senders learn completion by asking the receiver over the
//! auxiliary stream, so both directions complete honestly. File bytes
//! move over `vnilink/files/1` (`crate::transfer`). Records persist in
//! journals in the transfer directory, beside the partials of arriving
//! files.
//!
//! Threading follows the core rule: call from outside the runtime,
//! preferably off the UI thread. One `job_poll` advances at most one
//! file; progress is exact between polls. Records persist in per-job
//! journals (`job_journal`).
// trace: records every ignored failure

use super::*;

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::fs;
use crate::job::{self, ConflictChoice, JobDirection, JobSnapshot, JobState};
use crate::manifest::{self, Manifest, ManifestEntry};
use crate::session::Grant;

const STAGING_DIR: &str = "staging";
/// Under a job's staging: the partials of its arriving files, and beside
/// them the segment journal of each.
const STAGED_FILES: &str = "files";
const STAGED_SEGMENTS: &str = "segments";

/// Test hooks for cross-volume pre-staging. Production leaves them unset:
/// the copy runs when staged and destination volumes differ, and no probe
/// observes it. Tests scope the hooks to one job id, force the
/// cross-volume path on a single volume, and park a barrier in the copy
/// (`probe`), the parent-mkdir phase (`mkdir_probe`) or the final stage
/// sync (`sync_probe`) to prove global locks stay free.
#[cfg(unix)]
pub struct PrestageHooks {
    /// The engine the job belongs to ([`Core::prestage_key`]): job ids are
    /// per engine, and tests run several engines in one process.
    pub engine: usize,
    pub job_id: Option<u64>,
    pub force_cross_volume: bool,
    pub probe: Option<fn(u64, u64)>,
    pub mkdir_probe: Option<fn(u64)>,
    pub sync_probe: Option<fn(u64)>,
}

#[cfg(unix)]
pub(crate) static PRESTAGE_HOOKS: std::sync::Mutex<PrestageHooks> =
    std::sync::Mutex::new(PrestageHooks {
        engine: 0,
        job_id: None,
        force_cross_volume: false,
        probe: None,
        mkdir_probe: None,
        sync_probe: None,
    });

const NO_JOB: &str = "the job is unknown";
const SOURCE_CHANGED: &str = "the source changed since preparation";
const VERIFY_FAILED: &str = "the destination could not be verified";
const PUBLISH_FAILED: &str = "the file could not be published";
const STAGE_FAILED: &str = "the file could not be staged";

// Why a job stopped short of finishing, shown with the job until it
// resumes.
const UNREACHABLE: &str = "the other computer could not be reached";
const CONNECTION_ENDED: &str = "the connection to the other computer ended";
const STALLED: &str = "the transfer stalled";
const SENDER_BUSY: &str = "the other computer is busy sending other files";
const SENDER_STOPPED: &str = "the other computer stopped sending";
const SENDER_SHUT_DOWN: &str = "the other computer shut down";
const PERMISSION_OFF: &str = "file transfer is no longer allowed";
const UNPAIRED: &str = "the computers are no longer paired";
pub(super) const OWNER_GONE: &str = "the Files window that started it closed";
const STOPPED: &str = "the transfer stopped before the file was written";
const VOLUME_CHANGED: &str = "the destination's disk changed";
const APP_CLOSED: &str = "VniLink closed during the transfer";

/// Durable job record. Counters and choices survive restart; a partial
/// file in the transfer directory keeps the bytes of the file arriving.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct JobRecord {
    #[serde(default)]
    pub(super) owner: Option<super::service_jobs::JobOwner>,
    /// Set by the submitting side only.
    #[serde(default)]
    destination: Option<Landing>,
    pub(super) id: u64,
    /// Peer protocol job id; zero migrates older records whose ids matched.
    #[serde(default)]
    wire_id: u64,
    #[serde(default = "retention_now")]
    created_at: u64,
    pub(super) peer_key: [u8; 32],
    peer_id: String,
    direction: JobDirection,
    pub(super) sender: bool,
    pub(super) state: JobState,
    manifest: Option<Manifest>,
    local_root: u64,
    /// The leg's folder under `local_root`: where a receiver lands, or the
    /// folder a sender's `names` are entries of.
    local_rel: String,
    /// Sender only: the entries of `local_rel` it sends, by name; none when
    /// it sends the whole root.
    #[serde(default)]
    names: Vec<String>,
    /// Pinned source/target root for this leg. None migrates records
    /// saved before pinning: the first use adopts the current root.
    #[serde(default)]
    local_pin: Option<fs::PinnedRoot>,
    /// Sender only: each file's id on its volume when it was scanned (the
    /// inode, or the Windows file index), in manifest file order; zero when
    /// unknown. A source with another id was replaced since.
    #[serde(default)]
    source_ids: Vec<u64>,
    /// True when status queries ride the outgoing session (controller
    /// senders); false for host senders polling over the incoming one.
    outgoing: bool,
    file_index: usize,
    done_bytes: u64,
    files_done: u64,
    resumed_files: u64,
    skipped_files: u64,
    pub(super) waiting_file: Option<usize>,
    skipped_symlinks: u64,
    landed: HashMap<usize, String>,
    choices: HashMap<usize, ConflictChoice>,
    apply_remaining: Option<ConflictChoice>,
    pub(super) error: Option<String>,
    /// Verified bytes of the file arriving now; not saved, since a restart
    /// resumes that file from the store's own record.
    #[serde(skip)]
    in_flight: u64,
    /// Which run of the job this is. It advances when a stopped job runs
    /// again, so what a stopped run's fetch finds as it winds down is not
    /// taken for the new run's. Not saved: a restart starts a run.
    #[serde(skip)]
    pub(super) attempt: u64,
    /// Files landed since the journal last heard, oldest first.
    #[serde(skip)]
    unsaved_landed: Vec<(usize, String)>,
    /// A failure that may clear on its own, retried with backoff; none
    /// while steps succeed. Not saved: a restart resumes explicitly.
    #[serde(skip)]
    retry: Option<Retry>,
    /// Sender only: the file it sends now and how far into it the stream
    /// has taken bytes, so its progress moves without waiting on its
    /// receiver's replies.
    #[serde(skip)]
    sent_at: Option<(usize, u64)>,
}

impl JobRecord {
    /// The only way a job changes state, so that every change is recorded:
    /// which job, which side, from and to. Never a path or a peer's message.
    pub(super) fn enter(&mut self, state: JobState) {
        let stopped =
            |state: JobState| matches!(state, JobState::Cancelled | JobState::Interrupted);
        if stopped(self.state) && !stopped(state) && !state.is_terminal() {
            self.attempt += 1;
        }
        if self.state != state {
            let (from, to) = (self.state.name(), state.name());
            if state.is_terminal() {
                tracing::info!(target: crate::trace::FILES, job = self.id, sender = self.sender, from, to, files_done = self.files_done, done_bytes = self.done_bytes, "job state");
            } else {
                tracing::debug!(target: crate::trace::FILES, job = self.id, sender = self.sender, from, to, "job state");
            }
        }
        self.state = state;
    }
    /// A download of one `size`-byte file, as a receiver holds it.
    #[cfg(test)]
    pub(super) fn test_receiver(id: u64, size: u64) -> Self {
        Self {
            owner: None,
            destination: None,
            id,
            wire_id: id,
            created_at: retention_now(),
            peer_key: [0; 32],
            peer_id: String::new(),
            direction: JobDirection::Download,
            sender: false,
            state: JobState::Transferring,
            manifest: Some(Manifest {
                entries: vec![ManifestEntry {
                    path: "file.bin".to_owned(),
                    is_dir: false,
                    size,
                    source_mtime_ms: 0,
                }],
            }),
            local_root: 0,
            local_rel: String::new(),
            names: Vec::new(),
            local_pin: None,
            outgoing: false,
            skipped_symlinks: 0,
            file_index: 0,
            done_bytes: 0,
            files_done: 0,
            resumed_files: 0,
            skipped_files: 0,
            waiting_file: None,
            landed: HashMap::new(),
            choices: HashMap::new(),
            apply_remaining: None,
            error: None,
            in_flight: 0,
            attempt: 0,
            unsaved_landed: Vec::new(),
            retry: None,
            sent_at: None,
            source_ids: Vec::new(),
        }
    }

    /// A download of `files` files, as a receiver holds it.
    #[cfg(test)]
    pub(super) fn test_receiver_of(id: u64, files: usize) -> Self {
        let mut record = Self::test_receiver(id, 0);
        record.manifest = Some(Manifest {
            entries: (0..files)
                .map(|index| ManifestEntry {
                    path: format!("file-{index:05}.bin"),
                    is_dir: false,
                    size: 1,
                    source_mtime_ms: 0,
                })
                .collect(),
        });
        record
    }

    /// Lands the next file, as a receiver does.
    #[cfg(test)]
    pub(super) fn test_land_next(&mut self) {
        let index = self.file_index;
        self.land(index, format!("file-{index:05}.bin"));
        self.file_index += 1;
        self.files_done += 1;
        self.done_bytes += 1;
    }

    #[cfg(test)]
    pub(super) fn test_landed(&self) -> &HashMap<usize, String> {
        &self.landed
    }

    /// Whether the driver has a step to take for this job: neither ended
    /// nor interrupted, not a receiver waiting on a decision, and not a
    /// sender being prepared, which its request owns.
    fn drivable(&self) -> bool {
        !self.state.is_terminal()
            && self.state != JobState::Interrupted
            && (self.sender || self.state != JobState::WaitingDecision)
            && !(self.sender && self.state == JobState::Preparing)
    }

    /// Whether its steps come on a timer rather than at every change: a
    /// sender only asks its receiver how far it got, and a receiver whose
    /// sender is still preparing asks for its manifest.
    fn paced(&self) -> bool {
        self.sender || self.manifest.is_none()
    }

    /// Whether this is a sender still being prepared here: it has nothing
    /// to send yet, so nothing to resume.
    pub(super) fn preparing_here(&self) -> bool {
        self.sender && self.manifest.is_none()
    }

    /// What a receiver's answer may change on its sender.
    fn reported(&self) -> (JobState, [u64; 3], Option<String>, Option<usize>) {
        (
            self.state,
            [self.done_bytes, self.files_done, self.skipped_files],
            self.error.clone(),
            self.waiting_file,
        )
    }

    pub(super) fn has_manifest(&self) -> bool {
        self.manifest.is_some()
    }

    /// Records that file `index` landed as `name`, for the journal too.
    fn land(&mut self, index: usize, name: String) {
        self.unsaved_landed.push((index, name.clone()));
        self.landed.insert(index, name);
    }

    /// Replays a landed file from the journal.
    pub(super) fn land_saved(&mut self, index: usize, name: String) {
        self.landed.insert(index, name);
    }

    pub(super) fn unsaved_landed(&self) -> &[(usize, String)] {
        &self.unsaved_landed
    }

    /// The journal now holds the oldest `count` unsaved landed files.
    pub(super) fn saved_landed(&mut self, count: usize) {
        self.unsaved_landed
            .drain(..count.min(self.unsaved_landed.len()));
    }

    /// The record as a journal progress line: everything but its manifest
    /// and landed files, which the journal holds apart.
    pub(super) fn progress_line(&mut self) -> String {
        let manifest = self.manifest.take();
        let landed = std::mem::take(&mut self.landed);
        let line = serde_json::to_string(&*self).unwrap_or_default();
        self.manifest = manifest;
        self.landed = landed;
        line
    }

    /// Takes a progress line read from the journal, keeping this record's
    /// manifest and landed files.
    pub(super) fn take_progress(&mut self, progress: JobRecord) {
        let manifest = self.manifest.take();
        let landed = std::mem::take(&mut self.landed);
        *self = progress;
        self.manifest = manifest;
        self.landed = landed;
    }

    /// Sender only: bytes its streams have taken, counting the files before
    /// the one it sends now as whole. Zero once files stop moving.
    fn sent(&self) -> u64 {
        let Some((file, position)) = self.sent_at else {
            return 0;
        };
        if !self.sender || !matches!(self.state, JobState::Serving | JobState::Transferring) {
            return 0;
        }
        let before: u64 = self.files().iter().take(file).map(|entry| entry.size).sum();
        before.saturating_add(position)
    }

    /// The path of the file moving now, while files move: the receiver's
    /// own place, or for a sender the file after those its receiver has
    /// landed or skipped.
    fn current_file(&self) -> Option<String> {
        if !matches!(self.state, JobState::Serving | JobState::Transferring) {
            return None;
        }
        let index = if self.sender {
            (self.files_done + self.skipped_files) as usize
        } else {
            self.file_index
        };
        self.files().get(index).map(|entry| entry.path.clone())
    }

    /// Bytes done, including the file arriving now.
    fn done(&self) -> u64 {
        self.done_bytes.saturating_add(self.in_flight)
    }

    fn files(&self) -> Vec<&ManifestEntry> {
        self.manifest
            .as_ref()
            .map(|manifest| {
                manifest
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(super) fn snapshot(&self) -> JobSnapshot {
        let (total_bytes, files_total, label) = self
            .manifest
            .as_ref()
            .map(|manifest| {
                // A snapshot is taken at every step: no copy of the paths.
                let mut files = manifest.entries.iter().filter(|entry| !entry.is_dir);
                let label = match (files.next(), files.count()) {
                    (None, _) => "Transfer".to_owned(),
                    (Some(first), 0) => first.path.clone(),
                    (Some(first), rest) => format!("{} +{rest} more", first.path),
                };
                (manifest.total_bytes(), manifest.file_count() as u64, label)
            })
            .unwrap_or((0, 0, "Transfer".to_owned()));
        JobSnapshot {
            id: self.id,
            peer: self.peer_id.clone(),
            label,
            direction: self.direction,
            sender: self.sender,
            state: self.state,
            total_bytes,
            done_bytes: self
                .done()
                .max(self.sent())
                .min(total_bytes.max(self.done_bytes)),
            files_total,
            files_done: self.files_done,
            resumed_files: self.resumed_files,
            skipped_files: self.skipped_files,
            skipped_symlinks: self.skipped_symlinks,
            waiting_file: self.waiting_file.and_then(|index| {
                self.manifest
                    .as_ref()?
                    .entries
                    .iter()
                    .filter(|entry| !entry.is_dir)
                    .nth(index)
                    .map(|entry| entry.path.clone())
            }),
            current_file: self.current_file(),
            error: self.error.clone(),
            destination: self.destination.as_ref().map(|landing| {
                // A chosen folder is what the manifest's paths lie under.
                let folders: std::collections::HashSet<&str> = self
                    .manifest
                    .iter()
                    .flat_map(|manifest| &manifest.entries)
                    .filter_map(|entry| entry.path.split_once('/'))
                    .map(|(top, _)| top)
                    .collect();
                crate::job::JobDestination {
                    root: landing.root,
                    folder: landing.folder.clone(),
                    items: landing
                        .names
                        .iter()
                        .map(|name| crate::job::JobItem {
                            name: name.clone(),
                            folder: folders.contains(name.as_str()),
                        })
                        .collect(),
                }
            }),
        }
    }
}

/// Small files of one receiving job fetched ahead of their turn, by their
/// place among the job's files, with their sizes. They only fill their
/// partials: publication still takes each in order, and uses one only when
/// it arrived whole.
#[derive(Default)]
pub(super) struct Prefetch {
    /// Stops every fetch ahead when the job stops.
    stop: Option<tokio::sync::watch::Sender<bool>>,
    files: HashMap<usize, (u64, tokio::task::JoinHandle<Fetched>)>,
}

/// How a file fetched ahead ended, and the connection it arrived on.
type Fetched = (Result<crate::transfer::Received, Error>, Option<usize>);

/// Files a job fetches ahead of the one it takes now, and their bytes at
/// most. With the file taken now, a job stays within a sender's serving
/// bound for one peer.
const PREFETCH_FILES: usize = 7;
const PREFETCH_BYTES: u64 = 64 * 1024 * 1024;

/// Stops a job's fetches ahead and forgets them. Their partials stay, and
/// what they hold is checked again whenever it is used.
pub(super) fn stop_prefetch(shared: &Shared, id: u64) {
    if let Some(prefetch) = lock(&shared.job_prefetch).remove(&id) {
        if let Some(stop) = prefetch.stop {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
    }
}

/// Retries of a job whose last step failed in a way that may clear on its
/// own: a lost or reconnecting connection, a stalled stream, a sender that
/// stopped serving for now.
#[derive(Debug, Clone)]
struct Retry {
    /// Retries since the job last moved a byte.
    attempts: u32,
    first_at: std::time::Instant,
    next_at: std::time::Instant,
    /// The job's bytes when the first of these retries was scheduled.
    done_at: u64,
}

/// Retries before a job is left interrupted, and the longest it keeps
/// retrying without moving a byte.
const MAX_RETRIES: u32 = 6;
const RETRY_WINDOW: Duration = Duration::from_secs(120);

/// The pause before retry `attempt` (from one): 1, 2, 4, 8, 16, then 30 s,
/// each varied by up to a fifth so jobs that failed together spread out.
fn retry_pause(attempt: u32) -> Duration {
    let base = match attempt {
        0 | 1 => 1.0,
        2 => 2.0,
        3 => 4.0,
        4 => 8.0,
        5 => 16.0,
        _ => 30.0,
    };
    let spread = 0.8 + 0.4 * (rand::random::<u32>() as f64 / u32::MAX as f64);
    Duration::from_secs_f64(base * spread)
}

/// Where a transfer lands, as the side that submitted it keeps it: the
/// folder on the receiving computer and the names arriving in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Landing {
    root: u64,
    folder: String,
    names: Vec<String>,
}

/// `name` under `folder`, as a root-relative path.
fn join_rel(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

/// The root-relative paths a sender scans: each chosen entry of `folder`,
/// or with no names the folder itself, which is then the whole root.
fn source_rels(folder: &str, names: &[String]) -> Vec<String> {
    if names.is_empty() {
        vec![folder.to_owned()]
    } else {
        names.iter().map(|name| join_rel(folder, name)).collect()
    }
}

/// One root-relative source as its folder and its name; the whole root
/// (empty) names nothing.
fn split_source(rel: &str) -> (String, Vec<String>) {
    match rel.rsplit_once('/') {
        _ if rel.is_empty() => (String::new(), Vec::new()),
        Some((folder, name)) => (folder.to_owned(), vec![name.to_owned()]),
        None => (String::new(), vec![rel.to_owned()]),
    }
}

/// What a sender may be asked to send: entries of one folder, each named
/// once, or the whole root. A manifest holds no more entries than names.
fn check_sources(folder: &str, names: &[String]) -> Result<(), Error> {
    fs::check_rel(folder)?;
    if names.is_empty() && !folder.is_empty() {
        return Err(Error::InvalidSetting("nothing was chosen to copy"));
    }
    if names.len() > manifest::MAX_MANIFEST_FILES {
        return Err(Error::InvalidSetting(manifest::MANIFEST_LIMIT));
    }
    let mut seen = std::collections::HashSet::with_capacity(names.len());
    for name in names {
        fs::check_name(name)?;
        if !seen.insert(name.as_str()) {
            return Err(Error::InvalidSetting("an item was chosen twice"));
        }
    }
    Ok(())
}

/// Verify a job leg's pin against locked roots, adopting the current
/// root for legacy records. A rebound entry fails the leg: callers must
/// serve nothing on error. Runs under the roots lock together with the
/// resolution it guards, so a rebind between check and use is impossible.
fn verify_leg_pin(
    roots: &fs::RootSet,
    id: u64,
    pin: &mut Option<fs::PinnedRoot>,
) -> Result<(), Error> {
    if pin.is_none() {
        *pin = Some(roots.pin(id)?);
    }
    roots.verify_pin(pin.as_ref().expect("leg pin just adopted"))
}

impl Core {
    /// Uploads a local tree to the paired peer: scans, hashes, offers and
    /// sends the manifest. The returned id is the sender record; the peer
    /// adopts the same id for its receiver record.
    pub fn job_submit_upload(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let (folder, names) = split_source(src_rel);
        self.submit_upload(
            peer_endpoint_id,
            src_root,
            &folder,
            &names,
            dst_root,
            dst_rel,
            None,
        )
    }

    /// Uploads the entries `names` of `src_folder`, or with no names the
    /// whole root, as one job.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn submit_upload(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        check_sources(src_folder, names)?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        // The live session first: no scanning happens without one.
        // The host re-fences the generation on arrival.
        let generation = self.generation_for(&peer, true, owner)?;
        super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
        // Snapshot under a short lock; the recursive walk runs off every lock.
        let snapshot = lock(&self.shared.file_roots).clone();
        // Pin the leg to the scanned generation: each part re-verifies it,
        // so a rebind during admission fails the whole offer instead of
        // mixing generations.
        let mut pin = Some(snapshot.pin(src_root)?);
        let mut manifest = Manifest {
            entries: Vec::new(),
        };
        let mut skipped_symlinks = 0;
        let mut scanned = HashMap::new();
        for rel in source_rels(src_folder, names) {
            let scan = manifest::scan(&snapshot, src_root, &rel)?;
            skipped_symlinks += scan.skipped_symlinks;
            scanned.extend(
                scan.files
                    .iter()
                    .map(|file| (file.root_rel.clone(), file.ino)),
            );
            let mut part = manifest::skeleton(&scan);
            super::service_jobs::check_owner(&lock(&self.shared.state), owner)?;
            {
                // A directory source keeps its top folder; files stay flat.
                let roots = lock(&self.shared.file_roots);
                verify_leg_pin(&roots, src_root, &mut pin)?;
                if roots.resolve_dir(src_root, &rel).is_ok() {
                    manifest::prefix_source_dir(&rel, &mut part);
                }
            }
            manifest.entries.append(&mut part.entries);
            if manifest.entries.len() > manifest::MAX_MANIFEST_FILES {
                return Err(Error::InvalidSetting(manifest::MANIFEST_LIMIT));
            }
        }
        manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
        manifest::validate(&manifest)?;
        let source_ids = source_ids(&manifest, src_folder, &scanned);
        let id = allocate_job_id(&self.shared)?;
        let record = JobRecord {
            owner,
            destination: Some(Landing {
                root: dst_root,
                folder: dst_rel.to_owned(),
                names: names.to_vec(),
            }),
            id,
            wire_id: id,
            created_at: retention_now(),
            peer_key: peer,
            peer_id: peer_endpoint_id.to_owned(),
            direction: JobDirection::Upload,
            sender: true,
            state: JobState::Preparing,
            manifest: Some(manifest.clone()),
            local_root: src_root,
            local_rel: src_folder.to_owned(),
            names: names.to_vec(),
            local_pin: pin,
            outgoing: true,
            skipped_symlinks,
            file_index: 0,
            done_bytes: 0,
            files_done: 0,
            resumed_files: 0,
            skipped_files: 0,
            waiting_file: None,
            landed: HashMap::new(),
            choices: HashMap::new(),
            apply_remaining: None,
            error: None,
            in_flight: 0,
            attempt: 0,
            unsaved_landed: Vec::new(),
            retry: None,
            sent_at: None,
            source_ids,
        };
        // Announce over the auxiliary stream on the live control session.
        let offer = job::UploadOffer {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            sender_job: id,
            manifest,
            dst_root,
            dst_rel: dst_rel.to_owned(),
        };
        job::check_version(offer.v)?;
        // Serve before announcing: the receiver may ask for the first file
        // as soon as it accepts, before its reply reaches this side.
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, owner)?;
            let mut record = record;
            // Re-verify before announcing: a rebind between the last file
            // and the insert fails here instead of offering moved bytes.
            verify_leg_pin(
                &lock(&self.shared.file_roots),
                record.local_root,
                &mut record.local_pin,
            )?;
            record.enter(JobState::Serving);
            jobs.insert(id, record);
        }
        let announced = (|| {
            let reply_bytes = self.service_job_round_trip(
                owner,
                &peer,
                job::TAG_JOB_UPLOAD,
                &job::encode(&offer)?,
            )?;
            let reply: job::JobReply = job::decode(&reply_bytes)?;
            if reply.id != offer.id {
                return Err(Error::Malformed);
            }
            match reply.status {
                status if status == job::STATUS_OK => Ok(()),
                status => Err(job::error_for_status(status)),
            }
        })();
        if let Err(error) = announced {
            // An offer the receiver did not take leaves nothing to serve.
            lock(&self.shared.jobs).remove(&id);
            return Err(error);
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Asks the paired peer to prepare a download from its roots. The host
    /// scans, hashes and offers; this side adopts the host's job id for
    /// its receiver record.
    pub fn job_submit_download(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_rel: &str,
        dst_root: u64,
        dst_rel: &str,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let (folder, names) = split_source(src_rel);
        self.submit_download(
            peer_endpoint_id,
            src_root,
            &folder,
            &names,
            dst_root,
            dst_rel,
            None,
        )
    }

    /// Downloads the peer's entries `names` of `src_folder`, or with no
    /// names its whole root, as one job.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn submit_download(
        &self,
        peer_endpoint_id: &str,
        src_root: u64,
        src_folder: &str,
        names: &[String],
        dst_root: u64,
        dst_rel: &str,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let _engine = self.engine();
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        self.ensure_alive()?;
        check_sources(src_folder, names)?;
        Self::check_job_capacity(&self.shared)?;
        {
            let state = lock(&self.shared.state);
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        fs::check_rel(dst_rel)?;
        // Pin the destination leg at admission: every receive below
        // re-verifies it, so a target rebind reroutes no bytes.
        let pin = {
            let roots = lock(&self.shared.file_roots);
            if roots.snapshot().iter().all(|(id, _)| *id != dst_root) {
                return Err(Error::InvalidSetting("the destination root is unknown"));
            }
            Some(roots.pin(dst_root)?)
        };
        // The live session first, as in uploads.
        let generation = self.generation_for(&peer, true, owner)?;
        let request = job::DownloadRequest {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            src_root,
            src_folder: src_folder.to_owned(),
            names: names.to_vec(),
        };
        let reply_bytes = self.service_job_round_trip(
            owner,
            &peer,
            job::TAG_JOB_DOWNLOAD,
            &job::encode(&request)?,
        )?;
        let reply: job::ManifestReply = job::decode(&reply_bytes)?;
        if reply.id != request.id {
            return Err(Error::Malformed);
        }
        match reply.status {
            status if status == job::STATUS_OK => {}
            status => return Err(job::error_for_status(status)),
        }
        // A preparation still under way leaves the job Preparing; its
        // manifest is asked for until ready (`poll_receiver`).
        if let Some(manifest) = &reply.manifest {
            manifest::validate(manifest)?;
        }
        let id;
        {
            let mut jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, owner)?;
            if jobs
                .values()
                .any(|record| record.peer_key == peer && wire_job_id(record) == reply.job_id)
            {
                return Err(Error::InvalidSetting("the job id is already in use"));
            }
            id = allocate_job_id(&self.shared)?;
            self.shared
                .job_next
                .fetch_max(reply.job_id.saturating_add(1), Ordering::Relaxed);
            jobs.insert(
                id,
                JobRecord {
                    owner,
                    destination: Some(Landing {
                        root: dst_root,
                        folder: dst_rel.to_owned(),
                        names: names.to_vec(),
                    }),
                    id,
                    wire_id: reply.job_id,
                    created_at: retention_now(),
                    peer_key: peer,
                    peer_id: peer_endpoint_id.to_owned(),
                    direction: JobDirection::Download,
                    sender: false,
                    state: if reply.manifest.is_some() {
                        JobState::Transferring
                    } else {
                        JobState::Preparing
                    },
                    manifest: reply.manifest,
                    local_root: dst_root,
                    local_rel: dst_rel.to_owned(),
                    names: Vec::new(),
                    local_pin: pin,
                    outgoing: true,
                    landed: HashMap::new(),
                    file_index: 0,
                    done_bytes: 0,
                    files_done: 0,
                    resumed_files: 0,
                    skipped_files: 0,
                    waiting_file: None,
                    skipped_symlinks: 0,
                    choices: HashMap::new(),
                    apply_remaining: None,
                    error: None,
                    in_flight: 0,
                    attempt: 0,
                    unsaved_landed: Vec::new(),
                    retry: None,
                    sent_at: None,
                    source_ids: Vec::new(),
                },
            );
        }
        save_jobs(&self.shared)?;
        Ok(id)
    }

    /// Snapshots one job record.
    pub fn job_snapshot(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        lock(&self.shared.jobs)
            .get(&id)
            .map(JobRecord::snapshot)
            .ok_or(Error::InvalidSetting(NO_JOB))
    }

    /// Snapshots all job records by id.
    pub fn job_list(&self) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let jobs = lock(&self.shared.jobs);
        let mut ids: Vec<u64> = jobs.keys().copied().collect();
        ids.sort();
        Ok(ids
            .iter()
            .filter_map(|id| jobs.get(id).map(JobRecord::snapshot))
            .collect())
    }

    /// Advances one job by at most one file: receivers fetch, stage and
    /// publish; senders ask the receiver for its state. Exact progress is
    /// visible between polls. Concurrent polls for the same job (background
    /// driver plus window ticks) serialize through a per-job driving guard:
    /// the second caller observes the current snapshot without advancing.
    pub fn job_poll(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        super::service_jobs::interrupt_orphaned(&self.shared);
        let sender = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state.is_terminal()
                || record.state == JobState::Interrupted
                || (record.state == JobState::WaitingDecision && !record.sender)
            {
                return Ok(record.snapshot());
            }
            // A retry waits out its pause.
            if record
                .retry
                .as_ref()
                .is_some_and(|retry| retry.next_at > std::time::Instant::now())
            {
                return Ok(record.snapshot());
            }
            record.sender
        };
        // Per-job driving guard: background plus window ticks must not
        // advance the same job concurrently.
        if !lock(&self.shared.job_driving).insert(id) {
            return self.job_snapshot(id);
        }
        struct Driving<'a>(&'a Shared, u64);
        impl Drop for Driving<'_> {
            fn drop(&mut self) {
                lock(&self.0.job_driving).remove(&self.1);
            }
        }
        let _driving = Driving(&self.shared, id);
        if sender {
            self.poll_sender(id)
        } else {
            self.poll_receiver(id)
        }
    }

    /// Advances every active job by one step (receivers fetch/publish,
    /// senders query receiver state). Used by the background driver and by
    /// native windows; receivers waiting for a decision are skipped.
    /// Senders keep polling so receiver-side decisions cannot strand them.
    /// Runs on the caller's thread and blocks like `job_poll`.
    pub fn drive_jobs(&self) -> Result<Vec<JobSnapshot>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let ids: Vec<u64> = {
            let jobs = lock(&self.shared.jobs);
            jobs.values()
                .filter(|record| {
                    !record.state.is_terminal()
                        && (record.sender || record.state != JobState::WaitingDecision)
                })
                .map(|record| record.id)
                .collect()
        };
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            out.push(self.job_poll(id)?);
        }
        Ok(out)
    }

    /// Starts the background transfer driver, independent of any Files
    /// window: closing every window pauses window ticks but not this
    /// driver, and jobs keep their state and resume. Each active job gets a
    /// worker of its own for as long as it has a step to take, so a large
    /// file for one job never holds up another. A receiver takes file after
    /// file until it is blocked, on a decision, on admission or on its peer,
    /// then waits for any job to change. A sender asks its receiver how far
    /// it got, soon again while that changes and ever more seldom while it
    /// does not ([`SENDER_SOON`], [`SENDER_SELDOM`]); its own answers wake
    /// nothing. The driver only starts workers, when a job changes and once
    /// a second. Call once after `set_transfer_directory` from the owning
    /// `Arc<Core>`; the driver and its workers end when the Core shuts down
    /// or drops.
    pub fn start_job_driver(self: &std::sync::Arc<Self>) {
        let _engine = self.engine();
        const IDLE: Duration = Duration::from_secs(1);
        let weak = std::sync::Arc::downgrade(self);
        let shared = self.shared.clone();
        std::thread::spawn(crate::trace::within(self.shared.span.clone(), move || {
            let mut seen = shared.job_revision.load(Ordering::Acquire);
            loop {
                let Some(core) = weak.upgrade() else {
                    break;
                };
                if lock(&core.shared.state).shut_down {
                    break;
                }
                core.start_job_workers();
                drop(core);
                seen = wait_for_job_change(&shared, seen, IDLE);
            }
        }));
    }

    /// Gives each job with a step to take a worker, unless it has one.
    fn start_job_workers(self: &std::sync::Arc<Self>) {
        let ids: Vec<u64> = {
            let jobs = lock(&self.shared.jobs);
            let mut workers = lock(&self.shared.job_workers);
            jobs.values()
                .filter(|record| record.drivable() && workers.insert(record.id))
                .map(|record| record.id)
                .collect()
        };
        for id in ids {
            let weak = std::sync::Arc::downgrade(self);
            let shared = self.shared.clone();
            let span = shared.span.clone();
            std::thread::spawn(crate::trace::within(span, move || {
                work_job(weak, shared, id)
            }));
        }
    }

    /// Takes a receiver's steps until one moves it no further: it waits on
    /// a decision, on admission or on its peer, or it ended.
    fn drive_receiver(&self, id: u64) {
        let Ok(mut before) = self.job_snapshot(id) else {
            return;
        };
        loop {
            let Ok(after) = self.job_poll(id) else {
                return;
            };
            let advanced =
                after.files_done + after.skipped_files > before.files_done + before.skipped_files;
            if after.state != JobState::Transferring || !advanced {
                return;
            }
            before = after;
        }
    }

    /// Advances whenever any job changes; see [`Core::job_changes_since`].
    pub fn job_revision(&self) -> u64 {
        let _engine = self.engine();
        self.shared.job_revision.load(Ordering::Acquire)
    }

    fn check_job_capacity(shared: &Shared) -> Result<(), Error> {
        let active = lock(&shared.jobs)
            .values()
            .filter(|record| ACTIVE_STATES.contains(&record.state))
            .count();
        if active >= MAX_ACTIVE_JOBS {
            return Err(Error::Busy);
        }
        Ok(())
    }

    /// Cancels one job. The partial file stays for an explicit resume; the
    /// destination never holds a partial file. Cancellation is symmetric:
    /// the peer is notified best-effort so its record stops as well. A
    /// cancelled sender serves nothing more, and the receiver re-checks
    /// cancellation before every publication, so a sender cancel still
    /// prevents delivery even when bytes already arrived.
    pub fn job_cancel(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let (peer_key, outgoing) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if !record.state.is_terminal() {
                record.enter(JobState::Cancelled);
                record.error = None;
            }
            (record.peer_key, record.outgoing)
        };
        if let Some(stop) = lock(&self.shared.job_cancel).remove(&id) {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
        stop_prefetch(&self.shared, id);
        // Best-effort peer notification: without a live session the local
        // record is still cancelled and serves nothing, so a later status
        // poll or resume observes the terminal state.
        told_peer(
            "cancel",
            id,
            self.notify_peer_cancel(&peer_key, outgoing, id),
        );
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    fn notify_peer_cancel(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
        let cancel = job::JobCancel {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&cancel)?;
        // The reply is an ack; any failure (no session, stale generation,
        // denied) still leaves the local cancellation in place.
        told_peer(
            "cancel",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_CANCEL, &payload)
                .map(drop),
        );
        Ok(())
    }

    /// Resumes an interrupted, cancelled or failed job, coordinating across
    /// endpoints after symmetric cancellation: the local record resumes (sender
    /// re-offers through handles, receiver returns to `Transferring`) and the
    /// peer is notified best-effort to resume and reauthorize its offers, so
    /// retry works from either side. Source changes fail instead of moving
    /// different bytes.
    pub fn job_resume(&self, id: u64) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        self.check_job_owner(id)?;
        let (sender, peer_key, outgoing) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            match record.state {
                JobState::Interrupted | JobState::Cancelled | JobState::Failed => {}
                _ => return Ok(record.snapshot()),
            }
            (record.sender, record.peer_key, record.outgoing)
        };
        // Resuming reactivates bulk work: respect the concurrency bound.
        Self::check_job_capacity(&self.shared)?;
        let snapshot = if sender {
            self.resume_sender(id)?
        } else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.enter(if record.manifest.is_some() {
                JobState::Transferring
            } else {
                JobState::Preparing
            });
            record.waiting_file = None;
            record.error = None;
            drop(state);
            drop(jobs);
            save_jobs(&self.shared)?;
            self.job_snapshot(id)?
        };
        // Best-effort peer coordination: without a live session the local
        // resume still stands and a later status poll reconciles.
        told_peer(
            "resume",
            id,
            self.notify_peer_resume(&peer_key, outgoing, id),
        );
        Ok(snapshot)
    }

    fn notify_peer_resume(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
        let resume = job::JobResume {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let payload = job::encode(&resume)?;
        told_peer(
            "resume",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_RESUME, &payload)
                .map(drop),
        );
        Ok(())
    }

    /// Answers the waiting conflict for one job. Receiver-owned conflicts
    /// resolve locally; sender-mirrored conflicts (upload collisions seen by
    /// the controller) forward the authenticated decision to the receiver
    /// that owns the file, so both directions share the same conflict
    /// behavior without requiring the receiver's Files window.
    pub fn job_resolve(
        &self,
        id: u64,
        choice: ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<JobSnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let (sender, peer_key, outgoing) = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            super::service_jobs::check_owner(&lock(&self.shared.state), record.owner)?;
            if record.state != JobState::WaitingDecision {
                return Err(Error::InvalidSetting(
                    "the job is not waiting for a decision",
                ));
            }
            let file = record.waiting_file.ok_or(Error::InvalidSetting(NO_JOB))?;
            record.choices.insert(file, choice);
            if apply_to_remaining {
                record.apply_remaining = Some(choice);
            }
            // Receivers own their conflicts; senders mirror and forward.
            // Optimistically advance local state; the next status poll
            // reconciles if forwarding failed.
            record.enter(JobState::Transferring);
            record.waiting_file = None;
            (record.sender, record.peer_key, record.outgoing)
        };
        save_jobs(&self.shared)?;
        if sender {
            told_peer(
                "choice",
                id,
                self.notify_peer_resolve(&peer_key, outgoing, id, choice, apply_to_remaining),
            );
        }
        self.job_snapshot(id)
    }

    /// Forgets one terminal job (Complete, Cancelled or Failed), releasing its
    /// staging tree and orphaned offers. Active, WaitingDecision and
    /// Interrupted jobs are refused (resume or cancel first). Interrupted
    /// retains staging/store for byte-reusing resume; forgetting it would
    /// force full re-download, so explicit cancel-then-forget is required
    /// (cancel retains staging for resume; forget releases it).
    pub fn job_forget(&self, id: u64) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            match record.state {
                JobState::Complete | JobState::Cancelled | JobState::Failed => {}
                _ => {
                    return Err(Error::InvalidSetting(
                        "only finished jobs can be forgotten; cancel or complete first",
                    ));
                }
            }
        }
        // Release staging: Complete already removed it; Cancelled may keep
        // a partial for resume, which forgetting releases.
        remove_staging(&self.shared, id);
        lock(&self.shared.jobs).remove(&id);
        save_jobs(&self.shared)?;
        Ok(())
    }

    fn notify_peer_resolve(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        job_id: u64,
        choice: ConflictChoice,
        apply_to_remaining: bool,
    ) -> Result<(), Error> {
        let local = job_id;
        let (job_id, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&job_id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (wire_job_id(record), record.owner)
        };
        let generation = self.generation_for(peer, outgoing, owner)?;
        let choice_name = match choice {
            ConflictChoice::Skip => "skip",
            ConflictChoice::Replace => "replace",
            ConflictChoice::KeepBoth => "keep-both",
        }
        .to_owned();
        let resolve = job::JobResolve {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
            choice: choice_name,
            apply_to_remaining,
        };
        let payload = job::encode(&resolve)?;
        told_peer(
            "choice",
            local,
            self.job_round_trip_side(peer, outgoing, owner, job::TAG_JOB_RESOLVE, &payload)
                .map(drop),
        );
        Ok(())
    }

    fn poll_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        let (peer_key, outgoing, owner) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.peer_key, record.outgoing, record.owner)
        };
        let generation = match self.generation_for(&peer_key, outgoing, owner) {
            Ok(generation) => generation,
            Err(Error::NoSession | Error::StaleSession) => {
                return self.retry_later(id, UNREACHABLE)
            }
            Err(error) => return Err(error),
        };
        let query = job::JobStatus {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id: wire_job_id(
                lock(&self.shared.jobs)
                    .get(&id)
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
            ),
        };
        let reply_bytes = match self.job_round_trip_side(
            &peer_key,
            outgoing,
            owner,
            job::TAG_JOB_STATUS,
            &job::encode(&query)?,
        ) {
            Ok(bytes) => bytes,
            Err(_) => return self.retry_later(id, UNREACHABLE),
        };
        let reply: job::JobStateReply = job::decode(&reply_bytes).map_err(|_| Error::Malformed)?;
        if reply.id != query.id {
            return Err(Error::Malformed);
        }
        if reply.status != job::STATUS_OK {
            return self.job_snapshot(id);
        }
        let state = job::JobState::from_name(&reply.state).ok_or(Error::Malformed)?;
        let changed = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let before = record.reported();
            Self::retry_settled(record);
            // Do not overwrite a locally cancelled record with a stale
            // receiver state: cancellation wins.
            if record.state != JobState::Cancelled {
                // The receiver is the one that knows what has arrived.
                record.done_bytes = reply.done_bytes;
                record.files_done = reply.files_done;
                record.skipped_files = reply.skipped_files;
                match state {
                    JobState::Complete => {
                        record.enter(JobState::Complete);
                        record.error = None;
                        record.waiting_file = None;
                    }
                    JobState::Failed | JobState::Cancelled | JobState::Interrupted => {
                        record.enter(state);
                        record.error = reply.error.map(|text| truncate_error(&text));
                        record.waiting_file = None;
                    }
                    JobState::WaitingDecision => {
                        record.enter(JobState::WaitingDecision);
                        record.error = None;
                        // Map the receiver's waiting path back to this
                        // record's file index (manifests correlate by id).
                        record.waiting_file = reply.waiting_file.as_deref().and_then(|path| {
                            record
                                .manifest
                                .as_ref()?
                                .entries
                                .iter()
                                .filter(|entry| !entry.is_dir)
                                .position(|entry| entry.path == path)
                        });
                    }
                    JobState::Transferring if record.state == JobState::WaitingDecision => {
                        super::service_jobs::check_owner(&lock(&self.shared.state), record.owner)?;
                        record.enter(JobState::Serving);
                        record.waiting_file = None;
                    }
                    _ => {}
                }
            }
            record.reported() != before
        };
        // An answer that changes nothing is not saved and wakes no one.
        if changed {
            save_jobs(&self.shared)?;
        }
        self.job_snapshot(id)
    }

    fn poll_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Cancellation or connection interruption stops before any fetch.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            if record.manifest.is_none() {
                drop(jobs);
                return self.ask_prepared_manifest(id);
            }
        }
        let (wire_job, peer_bytes, file_index, file_count) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                wire_job_id(record),
                record.peer_key,
                record.file_index,
                record.files().len(),
            )
        };
        if file_index >= file_count {
            // Zero-file manifests still carry directory entries: build the
            // skeleton before claiming completion so empty folders land.
            // Revocation stops here: no new directories after the grant is gone.
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id, PERMISSION_OFF);
            }
            if let Err(error) = self.ensure_receiver_dirs(id) {
                return self.interrupt_on_rebind(id, error);
            }
            match self.finalize_receiver(id) {
                Err(error) => return self.interrupt_on_rebind(id, error),
                Ok(snapshot) => return Ok(snapshot),
            }
        }
        // First poll ensures the directory skeleton exactly once per job;
        // creation is idempotent, so repeats stay safe. Revocation stops
        // directory creation as well as file publication.
        if file_index == 0 {
            if !Self::receiver_granted(&self.shared, &peer_bytes) {
                return self.mark_interrupted(id, PERMISSION_OFF);
            }
            if let Err(error) = self.ensure_receiver_dirs(id) {
                return self.interrupt_on_rebind(id, error);
            }
        }
        let entry = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.files().get(file_index).cloned().cloned()
        }
        .ok_or(Error::InvalidSetting(NO_JOB))?;
        // Conflict check before any fetch: the destination wins ties until
        // the owner decides.
        let choice = match self.conflict_choice(id, file_index, &entry) {
            Err(error) => return self.interrupt_on_rebind(id, error),
            Ok(choice) => choice,
        };
        let Some(choice) = choice else {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.enter(JobState::WaitingDecision);
            record.waiting_file = Some(file_index);
            drop(jobs);
            save_jobs(&self.shared)?;
            return self.job_snapshot(id);
        };
        if choice == ConflictChoice::Skip {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            // A cancelled poll between check and commit still stops here.
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
            record.skipped_files += 1;
            record.done_bytes = record.done_bytes.saturating_add(entry.size);
            record.in_flight = 0;
            record.file_index += 1;
            drop(jobs);
            save_jobs(&self.shared)?;
            return self.job_snapshot(id);
        }
        // Fetch under the receiver's own grant: revocation stops new
        // reads here even while the sender still serves.
        if !Self::receiver_granted(&self.shared, &peer_bytes) {
            return self.mark_interrupted(id, PERMISSION_OFF);
        }
        // Admission precedes network fetching and export. Saturated workers
        // leave the job queued for a subsequent poll; impossible budgets fail
        // with a durable, payload-free explanation.
        let available = match self.receiver_available_storage(id, &entry.path) {
            Ok(available) => available,
            Err(Error::InvalidSetting(msg)) if msg == fs::ROOTS_REBOUND => {
                return self.mark_interrupted(id, fs::ROOTS_REBOUND);
            }
            // The destination folder is gone or unreadable: say so on the
            // job rather than failing every poll.
            Err(Error::InvalidSetting(msg)) => return self.mark_failed(id, msg),
            Err(error) => return Err(error),
        };
        let _reservation =
            match super::file_resources::reserve(&self.shared, id, entry.size, available) {
                Ok(Some(reservation)) => reservation,
                Ok(None) => return self.job_snapshot(id),
                Err(Error::InvalidSetting(message)) => return self.mark_failed(id, message),
                Err(error) => return Err(error),
            };
        // Fetch into the file's partial with a per-poll cancel watch.
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        // The run this fetch belongs to: what it finds is applied only while
        // the job is still that run.
        let attempt = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            super::service_jobs::check_owner(&state, record.owner)?;
            lock(&self.shared.job_cancel).insert(id, stop_tx);
            record.attempt
        };
        // A partial kept from an earlier attempt resumes where it ended.
        let (staged, segments) = self.stage_paths(id, &entry.path)?;
        for path in [&staged, &segments] {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| Error::InvalidSetting(STAGE_FAILED))?;
            }
        }
        // This file may already be arriving: take it before the next small
        // files start, so no second fetch ever writes its partial.
        let ahead = lock(&self.shared.job_prefetch)
            .get_mut(&id)
            .and_then(|prefetch| prefetch.files.remove(&file_index));
        self.prefetch_after(id, &peer_bytes, wire_job, file_index);
        let arrived = ahead
            .map(|(_, fetching)| self.runtime.block_on(fetching))
            .and_then(|joined| joined.ok());
        let (progress, arriving) = std::sync::mpsc::channel();
        let watcher = follow_arrival(self.shared.jobs.clone(), self.shared.clone(), id, arriving);
        let file = u32::try_from(file_index).map_err(|_| Error::InvalidSetting(NO_JOB))?;
        // A file fetched ahead is used only when it arrived whole, on the
        // connection still kept: revoking closes it, and a file that came
        // before must not land after. Anything else is fetched again here,
        // with this poll's retries and cancel.
        let fetch = match arrived {
            Some((Ok(done @ Received::Done { .. }), Some(via)))
                if super::transfer::still_connected(&self.shared, &peer_bytes, via) =>
            {
                drop(progress);
                Ok(done)
            }
            _ => self.fetch_job_file(
                &peer_bytes,
                wire_job,
                file,
                entry.size,
                &staged,
                &segments,
                progress,
                stop_rx,
            ),
        };
        if watcher.join().is_err() {
            tracing::warn!(target: crate::trace::FILES, job = id, "a transfer's cancel watcher panicked");
        }
        // A cancel or interruption during fetch preserves its state even if
        // bytes arrived; a job that stopped and runs again is a later run's,
        // whose cancel handle this fetch leaves in place.
        {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.attempt != attempt {
                return Ok(record.snapshot());
            }
            lock(&self.shared.job_cancel).remove(&id);
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
        }
        // Revocation between fetch and publication must not create the
        // destination file: re-check authorization before any staging or
        // commit. The publication itself re-checks as well.
        if !Self::receiver_granted(&self.shared, &peer_bytes) {
            return self.mark_interrupted(id, PERMISSION_OFF);
        }
        use crate::transfer::{self, Received};
        let downloaded = match fetch {
            Ok(Received::Done { downloaded }) => downloaded,
            // A sender stops serving only when its job stopped or its
            // permission or folder went: it will not serve again by itself.
            Ok(Received::Denied) => return self.mark_interrupted(id, SENDER_STOPPED),
            Ok(Received::ShutDown) => return self.retry_later(id, SENDER_SHUT_DOWN),
            Ok(Received::Stalled) => return self.retry_later(id, STALLED),
            Ok(Received::Busy) => return self.retry_later(id, SENDER_BUSY),
            Err(Error::InvalidSetting(reason))
                if reason == transfer::NO_SPACE || reason == transfer::PARTIAL_FAILED =>
            {
                return self.mark_failed(id, reason);
            }
            Err(_) => return self.retry_later(id, UNREACHABLE),
            Ok(Received::Changed) => return self.mark_failed(id, SOURCE_CHANGED),
            Ok(Received::Mismatch) => return self.mark_failed(id, VERIFY_FAILED),
            // A stopped fetch belongs to a job still cancelled or interrupted,
            // or to an earlier run: both returned above.
            Ok(Received::Cancelled) => return self.job_snapshot(id),
        };
        // The partial holds the verified file: publish it per the decision.
        let staged_size = std::fs::metadata(&staged)
            .map(|meta| meta.len())
            .unwrap_or(u64::MAX);
        if staged_size != entry.size {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(&staged)
            );
            return self.mark_failed(id, VERIFY_FAILED);
        }
        let landed = match self.publish_staged(id, &entry, &staged, choice) {
            Err(error) => return self.interrupt_on_rebind(id, error),
            Ok(Some(landed)) => landed,
            Ok(None) => return self.job_snapshot(id),
        };
        // The partial is published; its journal describes nothing now.
        crate::ignored!(
            crate::trace::FILES,
            "a staged file left behind is swept with its job's staging",
            std::fs::remove_file(&segments)
        );
        // Skip leaves the destination untouched; anything published lands
        // under its final name for verification.
        let _ = landed;
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let at = record.file_index;
            if choice != ConflictChoice::Skip {
                record.land(at, landed);
            }
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                drop(jobs);
                save_jobs(&self.shared)?;
                return self.job_snapshot(id);
            }
            if downloaded < entry.size {
                record.resumed_files += 1;
            }
            Self::retry_settled(record);
            record.files_done += 1;
            record.done_bytes = record.done_bytes.saturating_add(entry.size);
            record.in_flight = 0;
            record.file_index += 1;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Asks the host whether a download it is preparing is ready, and
    /// takes its manifest once it is. Still preparing leaves the job as it
    /// is; a refusal fails it.
    fn ask_prepared_manifest(&self, id: u64) -> Result<JobSnapshot, Error> {
        let (peer, outgoing, owner, job_id) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.peer_key,
                record.outgoing,
                record.owner,
                wire_job_id(record),
            )
        };
        let generation = match self.generation_for(&peer, outgoing, owner) {
            Ok(generation) => generation,
            Err(Error::NoSession | Error::StaleSession) => {
                return self.retry_later(id, UNREACHABLE)
            }
            Err(error) => return Err(error),
        };
        let query = job::JobStatus {
            v: job::JOB_VERSION,
            id: 1,
            generation,
            job_id,
        };
        let Ok(bytes) = self.job_round_trip_side(
            &peer,
            outgoing,
            owner,
            job::TAG_JOB_MANIFEST,
            &job::encode(&query)?,
        ) else {
            return self.job_snapshot(id);
        };
        let reply: job::ManifestReply = job::decode(&bytes)?;
        if reply.id != query.id || reply.job_id != job_id {
            return Err(Error::Malformed);
        }
        let manifest = match (reply.status, reply.manifest) {
            (job::STATUS_OK, None) => return self.job_snapshot(id),
            (job::STATUS_OK, Some(manifest)) => manifest,
            (job::STATUS_STALE, _) => return self.retry_later(id, CONNECTION_ENDED),
            _ => return self.mark_failed(id, PREPARE_FAILED),
        };
        if manifest::validate(&manifest).is_err() {
            return self.mark_failed(id, PREPARE_FAILED);
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.manifest.is_some() || record.state.is_terminal() {
                return Ok(record.snapshot());
            }
            record.manifest = Some(manifest);
            if record.state == JobState::Preparing {
                record.enter(JobState::Transferring);
            }
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Returns the stored choice, the job-wide choice, or `None` when the
    /// destination is free. A waiting state is entered by the caller.
    fn conflict_choice(
        &self,
        id: u64,
        file_index: usize,
        entry: &ManifestEntry,
    ) -> Result<Option<ConflictChoice>, Error> {
        let (local_root, local_rel) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone())
        };
        let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
        if std::fs::symlink_metadata(&dest).is_err() {
            return Ok(Some(ConflictChoice::Replace));
        }
        let jobs = lock(&self.shared.jobs);
        let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
        if let Some(choice) = record.choices.get(&file_index).copied() {
            return Ok(Some(choice));
        }
        if let Some(choice) = record.apply_remaining {
            return Ok(Some(choice));
        }
        Ok(None)
    }

    fn receiver_dest(
        &self,
        id: u64,
        root: u64,
        rel: &str,
        path: &str,
    ) -> Result<std::path::PathBuf, Error> {
        let mut pin = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.local_pin.clone()
        };
        let adopted = pin.is_none();
        let dest = {
            let roots = lock(&self.shared.file_roots);
            // A rebound destination fails the leg instead of writing
            // staged bytes into whatever the path resolves to now.
            verify_leg_pin(&roots, root, &mut pin)?;
            let (parent_rel, name) = match path.rfind('/') {
                Some(slash) => (&path[..slash], &path[slash + 1..]),
                None => ("", path),
            };
            // The manifest jail already validated every component; resolve the
            // parent through it (creating missing levels idempotently) and
            // append the single validated file name.
            let full_parent = if rel.is_empty() {
                parent_rel.to_owned()
            } else if parent_rel.is_empty() {
                rel.to_owned()
            } else {
                format!("{rel}/{parent_rel}")
            };
            if roots.resolve_dir(root, &full_parent).is_err() {
                manifest::create_levels(&roots, root, rel, parent_rel)?;
            }
            roots.resolve_dir(root, &full_parent)?.join(name)
        };
        if adopted {
            if let Some(record) = lock(&self.shared.jobs).get_mut(&id) {
                record.local_pin = pin;
            }
        }
        Ok(dest)
    }

    fn receiver_available_storage(&self, id: u64, path: &str) -> Result<u64, Error> {
        let (root, rel) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone())
        };
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        let full = match (rel.is_empty(), parent.is_empty()) {
            (true, _) => parent.to_owned(),
            (_, true) => rel,
            _ => format!("{rel}/{parent}"),
        };
        #[cfg(unix)]
        let destination = {
            let fd = lock(&self.shared.file_roots).open_dir_fd(root, &full)?;
            super::file_resources::available_fd(&fd)?
        };
        #[cfg(not(unix))]
        let destination = {
            let path = lock(&self.shared.file_roots).resolve_dir(root, &full)?;
            super::file_resources::available(&path)?
        };
        let transfer_dir = super::transfer::transfer_directory(&self.shared)?;
        Ok(destination.min(super::file_resources::available(&transfer_dir)?))
    }

    /// Starts fetching the small files after `current` that are not
    /// already arriving, up to [`PREFETCH_FILES`] and [`PREFETCH_BYTES`]
    /// ahead. A file of more than one segment is left for its turn.
    fn prefetch_after(&self, id: u64, peer: &[u8; 32], wire_job: u64, current: usize) {
        #[cfg(any(test, feature = "test-probes"))]
        if self.shared.prefetch_off.load(Ordering::Relaxed) {
            return;
        }
        let Ok(addr) = super::transfer::peer_addr(&self.shared, peer) else {
            return;
        };
        let started = {
            let jobs = lock(&self.shared.jobs);
            let Some(record) = jobs.get(&id) else {
                return;
            };
            let mut prefetches = lock(&self.shared.job_prefetch);
            let prefetch = prefetches.entry(id).or_default();
            // Files the job already passed, landed or skipped, are done with.
            prefetch.files.retain(|index, _| *index > current);
            let mut count = prefetch.files.len();
            let mut bytes: u64 = prefetch.files.values().map(|(size, _)| size).sum();
            let mut started = Vec::new();
            for (index, entry) in record.files().iter().enumerate().skip(current + 1) {
                if count >= PREFETCH_FILES || bytes >= PREFETCH_BYTES {
                    break;
                }
                if entry.size > crate::transfer::SEGMENT_LEN || prefetch.files.contains_key(&index)
                {
                    continue;
                }
                count += 1;
                bytes += entry.size;
                started.push((index, entry.path.clone(), entry.size));
            }
            let stop = prefetch
                .stop
                .get_or_insert_with(|| tokio::sync::watch::channel(false).0)
                .clone();
            (started, stop)
        };
        let (started, stop) = started;
        for (index, path, size) in started {
            let Ok((staged, segments)) = self.stage_paths(id, &path) else {
                return;
            };
            let made = [&staged, &segments].into_iter().all(|path| {
                path.parent()
                    .is_none_or(|parent| std::fs::create_dir_all(parent).is_ok())
            });
            let Ok(file) = u32::try_from(index) else {
                return;
            };
            if !made {
                return;
            }
            let shared = self.shared.clone();
            let handle = self.runtime.handle().clone();
            let peer = *peer;
            let addr = addr.clone();
            let mut cancel = stop.subscribe();
            // A blocking worker drives it: partial writes block.
            let fetching = self.runtime.spawn_blocking(move || {
                let (progress, _) = std::sync::mpsc::channel();
                let mut via = None;
                let fetched = handle.block_on(super::transfer::fetch_file(
                    &shared,
                    &peer,
                    addr,
                    crate::transfer::FileRequest {
                        job: wire_job,
                        file,
                        offset: 0,
                        held: 0,
                    },
                    size,
                    &staged,
                    &segments,
                    &progress,
                    &mut cancel,
                    &mut via,
                ));
                (fetched, via)
            });
            lock(&self.shared.job_prefetch)
                .entry(id)
                .or_default()
                .files
                .insert(index, (size, fetching));
        }
    }

    /// Where file `rel_path` of job `id` arrives, and its segment journal.
    fn stage_paths(
        &self,
        id: u64,
        rel_path: &str,
    ) -> Result<(std::path::PathBuf, std::path::PathBuf), Error> {
        let job = super::transfer::transfer_directory(&self.shared)?
            .join(STAGING_DIR)
            .join(format!("job-{id}"));
        Ok((
            job.join(STAGED_FILES).join(rel_path),
            job.join(STAGED_SEGMENTS).join(rel_path),
        ))
    }

    /// A rebound destination leg interrupts the job instead of wedging
    /// the poll: the error is retryable, so restoring the original roots
    /// and resuming continues. Any other error keeps its meaning.
    fn interrupt_on_rebind(&self, id: u64, error: Error) -> Result<JobSnapshot, Error> {
        match error {
            Error::InvalidSetting(msg) if msg == fs::ROOTS_REBOUND => {
                self.mark_interrupted(id, fs::ROOTS_REBOUND)
            }
            error => Err(error),
        }
    }

    fn ensure_receiver_dirs(&self, id: u64) -> Result<(), Error> {
        // Snapshot under short locks; mkdirs may block on slow storage, so
        // no job/state/roots lock is held across them. Publish re-checks
        // owner/grant before committing.
        let (local_root, local_rel, manifest, roots_snapshot, pin) = {
            let jobs = lock(&self.shared.jobs);
            let state = lock(&self.shared.state);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            super::service_jobs::check_owner(&state, record.owner)?;
            let mut pin = record.local_pin.clone();
            let adopted = pin.is_none();
            // Verify and clone under one roots hold: the snapshot is
            // exactly the verified epoch, so a rebind cannot slip
            // skeleton mkdirs into the new directory.
            let roots_snapshot = {
                let roots = lock(&self.shared.file_roots);
                verify_leg_pin(&roots, record.local_root, &mut pin)?;
                roots.clone()
            };
            (
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                roots_snapshot,
                adopted.then_some(pin),
            )
        };
        // Persist a legacy adopt; the snapshot above is already dropped
        // (sequential, never nested).
        if let Some(pin) = pin {
            if let Some(record) = lock(&self.shared.jobs).get_mut(&id) {
                record.local_pin = pin;
            }
        }
        #[cfg(unix)]
        let mkdir_probe = {
            let hooks = lock(&PRESTAGE_HOOKS);
            if hooks.engine == self.prestage_key() && hooks.job_id == Some(id) {
                hooks.mkdir_probe
            } else {
                None
            }
        };
        #[cfg(unix)]
        if let Some(probe) = mkdir_probe {
            probe(id);
        }
        // The destination base itself may be fresh: build it first so the
        // per-entry levels below always start from an existing folder.
        if !local_rel.is_empty() && roots_snapshot.resolve_dir(local_root, &local_rel).is_err() {
            manifest::create_levels(&roots_snapshot, local_root, "", &local_rel)?;
        }
        manifest::ensure_manifest_dirs(&roots_snapshot, local_root, &local_rel, &manifest)
    }

    /// Publishes one staged file, returning the landed file name.
    /// Returns `None` when the poll must stop with the stored snapshot
    /// (a lost KeepBoth race re-enters the conflict state).
    /// Enforces the transfer grant at publication: revocation between fetch
    /// and commit refuses the write instead of creating the destination.
    fn publish_staged(
        &self,
        id: u64,
        entry: &ManifestEntry,
        staged: &std::path::Path,
        choice: ConflictChoice,
    ) -> Result<Option<String>, Error> {
        let (local_root, local_rel, peer_key) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (record.local_root, record.local_rel.clone(), record.peer_key)
        };
        if !Self::receiver_granted(&self.shared, &peer_key) {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(staged)
            );
            self.mark_interrupted(id, PERMISSION_OFF)?;
            return Ok(None);
        }
        let policy = match choice {
            ConflictChoice::Replace => manifest::PublishPolicy::Replace,
            ConflictChoice::KeepBoth => manifest::PublishPolicy::KeepBoth,
            ConflictChoice::Skip => return Ok(None),
        };
        #[cfg(unix)]
        {
            self.publish_staged_anchored(id, &entry.path, staged, policy, local_root, &local_rel)
        }
        #[cfg(not(unix))]
        {
            let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
            let dest_dir = dest
                .parent()
                .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?
                .to_path_buf();
            let file_name = dest
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?
                .to_owned();
            match manifest::publish_file(&dest_dir, &file_name, staged, policy) {
                Ok(landed) => Ok(Some(landed)),
                Err(Error::FileExists) => {
                    // Lost a KeepBoth race after choosing: surface the
                    // conflict again instead of failing the job.
                    let mut jobs = lock(&self.shared.jobs);
                    let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
                    record.choices.remove(&record.file_index);
                    record.enter(JobState::WaitingDecision);
                    record.waiting_file = Some(record.file_index);
                    drop(jobs);
                    save_jobs(&self.shared)?;
                    Ok(None)
                }
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    Ok(None)
                }
            }
        }
    }

    /// Test hooks for cross-volume pre-staging; see [`PRESTAGE_HOOKS`].
    /// Production leaves them unset.
    #[cfg(unix)]
    pub fn set_prestage_hooks(hooks: PrestageHooks) {
        *lock(&PRESTAGE_HOOKS) = hooks;
    }

    /// Identifies this engine to its test hooks: [`PrestageHooks`] on
    /// Unix, and [`PREPARE_SCAN_HOOKS`] everywhere.
    #[cfg(any(unix, test, feature = "test-probes"))]
    pub fn prestage_key(&self) -> usize {
        let _engine = self.engine();
        Arc::as_ptr(&self.shared) as usize
    }

    /// Test probe for in-flight serve opens; see [`SERVE_SOURCE_PROBE`].
    /// Production leaves it unset.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn set_serve_source_probe(probe: Option<fn(usize, u64)>) {
        *lock(&SERVE_SOURCE_PROBE) = probe;
    }

    /// Files of job `id` being fetched ahead of their turn.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn fetching_ahead(&self, id: u64) -> usize {
        let _engine = self.engine();
        lock(&self.shared.job_prefetch)
            .get(&id)
            .map_or(0, |prefetch| prefetch.files.len())
    }

    /// Takes one file at a time, fetching none ahead, for tests that stop
    /// a job between two files.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn fetch_one_file_at_a_time(&self) {
        let _engine = self.engine();
        self.shared.prefetch_off.store(true, Ordering::Relaxed);
    }

    /// Identifies this engine to [`SERVE_SOURCE_PROBE`].
    #[cfg(any(test, feature = "test-probes"))]
    pub fn engine_key(&self) -> usize {
        let _engine = self.engine();
        Arc::as_ptr(&self.shared) as usize
    }

    /// Receiver liveness for one pre-stage block: the job still exists and
    /// is not stood down, the peer keeps the transfer grant, and the Files
    /// owner (if any) is still live. Locks are taken and released per
    /// call; the copy loop never holds them across I/O.
    #[cfg(unix)]
    fn prestage_live(&self, id: u64) -> bool {
        let (owner, peer_key) = {
            let jobs = lock(&self.shared.jobs);
            let Some(record) = jobs.get(&id) else {
                return false;
            };
            if matches!(
                record.state,
                JobState::Cancelled | JobState::Interrupted | JobState::Failed
            ) {
                return false;
            }
            (record.owner, record.peer_key)
        };
        let state = lock(&self.shared.state);
        let granted = match state.trust.get(&peer_key) {
            Some(device) => state.trust.granted(&device.endpoint_id, Grant::Transfer),
            None => false,
        };
        granted && super::service_jobs::check_owner(&state, owner).is_ok()
    }

    /// Cross-volume pre-stage, run OUTSIDE global locks. When the staged
    /// file and the destination live on different filesystems, stream the
    /// bytes to a temp sibling on the destination volume with per-block
    /// cancel/grant checks; the commit under locks is then a fast
    /// same-filesystem rename. Returns the stage to commit, `None` for
    /// the same-volume fast path, or stands the file down (record
    /// updated, siblings cleaned) and returns `None` with `stood_down`.
    #[cfg(unix)]
    fn prestage_cross_volume<'a>(
        &self,
        id: u64,
        staged: &std::path::Path,
        dir_fd: std::os::unix::io::BorrowedFd<'a>,
    ) -> Result<(Option<vnilink_rename::anchored::SiblingStage<'a>>, bool), Error> {
        use std::io::Read;
        let (force, probe, sync_probe) = {
            let hooks = lock(&PRESTAGE_HOOKS);
            let scoped = hooks.engine == self.prestage_key() && hooks.job_id == Some(id);
            (
                scoped && hooks.force_cross_volume,
                if scoped { hooks.probe } else { None },
                if scoped { hooks.sync_probe } else { None },
            )
        };
        if !force {
            match vnilink_rename::anchored::same_filesystem(staged, dir_fd) {
                Ok(true) => return Ok((None, false)),
                Ok(false) => {}
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    return Ok((None, true));
                }
            }
        }
        let mut stage = match vnilink_rename::anchored::SiblingStage::create(dir_fd) {
            Ok(stage) => stage,
            Err(_) => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
        };
        let mut src = match std::fs::File::open(staged) {
            Ok(file) => file,
            Err(_) => {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
        };
        let mut buf = vec![0u8; 64 * 1024];
        let mut copied: u64 = 0;
        loop {
            if !self.prestage_live(id) {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_interrupted(id, STOPPED)?;
                return Ok((None, true));
            }
            if let Some(probe) = probe {
                probe(id, copied);
            }
            let n = match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => {
                    crate::ignored!(
                        crate::trace::FILES,
                        "a staged file left behind is swept with its job's staging",
                        std::fs::remove_file(staged)
                    );
                    self.mark_failed(id, PUBLISH_FAILED)?;
                    return Ok((None, true));
                }
            };
            if stage.write_all(&buf[..n]).is_err() {
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                self.mark_failed(id, PUBLISH_FAILED)?;
                return Ok((None, true));
            }
            copied += n as u64;
        }
        // Durability lands here, before the authority gate: the commit under
        // locks performs only the atomic rename via `commit_prepared`.
        if let Some(probe) = sync_probe {
            probe(id);
        }
        if stage.fsync().is_err() {
            crate::ignored!(
                crate::trace::FILES,
                "a staged file left behind is swept with its job's staging",
                std::fs::remove_file(staged)
            );
            self.mark_failed(id, PUBLISH_FAILED)?;
            return Ok((None, true));
        }
        Ok((Some(stage), false))
    }

    /// Anchored publication: the destination parent is opened as a directory
    /// handle and the staged file is committed with `renameat`, so a parent
    /// replaced by a symlink cannot redirect the write outside the root.
    /// Cross-volume bytes are pre-staged to a destination sibling BEFORE
    /// locking; the final authorization/state decision is synchronized with
    /// the commit: both the job and grant locks are held across the re-check
    /// and the fast same-filesystem `renameat` only, so receiver
    /// cancellation (`Cancelled`) or revocation (`Interrupted` via grant
    /// loss) completing before the rename prevents destination creation.
    #[cfg(unix)]
    fn publish_staged_anchored(
        &self,
        id: u64,
        manifest_path: &str,
        staged: &std::path::Path,
        policy: manifest::PublishPolicy,
        local_root: u64,
        local_rel: &str,
    ) -> Result<Option<String>, Error> {
        use std::os::unix::io::AsFd;
        let (parent_rel, file_name) = match manifest_path.rfind('/') {
            Some(slash) => (&manifest_path[..slash], &manifest_path[slash + 1..]),
            None => ("", manifest_path),
        };
        let full_parent = if local_rel.is_empty() {
            parent_rel.to_owned()
        } else if parent_rel.is_empty() {
            local_rel.to_owned()
        } else {
            format!("{local_rel}/{parent_rel}")
        };
        // Ensure the parent chain exists through anchored mkdirs, then open it.
        // Mkdirs may block on slow storage: snapshot the owner check and the
        // root set under short locks, then run every filesystem call with no
        // job/state/roots lock held. The commit below re-locks and re-checks
        // cancellation, grant and owner before the rename.
        let (roots_snapshot, mut pin, adopted) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            let mut pin = record.local_pin.clone();
            let adopted = pin.is_none();
            // Verify and clone under one roots hold: pre-stage mkdirs
            // below run against exactly the verified epoch.
            let roots = lock(&self.shared.file_roots);
            verify_leg_pin(&roots, record.local_root, &mut pin)?;
            (roots.clone(), pin, adopted)
        };
        let mkdir_probe = {
            let hooks = lock(&PRESTAGE_HOOKS);
            if hooks.engine == self.prestage_key() && hooks.job_id == Some(id) {
                hooks.mkdir_probe
            } else {
                None
            }
        };
        if roots_snapshot
            .open_dir_fd(local_root, &full_parent)
            .is_err()
        {
            if let Some(probe) = mkdir_probe {
                probe(id);
            }
            manifest::create_levels(&roots_snapshot, local_root, local_rel, parent_rel)?;
        }
        let roots = lock(&self.shared.file_roots);
        // The dir handle below pins whatever the live roots resolve NOW:
        // re-verify the leg first so a rebound destination fails instead
        // of binding the new directory.
        verify_leg_pin(&roots, local_root, &mut pin)?;
        let dir_fd = roots.open_dir_fd(local_root, &full_parent)?;
        drop(roots);
        // For KeepBoth, choose the free name through the anchored listing
        // before locking; the no-replace commit below decides atomically and
        // re-enters conflict on a lost race.
        let free_name: Option<String> = match policy {
            manifest::PublishPolicy::Replace => None,
            manifest::PublishPolicy::KeepBoth => {
                let existing: std::collections::HashSet<String> =
                    vnilink_rename::anchored::list_names(dir_fd.as_fd())
                        .map_err(|_| Error::InvalidSetting(PUBLISH_FAILED))?
                        .into_iter()
                        .filter_map(|os| os.into_string().ok())
                        .collect();
                Some(manifest::keep_both_name(file_name, |candidate| {
                    existing.contains(candidate)
                }))
            }
        };
        // Cross-volume bytes move to a destination sibling BEFORE locking;
        // the commit below only renames within one filesystem, so slow
        // destination I/O never holds global locks. A stood-down file
        // (cancelled/revoked/failed mid-copy) already updated its record.
        let (mut stage, stood_down) = self.prestage_cross_volume(id, staged, dir_fd.as_fd())?;
        if stood_down {
            return Ok(None);
        }
        // One commit per publish: consumes the pre-staged sibling when the
        // volumes differ, else renames staged directly (strict: EXDEV
        // means a remount raced the pre-stage check and stands down
        // retryably). Reports whether the caller must remove `staged`
        // (sibling commits leave it behind; renames move it).
        let mut commit_file = |dest_name: &str, replace: bool| -> std::io::Result<bool> {
            if let Some(stage) = stage.take() {
                // Pre-synced above: rename only, no slow I/O under the gate.
                stage.commit_prepared(dest_name, replace)?;
                Ok(true)
            } else {
                vnilink_rename::anchored::rename_from_staged(
                    staged,
                    dir_fd.as_fd(),
                    dest_name,
                    replace,
                )?;
                Ok(false)
            }
        };
        // Commit under both locks: re-check cancellation and grant after any
        // barrier, then fast same-filesystem `renameat` without releasing.
        // Revocation (grant update needs `state`, interruption needs `jobs`)
        // and cancellation (needs `jobs`) both block here, so completing
        // before the rename prevents it.
        let commit: Result<Option<String>, Error> = (|| {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(None);
            }
            // Persist a legacy adopt: later publishes are strict. The
            // adopted pin was verified against the live roots above before
            // the destination handle was opened.
            if adopted {
                record.local_pin = pin.clone();
            }
            let peer_key = record.peer_key;
            // Hold `state` from grant check through `renameat`: revoke's grant
            // update needs `state`, so it blocks until after the commit.
            // Jobs->state order; no other path nests state->jobs.
            let state_guard = lock(&self.shared.state);
            let granted = match state_guard.trust.get(&peer_key) {
                Some(device) => state_guard
                    .trust
                    .granted(&device.endpoint_id, Grant::Transfer),
                None => false,
            };
            if !granted || super::service_jobs::check_owner(&state_guard, record.owner).is_err() {
                if record.state != JobState::Cancelled {
                    record.enter(JobState::Interrupted);
                    let reason = if granted { OWNER_GONE } else { PERMISSION_OFF };
                    record.error = Some(reason.to_owned());
                    record.waiting_file = None;
                }
                drop(state_guard);
                drop(jobs);
                crate::ignored!(
                    crate::trace::FILES,
                    "a staged file left behind is swept with its job's staging",
                    std::fs::remove_file(staged)
                );
                save_jobs(&self.shared)?;
                return Ok(None);
            }
            // Still active with both locks held: commit without releasing.
            // `state_guard` fences revoke's grant update; `jobs` fences
            // cancellation/interruption until after `renameat`.
            match policy {
                manifest::PublishPolicy::Replace => {
                    // Recheck conflict immediately before commit (handoff): an
                    // absent destination uses no-replace so a newly appearing
                    // file returns to conflict instead of silent overwrite;
                    // explicit Replace (stored choice or apply-remaining) may
                    // overwrite an existing destination.
                    let dest_exists =
                        vnilink_rename::anchored::stat_at(dir_fd.as_fd(), file_name).is_ok();
                    let at = record.file_index;
                    let explicit_replace = record.choices.get(&at).copied()
                        == Some(ConflictChoice::Replace)
                        || record.apply_remaining == Some(ConflictChoice::Replace);
                    if dest_exists && !explicit_replace {
                        record.choices.remove(&at);
                        record.enter(JobState::WaitingDecision);
                        record.waiting_file = Some(at);
                        drop(state_guard);
                        drop(jobs);
                        save_jobs(&self.shared)?;
                        return Ok(None);
                    }
                    // Absent (auto or explicit with deleted dest) -> no-replace
                    // (detects race via Exists); present + explicit -> overwrite.
                    let allow_overwrite = dest_exists && explicit_replace;
                    match commit_file(file_name, allow_overwrite) {
                        Ok(consumed_sibling) => {
                            if consumed_sibling {
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                            }
                            Ok(Some(file_name.to_owned()))
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            // Lost auto-Replace race (dest appeared): conflict.
                            record.choices.remove(&record.file_index);
                            record.enter(JobState::WaitingDecision);
                            record.waiting_file = Some(record.file_index);
                            drop(state_guard);
                            drop(jobs);
                            save_jobs(&self.shared)?;
                            return Ok(None);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                            // Remount raced the pre-stage check: stand down
                            // retryably; the next poll re-detects volumes.
                            if record.state != JobState::Cancelled {
                                record.enter(JobState::Interrupted);
                                record.error = Some(VOLUME_CHANGED.to_owned());
                                record.waiting_file = None;
                            }
                            drop(state_guard);
                            drop(jobs);
                            crate::ignored!(
                                crate::trace::FILES,
                                "a staged file left behind is swept with its job's staging",
                                std::fs::remove_file(staged)
                            );
                            save_jobs(&self.shared)?;
                            return Ok(None);
                        }
                        Err(_) => {
                            drop(state_guard);
                            drop(jobs);
                            crate::ignored!(
                                crate::trace::FILES,
                                "a staged file left behind is swept with its job's staging",
                                std::fs::remove_file(staged)
                            );
                            self.mark_failed(id, PUBLISH_FAILED)?;
                            return Ok(None);
                        }
                    }
                }
                manifest::PublishPolicy::KeepBoth => {
                    let free = free_name
                        .clone()
                        .ok_or(Error::InvalidSetting(PUBLISH_FAILED))?;
                    if free == file_name {
                        match commit_file(file_name, false) {
                            Ok(consumed_sibling) => {
                                if consumed_sibling {
                                    crate::ignored!(
                                        crate::trace::FILES,
                                        "a staged file left behind is swept with its job's staging",
                                        std::fs::remove_file(staged)
                                    );
                                }
                                Ok(Some(file_name.to_owned()))
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                // Lost race: re-enter conflict.
                                record.choices.remove(&record.file_index);
                                record.enter(JobState::WaitingDecision);
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                                if record.state != JobState::Cancelled {
                                    record.enter(JobState::Interrupted);
                                    record.error = Some(VOLUME_CHANGED.to_owned());
                                    record.waiting_file = None;
                                }
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                return Ok(None);
                            }
                        }
                    } else {
                        match commit_file(&free, false) {
                            Ok(consumed_sibling) => {
                                if consumed_sibling {
                                    crate::ignored!(
                                        crate::trace::FILES,
                                        "a staged file left behind is swept with its job's staging",
                                        std::fs::remove_file(staged)
                                    );
                                }
                                Ok(Some(free))
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                                record.choices.remove(&record.file_index);
                                record.enter(JobState::WaitingDecision);
                                record.waiting_file = Some(record.file_index);
                                drop(state_guard);
                                drop(jobs);
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                                if record.state != JobState::Cancelled {
                                    record.enter(JobState::Interrupted);
                                    record.error = Some(VOLUME_CHANGED.to_owned());
                                    record.waiting_file = None;
                                }
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                save_jobs(&self.shared)?;
                                return Ok(None);
                            }
                            Err(_) => {
                                drop(state_guard);
                                drop(jobs);
                                crate::ignored!(
                                    crate::trace::FILES,
                                    "a staged file left behind is swept with its job's staging",
                                    std::fs::remove_file(staged)
                                );
                                self.mark_failed(id, PUBLISH_FAILED)?;
                                return Ok(None);
                            }
                        }
                    }
                }
            }
        })();
        // `commit` only returns `Ok` (all io errors handled above as failed/
        // conflict); any crate `Err` (unknown job, etc.) propagates.
        commit
    }

    fn finalize_receiver(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Verify every manifest path landed before claiming completion.
        // Skipped files intentionally keep the destination as it was.
        let (local_root, local_rel, manifest, choices, apply_remaining) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.local_root,
                record.local_rel.clone(),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                record.choices.clone(),
                record.apply_remaining,
            )
        };
        let landed = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.landed.clone()
        };
        for (index, entry) in manifest
            .entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .enumerate()
        {
            // Every file before the end either landed or was skipped: by
            // its own answer, or by one given for the rest.
            let skipped = choices.get(&index).copied().or(apply_remaining);
            let name = match landed.get(&index) {
                None if skipped == Some(ConflictChoice::Skip) => continue,
                None => return Err(Error::InvalidSetting(NO_JOB)),
                Some(_) if choices.get(&index).copied() == Some(ConflictChoice::Skip) => continue,
                Some(name) => name,
            };
            let dest = self.receiver_dest(id, local_root, &local_rel, &entry.path)?;
            let dest = dest
                .parent()
                .ok_or(Error::InvalidSetting(VERIFY_FAILED))?
                .join(name);
            let size = std::fs::metadata(&dest)
                .map(|meta| meta.len())
                .unwrap_or(u64::MAX);
            if size != entry.size {
                return self.mark_failed(id, VERIFY_FAILED);
            }
        }
        remove_staging(&self.shared, id);
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if matches!(record.state, JobState::Cancelled | JobState::Interrupted) {
                return Ok(record.snapshot());
            }
            record.enter(JobState::Complete);
            record.error = None;
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Schedules another try of a step that failed for `reason`, a failure
    /// that may clear on its own, keeping the job's state. The job shows
    /// `reason` meanwhile. Once [`MAX_RETRIES`] tries or [`RETRY_WINDOW`]
    /// pass without the job moving a byte, it is interrupted for `reason`.
    fn retry_later(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        let give_up = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            if record.state.is_terminal() || record.state == JobState::Interrupted {
                return Ok(record.snapshot());
            }
            let now = std::time::Instant::now();
            let done = record.done();
            let fresh = Retry {
                attempts: 0,
                first_at: now,
                next_at: now,
                done_at: done,
            };
            let retry = record.retry.get_or_insert_with(|| fresh.clone());
            // Bytes moved since: this is a new run of failures.
            if done > retry.done_at {
                *retry = fresh;
            }
            if retry.attempts >= MAX_RETRIES || now.duration_since(retry.first_at) >= RETRY_WINDOW {
                record.retry = None;
                true
            } else {
                retry.attempts += 1;
                retry.next_at = now + retry_pause(retry.attempts);
                self.shared.transfer_counters.retried();
                record.error = Some(reason.to_owned());
                false
            }
        };
        if give_up {
            return self.mark_interrupted(id, reason);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Ends a run of retries after a step that worked.
    fn retry_settled(record: &mut JobRecord) {
        if record.retry.take().is_some() {
            record.error = None;
        }
    }

    /// Stops a job for `reason`, which it shows until it resumes. A
    /// cancelled job stays cancelled.
    fn mark_interrupted(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.retry = None;
            if record.state != JobState::Cancelled {
                record.enter(JobState::Interrupted);
                record.error = Some(reason.to_owned());
            }
        }
        stop_prefetch(&self.shared, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// Receiver-side authorization at publication time: the peer must still
    /// be paired with the transfer grant. Revocation stops publication even
    /// when bytes already arrived.
    fn receiver_granted(shared: &Shared, peer: &[u8; 32]) -> bool {
        let state = lock(&shared.state);
        match state.trust.get(peer) {
            Some(device) => state.trust.granted(&device.endpoint_id, Grant::Transfer),
            None => false,
        }
    }

    fn mark_failed(&self, id: u64, reason: &'static str) -> Result<JobSnapshot, Error> {
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            record.enter(JobState::Failed);
            record.error = Some(reason.to_owned());
        }
        // Failed jobs do not resume: their partial only takes disk.
        remove_staging(&self.shared, id);
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    fn resume_sender(&self, id: u64) -> Result<JobSnapshot, Error> {
        // Re-verify sources after a restart. A changed source fails the
        // job instead of moving new bytes.
        // Adopt the leg's pin first (jobs->roots order); the pin is
        // leg-wide, so an upfront mismatch is definitive and fails with
        // the rebound cause instead of erroring the resume call.
        let rebound = {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            verify_leg_pin(
                &lock(&self.shared.file_roots),
                record.local_root,
                &mut record.local_pin,
            )
            .is_err()
        };
        if rebound {
            return self.mark_failed(id, fs::ROOTS_REBOUND);
        }
        let (local_root, local_rel, local_pin, manifest, source_ids) = {
            let jobs = lock(&self.shared.jobs);
            let record = jobs.get(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            (
                record.local_root,
                record.local_rel.clone(),
                record.local_pin.clone().expect("leg pin just verified"),
                record
                    .manifest
                    .clone()
                    .ok_or(Error::InvalidSetting(NO_JOB))?,
                record.source_ids.clone(),
            )
        };
        if let Err(reason) = check_sending_sources(
            &self.shared,
            local_root,
            &local_rel,
            &local_pin,
            &manifest,
            &source_ids,
        ) {
            return self.mark_failed(id, reason);
        }
        {
            let mut jobs = lock(&self.shared.jobs);
            let record = jobs.get_mut(&id).ok_or(Error::InvalidSetting(NO_JOB))?;
            let state = lock(&self.shared.state);
            super::service_jobs::check_owner(&state, record.owner)?;
            record.enter(JobState::Serving);
            record.error = None;
            drop(state);
            drop(jobs);
        }
        save_jobs(&self.shared)?;
        self.job_snapshot(id)
    }

    /// The wire generation of the session a job with `peer` and `owner`
    /// rides on this side ([`super::service_jobs::job_session`]).
    fn generation_for(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        owner: Option<super::service_jobs::JobOwner>,
    ) -> Result<u64, Error> {
        let state = self.alive_state()?;
        let session = super::service_jobs::job_session(&state, peer, outgoing, owner)
            .ok_or(Error::NoSession)?;
        if session.phase != crate::session::SessionPhase::Active {
            return Err(Error::NoSession);
        }
        Ok(session.wire_stamp.generation)
    }

    /// One job-control round trip on the session the job rides.
    pub(super) fn job_round_trip_side(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        owner: Option<super::service_jobs::JobOwner>,
        tag: u8,
        payload: &[u8],
    ) -> Result<Vec<u8>, Error> {
        self.job_round_trip_side_owned(peer, outgoing, tag, payload, owner, false)
    }

    /// As [`Core::job_round_trip_side`]; `leased` also holds the owning
    /// service's lease across it, bounded in time, as a submission does.
    pub(super) fn job_round_trip_side_owned(
        &self,
        peer: &[u8; 32],
        outgoing: bool,
        tag: u8,
        payload: &[u8],
        owner: Option<super::service_jobs::JobOwner>,
        leased: bool,
    ) -> Result<Vec<u8>, Error> {
        let mut lease = owner
            .filter(|_| leased)
            .map(|owner| self.service_lease(owner.id, owner.generation))
            .transpose()?;
        // Senders poll over the session facing the peer: outgoing for
        // controller senders, incoming for host senders, the one the job
        // rides each time.
        let connection = {
            let state = self.alive_state()?;
            let session = super::service_jobs::job_session(&state, peer, outgoing, owner)
                .ok_or(Error::NoSession)?;
            if session.phase != crate::session::SessionPhase::Active {
                return Err(Error::NoSession);
            }
            session.link.clone().ok_or(Error::NoSession)?
        };
        let mut frame = Vec::with_capacity(1 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(payload);
        self.runtime.block_on(async {
            let work = async {
                let (mut send, mut recv) =
                    connection.open_bi().await.map_err(Error::network_debug)?;
                crate::sidestream::write_frame(
                    &mut send,
                    &frame,
                    |bytes: &Vec<u8>| {
                        let mut out = Vec::with_capacity(4 + bytes.len());
                        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                        out.extend_from_slice(bytes);
                        Ok(out)
                    },
                    "the job frame was not written",
                )
                .await?;
                let reply: Vec<u8> = crate::sidestream::read_frame(
                    &mut recv,
                    |len| {
                        if len > job::MAX_JOB_FRAME {
                            Err(Error::Malformed)
                        } else {
                            Ok(())
                        }
                    },
                    |bytes| Ok(bytes.to_vec()),
                )
                .await?;
                Ok(reply)
            };
            if let Some(lease) = lease.as_mut() {
                lease
                    .run(async {
                        tokio::time::timeout(Duration::from_secs(60), work)
                            .await
                            .map_err(|_| Error::Timeout)?
                    })
                    .await
            } else {
                work.await
            }
        })
    }
}

fn truncate_error(text: &str) -> String {
    const MAX: usize = 256;
    if text.len() <= MAX {
        return text.to_owned();
    }
    text[..MAX].to_owned()
}

/// Largest concurrent bulk jobs (Serving/Transferring/WaitingDecision).
/// Bounds staging disk (manifest-capped per job), memory and bulk traffic
/// alongside desktop video/input. Paused (Interrupted) and terminal jobs do
/// not count; explicit resume re-checks capacity.
pub const MAX_ACTIVE_JOBS: usize = 8;

pub(super) const ACTIVE_STATES: &[JobState] = &[
    JobState::Preparing,
    JobState::Serving,
    JobState::Transferring,
    JobState::WaitingDecision,
];

/// Retention: partials live under the transfer directory per job
/// (`staging/job-{id}`), removed on Complete (verified publish), Failed and
/// forget; retained on Cancelled/Interrupted for byte-reusing resume until
/// the resume policy evicts them at the next load.
/// Manifests bound each job's file count, free-space admission bounds its
/// bytes and `MAX_ACTIVE_JOBS` bounds concurrency, so disk stays
/// proportional to in-flight plus resumable work, not unbounded history.

/// Resumable payload cache: seven days, at most 4 GiB across inactive jobs.
/// Active work remains protected by the manifest and concurrency limits.
fn retention_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn allocate_job_id(shared: &Shared) -> Result<u64, Error> {
    shared
        .job_next
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map_err(|_| Error::Busy)
}

/// Re-checks a sending leg's files against its manifest through the
/// approved root's handles, never retained absolute paths: each must still
/// be the regular file scanned, with the manifest's size and modification
/// time.
/// Returns the reason to fail the job with.
fn check_sending_sources(
    shared: &Shared,
    local_root: u64,
    local_rel: &str,
    pin: &fs::PinnedRoot,
    manifest: &Manifest,
    source_ids: &[u64],
) -> Result<(), &'static str> {
    let files = manifest.entries.iter().filter(|entry| !entry.is_dir);
    for (index, entry) in files.enumerate() {
        let scanned_id = source_ids.get(index).copied().unwrap_or(0);
        // The manifest entry already carries its chosen name, so it lies
        // under the source folder.
        let root_rel = join_rel(local_rel, &entry.path);
        let (parent_rel, name) = root_rel.rsplit_once('/').unwrap_or(("", &root_rel));
        // The roots guard ends before the caller records a failure:
        // roots->jobs would invert the publication order.
        let identity = {
            let roots = lock(&shared.file_roots);
            roots.verify_pin(pin).map_err(|_| fs::ROOTS_REBOUND)?;
            source_identity(&roots, local_root, parent_rel, name)
        };
        if !identity.is_some_and(|identity| entry_matches(entry, scanned_id, identity)) {
            return Err(SOURCE_CHANGED);
        }
    }
    Ok(())
}

/// Each file's scanned id in `manifest` file order, from ids by the
/// source's root-relative path; zero where none was scanned.
fn source_ids(manifest: &Manifest, local_rel: &str, scanned: &HashMap<String, u64>) -> Vec<u64> {
    manifest
        .entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(|entry| {
            scanned
                .get(&join_rel(local_rel, &entry.path))
                .copied()
                .unwrap_or(0)
        })
        .collect()
}

/// What identifies a source between its scan and its reads: its id on
/// the volume, size and modification time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SourceIdentity {
    pub(super) id: u64,
    pub(super) size: u64,
    pub(super) mtime_ms: u64,
}

/// Whether a source is still the one scanned for `entry`: the same file,
/// with the manifest's size and modification time. Unknown ids and times
/// (zero) are not compared.
fn entry_matches(entry: &ManifestEntry, scanned_id: u64, found: SourceIdentity) -> bool {
    (scanned_id == 0 || found.id == scanned_id)
        && found.size == entry.size
        && (entry.source_mtime_ms == 0 || found.mtime_ms == entry.source_mtime_ms)
}

/// The identity of a regular file, not a link, under `root`.
fn source_identity(
    roots: &fs::RootSet,
    root: u64,
    parent_rel: &str,
    name: &str,
) -> Option<SourceIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsFd;
        let parent = roots.open_dir_fd(root, parent_rel).ok()?;
        let st = vnilink_rename::anchored::stat_at(parent.as_fd(), name).ok()?;
        (st.is_file && !st.is_symlink).then_some(SourceIdentity {
            id: st.ino,
            size: st.size,
            mtime_ms: st.mtime_ms,
        })
    }
    #[cfg(windows)]
    {
        let (source, _) =
            super::transfer::open_verified_source(roots, root, parent_rel, name).ok()?;
        Some(SourceIdentity {
            id: source.identity().1,
            size: source.size(),
            mtime_ms: source.mtime_ms(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (roots, root, parent_rel, name);
        None
    }
}

pub(super) fn wire_job_id(record: &JobRecord) -> u64 {
    if record.wire_id == 0 {
        record.id
    } else {
        record.wire_id
    }
}
fn wire_job<'a>(
    jobs: &'a HashMap<u64, JobRecord>,
    peer: &[u8; 32],
    id: u64,
) -> Option<&'a JobRecord> {
    jobs.values()
        .find(|record| record.peer_key == *peer && wire_job_id(record) == id)
}
pub(super) fn wire_job_mut<'a>(
    jobs: &'a mut HashMap<u64, JobRecord>,
    peer: &[u8; 32],
    id: u64,
) -> Option<&'a mut JobRecord> {
    jobs.values_mut()
        .find(|record| record.peer_key == *peer && wire_job_id(record) == id)
}

/// Neutral links accept lifecycle messages only for live service-owned jobs.
/// Every control request shares the status request's identity fields.
pub(super) fn neutral_job_control_allowed(
    shared: &Shared,
    peer: &[u8; 32],
    tag: u8,
    payload: &[u8],
) -> bool {
    if !matches!(
        tag,
        job::TAG_JOB_STATUS
            | job::TAG_JOB_CANCEL
            | job::TAG_JOB_RESOLVE
            | job::TAG_JOB_RESUME
            | job::TAG_JOB_MANIFEST
    ) {
        return false;
    }
    let Ok(query) = job::decode::<job::JobStatus>(payload) else {
        return false;
    };
    let jobs = lock(&shared.jobs);
    wire_job(&jobs, peer, query.job_id).is_some_and(|record| {
        record.owner.is_some()
            && super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_ok()
    })
}

pub(super) fn retained_job_ids(
    jobs: &HashMap<u64, JobRecord>,
    policy: crate::job::FileRetentionPolicy,
    now: u64,
) -> std::collections::HashSet<u64> {
    let mut live = std::collections::HashSet::new();
    let mut records: Vec<_> = jobs.values().collect();
    records.sort_by_key(|record| std::cmp::Reverse((record.created_at, record.id)));
    let mut budget = policy.resume_cache_bytes;
    for record in records {
        let keep = match record.state {
            JobState::Complete | JobState::Failed => false,
            JobState::Interrupted | JobState::Cancelled => {
                let size = record
                    .manifest
                    .as_ref()
                    .map(Manifest::total_bytes)
                    .unwrap_or(0);
                if policy.resume_cache_bytes == 0
                    || policy.resume_cache_age_secs == 0
                    || now.saturating_sub(record.created_at) >= policy.resume_cache_age_secs
                    || size > budget
                {
                    false
                } else {
                    budget -= size;
                    true
                }
            }
            _ => true,
        };
        if keep {
            live.insert(record.id);
        }
    }
    live
}

/// Whether `record` is a sender job that may serve `peer` now.
fn serving(record: &JobRecord, peer: &[u8; 32]) -> bool {
    record.peer_key == *peer
        && record.sender
        && matches!(
            record.state,
            JobState::Serving | JobState::Transferring | JobState::WaitingDecision
        )
}

/// The peer's sending job with wire id `job` while it may serve, under a
/// live owner.
fn served_job<'a>(
    jobs: &'a HashMap<u64, JobRecord>,
    state: &State,
    peer: &[u8; 32],
    job: u64,
) -> Option<&'a JobRecord> {
    serving_job(jobs, peer, job)
        .filter(|record| super::service_jobs::check_owner(state, record.owner).is_ok())
}

/// The peer's sending job with wire id `job` while its state serves.
/// Receiver records sharing the wire id never match.
fn serving_job<'a>(
    jobs: &'a HashMap<u64, JobRecord>,
    peer: &[u8; 32],
    job: u64,
) -> Option<&'a JobRecord> {
    jobs.values()
        .find(|record| wire_job_id(record) == job && serving(record, peer))
}

/// True while `peer` holds the Transfer grant and its sending job `job`
/// may still serve.
pub(super) fn still_serving(shared: &Shared, peer: &[u8; 32], job: u64) -> bool {
    let jobs = lock(&shared.jobs);
    let state = lock(&shared.state);
    state.trust.granted(peer, Grant::Transfer) && served_job(&jobs, &state, peer, job).is_some()
}

/// Bytes a sending stream advanced at least before its job hears again,
/// so progress moves smoothly without taking the jobs lock per chunk.
const SENT_EVERY: u64 = 1024 * 1024;

/// Records that the peer's sending job `job` has sent file `file` up to
/// `position`, telling watchers once it moved [`SENT_EVERY`] or finished.
pub(super) fn sending_at(
    shared: &Shared,
    peer: &[u8; 32],
    job: u64,
    file: u32,
    position: u64,
    finished: bool,
) {
    {
        let mut jobs = lock(&shared.jobs);
        let Some(record) = jobs
            .values_mut()
            .find(|record| wire_job_id(record) == job && serving(record, peer))
        else {
            return;
        };
        let file = file as usize;
        let moved = match record.sent_at {
            Some((at, before)) if at == file => position.saturating_sub(before),
            _ => u64::MAX,
        };
        if moved < SENT_EVERY && !finished {
            return;
        }
        record.sent_at = Some((file, position));
    }
    jobs_changed(shared);
}

/// Test probe parked by the rebind regression: called with the serving
/// engine ([`Core::engine_key`]) and job after the source opens, before it
/// is authorized. Production leaves it unset. The probe must return
/// promptly for jobs it does not recognize: suites run in parallel.
pub static SERVE_SOURCE_PROBE: std::sync::Mutex<Option<fn(usize, u64)>> =
    std::sync::Mutex::new(None);

/// Opens file `file` of the peer's sending job `job` for serving, or says
/// which status refuses it. The source is reached through the leg's pinned
/// root by handles, never a retained absolute path, and must still be the
/// regular file scanned for the manifest: a file renamed over it, even
/// with the same size and time, is another file. A rebound root refuses the request
/// instead of serving moved bytes, while a file already open completes.
pub(super) fn open_served_file(
    shared: &Shared,
    peer: &[u8; 32],
    job: u64,
    file: u32,
) -> Result<crate::transfer::Source, u8> {
    use crate::transfer::{STATUS_CHANGED, STATUS_DENIED};
    let (id, local_root, local_rel, entry, scanned_id, mut pin) = {
        let jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        if !state.trust.granted(peer, Grant::Transfer) {
            return Err(STATUS_DENIED);
        }
        let record = served_job(&jobs, &state, peer, job).ok_or(STATUS_DENIED)?;
        let entry = (*record.files().get(file as usize).ok_or(STATUS_DENIED)?).clone();
        (
            record.id,
            record.local_root,
            record.local_rel.clone(),
            entry,
            record.source_ids.get(file as usize).copied().unwrap_or(0),
            record.local_pin.clone(),
        )
    };
    let root_rel = join_rel(&local_rel, &entry.path);
    let (parent_rel, name) = root_rel.rsplit_once('/').unwrap_or(("", &root_rel));
    let adopted = pin.is_none();
    // The opens run under one short roots hold, so the pin they verify is
    // the root they walk.
    #[cfg(unix)]
    let (opened, identity) = {
        use std::os::unix::io::AsFd;
        let roots = lock(&shared.file_roots);
        verify_leg_pin(&roots, local_root, &mut pin).map_err(|_| STATUS_DENIED)?;
        let parent = roots
            .open_dir_fd(local_root, parent_rel)
            .map_err(|_| STATUS_CHANGED)?;
        let st =
            vnilink_rename::anchored::stat_at(parent.as_fd(), name).map_err(|_| STATUS_CHANGED)?;
        if !st.is_file || st.is_symlink {
            return Err(STATUS_CHANGED);
        }
        let fd = vnilink_rename::anchored::open_file_verified(parent.as_fd(), name, st.dev, st.ino)
            .map_err(|_| STATUS_CHANGED)?;
        let identity = SourceIdentity {
            id: st.ino,
            size: st.size,
            mtime_ms: st.mtime_ms,
        };
        (std::fs::File::from(fd), identity)
    };
    #[cfg(windows)]
    let (opened, identity) = {
        let roots = lock(&shared.file_roots);
        verify_leg_pin(&roots, local_root, &mut pin).map_err(|_| STATUS_DENIED)?;
        let (source, _) =
            super::transfer::open_verified_source(&roots, local_root, parent_rel, name)
                .map_err(|_| STATUS_CHANGED)?;
        let identity = SourceIdentity {
            id: source.identity().1,
            size: source.size(),
            mtime_ms: source.mtime_ms(),
        };
        (source, identity)
    };
    if !entry_matches(&entry, scanned_id, identity) {
        return Err(STATUS_CHANGED);
    }
    // Persist a legacy adopt so later requests are strict; the roots guard
    // above is already dropped (sequential, never nested).
    if adopted {
        if let Some(record) = lock(&shared.jobs).get_mut(&id) {
            record.local_pin = pin;
        }
    }
    // Copy the probe out: a parked probe must not hold the lock that
    // disarming it takes.
    let probe = *lock(&SERVE_SOURCE_PROBE);
    if let Some(probe) = probe {
        probe(shared as *const Shared as usize, job);
    }
    // The job may have stopped or the grant gone while the source opened.
    if !still_serving(shared, peer, job) {
        return Err(STATUS_DENIED);
    }
    #[cfg(unix)]
    let served = crate::transfer::Source {
        file: opened,
        size: entry.size,
        mtime_ms: entry.source_mtime_ms,
    };
    #[cfg(windows)]
    let served = crate::transfer::Source {
        file: opened.file().try_clone().map_err(|_| STATUS_CHANGED)?,
        size: entry.size,
        mtime_ms: entry.source_mtime_ms,
        _hold: opened,
    };
    Ok(served)
}

/// True when renaming or deleting `rel/name` under `root` would disturb an
/// active transfer: the entry is a source of a sender job or a destination
/// (base or manifest path) of a receiver job. Delete uses this same overlap
/// rule. Terminal jobs never block; anything resumable does, so publication
/// cannot strand or recreate the old path.
pub(super) fn rename_blocked(shared: &Shared, root: u64, rel: &str, name: &str) -> bool {
    let jobs = lock(&shared.jobs);
    rename_blocked_with(&jobs, root, rel, name)
}

pub(super) fn rename_blocked_with(
    jobs: &std::collections::HashMap<u64, JobRecord>,
    root: u64,
    rel: &str,
    name: &str,
) -> bool {
    let target = if rel.is_empty() {
        name.to_owned()
    } else {
        format!("{rel}/{name}")
    };
    for record in jobs.values() {
        if record.local_root != root {
            continue;
        }
        match record.state {
            JobState::Complete | JobState::Cancelled | JobState::Failed => continue,
            _ => {}
        }
        if record.sender {
            // Sender source: the chosen entries of `local_rel`, or with
            // none the whole root. Any overlap blocks.
            if record.names.is_empty() {
                return true;
            }
            if record
                .names
                .iter()
                .any(|name| paths_overlap(&join_rel(&record.local_rel, name), &target))
            {
                return true;
            }
        } else {
            // Receiver destination: base plus every manifest path. An empty
            // base with no manifest yet still blocks renames of the base
            // itself (checked via base overlap).
            if paths_overlap(&record.local_rel, &target) {
                return true;
            }
            if let Some(manifest) = &record.manifest {
                for entry in &manifest.entries {
                    let full = if record.local_rel.is_empty() {
                        entry.path.clone()
                    } else {
                        format!("{}/{}", record.local_rel, entry.path)
                    };
                    // `entry.path` for dirs is the dir itself; files give
                    // their full path. Either overlapping the rename target
                    // (the entry itself, a child, or an ancestor) blocks.
                    if paths_overlap(&full, &target) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn paths_overlap(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        // Empty names the root itself: any entry under that root overlaps.
        return true;
    }
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// Marks a peer's active jobs interrupted on revoke, unpair and grant
/// removal. Verified bytes stay in the store; resume continues them.
/// In-flight fetches are signalled through their cancel watches so active
/// work stops instead of publishing after revocation.
pub(super) fn interrupt_peer_jobs(shared: &Shared, peer: &[u8; 32], unpaired: bool) {
    let reason = if unpaired { UNPAIRED } else { PERMISSION_OFF };
    interrupt_jobs(shared, peer, false, reason);
}
/// Marks the jobs riding a controller session that ended interrupted.
pub(super) fn interrupt_controller_jobs(shared: &Shared, peer: &[u8; 32]) {
    interrupt_jobs(shared, peer, true, CONNECTION_ENDED);
}
fn interrupt_jobs(shared: &Shared, peer: &[u8; 32], controller_only: bool, reason: &'static str) {
    let ids: Vec<u64> = {
        let mut jobs = lock(&shared.jobs);
        let mut ids = Vec::new();
        for record in jobs.values_mut() {
            if record.peer_key == *peer && (!controller_only || record.outgoing) {
                match record.state {
                    JobState::Serving | JobState::Transferring | JobState::WaitingDecision => {
                        record.enter(JobState::Interrupted);
                        record.error = Some(reason.to_owned());
                        record.waiting_file = None;
                        ids.push(record.id);
                    }
                    _ => {}
                }
            }
        }
        ids
    };
    for id in ids {
        if let Some(stop) = lock(&shared.job_cancel).remove(&id) {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
        stop_prefetch(shared, id);
    }
    persist_jobs(shared);
}

/// Tells job watchers, and the background driver and workers, that
/// something changed.
pub(super) fn jobs_changed(shared: &Shared) {
    shared.job_revision.fetch_add(1, Ordering::AcqRel);
    let (gate, wake) = &shared.job_wake;
    // Under the gate, so a worker that just found nothing changed is
    // already waiting.
    let _gate = lock(gate);
    wake.notify_all();
}

/// How soon a sender asks its receiver again after an answer that changed
/// it, and the longest it waits while answers change nothing.
const SENDER_SOON: Duration = Duration::from_millis(100);
const SENDER_SELDOM: Duration = Duration::from_secs(1);
/// The longest a blocked receiver waits for a change before it looks again.
const RECEIVER_RETRY: Duration = Duration::from_millis(200);

/// One job's worker: takes its steps until it has none left, then lets
/// the driver give the job a worker again when it next has one.
fn work_job(weak: std::sync::Weak<Core>, shared: Arc<Shared>, id: u64) {
    struct Working<'a>(&'a Shared, u64);
    impl Drop for Working<'_> {
        fn drop(&mut self) {
            lock(&self.0.job_workers).remove(&self.1);
        }
    }
    let _working = Working(&shared, id);
    let mut pause = SENDER_SOON;
    loop {
        let seen = shared.job_revision.load(Ordering::Acquire);
        let Some(paced) = job_pace(&shared, id) else {
            return;
        };
        {
            let Some(core) = weak.upgrade() else {
                return;
            };
            if lock(&shared.state).shut_down {
                return;
            }
            if paced {
                let before = core.job_snapshot(id).ok();
                let after = core.job_poll(id).ok();
                pause = if after.is_some() && after != before {
                    SENDER_SOON
                } else {
                    (pause * 2).min(SENDER_SELDOM)
                };
            } else {
                core.drive_receiver(id);
            }
        }
        if paced {
            // Changes meanwhile do not hurry it: only one that leaves it
            // nothing to do ends the wait early.
            let due = std::time::Instant::now() + pause;
            let mut seen = shared.job_revision.load(Ordering::Acquire);
            loop {
                let left = due.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break;
                }
                seen = wait_for_job_change(&shared, seen, left);
                if job_pace(&shared, id).is_none() {
                    return;
                }
            }
        } else {
            wait_for_job_change(&shared, seen, RECEIVER_RETRY);
        }
    }
}

/// Whether job `id` has a step to take, and if so whether it is paced:
/// a sender asks its peer on a timer instead of at every change.
fn job_pace(shared: &Shared, id: u64) -> Option<bool> {
    lock(&shared.jobs)
        .get(&id)
        .filter(|record| record.drivable())
        .map(JobRecord::paced)
}

/// Waits until the job revision differs from `seen` or `timeout` passes,
/// and returns the revision then.
fn wait_for_job_change(shared: &Shared, seen: u64, timeout: Duration) -> u64 {
    let deadline = std::time::Instant::now() + timeout;
    let (gate, wake) = &shared.job_wake;
    let mut guard = lock(gate);
    loop {
        let now = shared.job_revision.load(Ordering::Acquire);
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if now != seen || left.is_zero() {
            return now;
        }
        guard = wake
            .wait_timeout(guard, left)
            .map(|(guard, _)| guard)
            .unwrap_or_else(|poison| poison.into_inner().0);
    }
}

/// Copies a file's arriving byte count onto its job, at most four times a
/// second, until the fetch drops its end of `arriving`.
fn follow_arrival(
    jobs: Arc<Mutex<HashMap<u64, JobRecord>>>,
    shared: Arc<Shared>,
    id: u64,
    arriving: std::sync::mpsc::Receiver<u64>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(crate::trace::within(shared.span.clone(), move || {
        const EVERY: std::time::Duration = std::time::Duration::from_millis(250);
        let mut shown = std::time::Instant::now() - EVERY;
        let mut latest = None;
        loop {
            let next = arriving.recv_timeout(EVERY);
            match next {
                Ok(bytes) => latest = Some(bytes),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if let Some(bytes) = latest.filter(|_| shown.elapsed() >= EVERY) {
                if let Some(record) = lock(&jobs).get_mut(&id) {
                    record.in_flight = bytes;
                }
                jobs_changed(&shared);
                shown = std::time::Instant::now();
                latest = None;
            }
        }
    }))
}

/// Removes the partial files of job `id`, stopping its fetches ahead.
fn remove_staging(shared: &Shared, id: u64) {
    stop_prefetch(shared, id);
    if let Some(dir) = lock(&shared.transfer_dir).clone() {
        crate::ignored!(
            crate::trace::FILES,
            "staging left behind is swept on the next start",
            std::fs::remove_dir_all(dir.join(STAGING_DIR).join(format!("job-{id}")))
        );
    }
}

/// Saves the jobs that changed and tells watchers: each change to a job
/// ends here. A save that changes nothing writes and tells nothing. A save
/// that fails is recorded: what it held may not survive a restart.
pub(super) fn save_jobs(shared: &Shared) -> Result<(), Error> {
    let saved = super::job_journal::save_jobs(shared);
    if let Err(error) = &saved {
        tracing::warn!(
            target: crate::trace::FILES,
            failure = error.name(),
            reason = error.reason(),
            "transfer records were not saved: a restart may lose their latest state"
        );
    }
    saved
}

/// [`save_jobs`] for callers that carry on either way; the failure is
/// recorded there.
pub(super) fn persist_jobs(shared: &Shared) {
    // `save_jobs` recorded any failure.
    if save_jobs(shared).is_err() {}
}

/// Records that the peer was not told of a change to a shared job: its
/// side keeps the job as it was until it next asks.
fn told_peer(change: &'static str, job: u64, told: Result<(), Error>) {
    if let Err(error) = told {
        tracing::info!(
            target: crate::trace::FILES,
            job,
            change,
            failure = error.name(),
            reason = error.reason(),
            "the peer was not told: its side keeps the job as it was until it next asks"
        );
    }
}

pub(super) fn load_jobs(shared: &Shared) {
    // Each record is written afresh at its next save.
    lock(&shared.job_journal).reset();
    let Some(dir) = lock(&shared.transfer_dir).clone() else {
        return;
    };
    let Some((mut records, clean)) = super::job_journal::read_journals(&dir) else {
        return;
    };
    let mut max = 0u64;
    {
        let mut jobs = lock(&shared.jobs);
        for mut record in records.drain(..) {
            if let Some(owner) = record.owner.as_mut() {
                owner.id = 0; // service ids are process-local; never rebind on restart
            }
            // Older controller download records used the wrong side flag.
            if !record.sender && record.direction == JobDirection::Download {
                record.outgoing = true;
            }
            // Anything in flight died with the process; a preparation has
            // nothing to resume from.
            match record.state {
                _ if record.preparing_here() && !record.state.is_terminal() => {
                    record.enter(JobState::Failed);
                    record.error = Some(PREPARE_STOPPED.to_owned());
                }
                JobState::Preparing | JobState::Serving | JobState::Transferring => {
                    record.enter(JobState::Interrupted);
                    record.error = Some(APP_CLOSED.to_owned());
                }
                _ => {}
            }
            max = max.max(record.id).max(wire_job_id(&record));
            jobs.insert(record.id, record);
        }
    }
    if clean {
        reconcile_staging(shared, &dir);
    }
    let next = shared.job_next.load(Ordering::Relaxed);
    if max.saturating_add(1) > next {
        shared
            .job_next
            .store(max.saturating_add(1), Ordering::Relaxed);
    }
}

/// Only reconcile after every journal was read. Missing or unreadable
/// journals retain partials for diagnosis rather than interpreting
/// corruption as permission to delete all previously owned data. Jobs
/// past the retention policy lose their partials. Only canonical managed
/// names are candidates; unexpected entries remain untouched.
fn reconcile_staging(shared: &Shared, dir: &std::path::Path) {
    let policy = *lock(&shared.file_retention_policy);
    let keep = retained_job_ids(&lock(&shared.jobs), policy, retention_now());
    #[cfg(unix)]
    reconcile_staging_at(dir, &keep);
    #[cfg(not(unix))]
    let _ = (dir, keep);
}
#[cfg(unix)]
fn reconcile_staging_at(dir: &std::path::Path, keep: &std::collections::HashSet<u64>) {
    use std::os::fd::AsFd;
    use vnilink_rename::anchored;
    let Ok(root) = anchored::open_root(dir) else {
        return;
    };
    if let Ok(staging) = anchored::open_child(root.as_fd(), STAGING_DIR) {
        if let Ok(entries) = anchored::list_names(staging.as_fd()) {
            for name in entries {
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name
                    .strip_prefix("job-")
                    .and_then(|n| n.parse::<u64>().ok())
                else {
                    continue;
                };
                if name != format!("job-{id}") || keep.contains(&id) {
                    continue;
                }
                crate::ignored!(
                    crate::trace::FILES,
                    "staging left behind is swept on the next start",
                    anchored::remove_tree_at(staging.as_fd(), name)
                );
            }
        }
    }
}

/// Host-side job request handler: upload offers, download requests and
/// status queries from the controller. Generation is fenced before any
/// grant check or disk touch.
pub(super) async fn handle_job_request(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    tag: u8,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    // Every job message carries its generation inside the JSON; fence it
    // against the live incoming session before parsing further. The id
    // rides along so even the stale answer correlates.
    let (generation, request_id): (u64, u32) = serde_json::from_slice::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| {
            Some((
                value.get("generation")?.as_u64()?,
                value.get("id")?.as_u64()? as u32,
            ))
        })
        .ok_or(())?;
    let live = {
        let state = lock(&shared.state);
        matches!(state.incoming.get(&session_id), Some(session)
            if session.generation == generation
                && session.phase == crate::session::SessionPhase::Active
                && session.peer_key == *peer)
    };
    if !live {
        let denied = job::JobStateReply {
            v: job::JOB_VERSION,
            id: request_id,
            status: job::STATUS_STALE,
            state: JobState::Interrupted.name().to_owned(),
            error: None,
            waiting_file: None,
            done_bytes: 0,
            files_done: 0,
            skipped_files: 0,
        };
        reply(send, job::encode(&denied).map_err(|_| ())?).await?;
        return Ok(());
    }
    match tag {
        t if t == job::TAG_JOB_UPLOAD => {
            handle_upload_offer(shared, peer, send, payload, None).await
        }
        t if t == job::TAG_JOB_DOWNLOAD => {
            handle_download_request(shared, session_id, peer, send, payload, None).await
        }
        t if t == job::TAG_JOB_STATUS => {
            handle_status_query(shared, peer, true, send, payload).await
        }
        t if t == job::TAG_JOB_MANIFEST => handle_manifest_query(shared, peer, send, payload).await,
        t if t == job::TAG_JOB_CANCEL => handle_cancel(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESOLVE => handle_resolve(shared, peer, true, send, payload).await,
        t if t == job::TAG_JOB_RESUME => handle_resume(shared, peer, true, send, payload).await,
        _ => Err(()),
    }
}

/// Controller-side job handler: status queries from the host and symmetric
/// cancellation/decisions/resume from either sender or receiver arrive here.
pub(super) async fn handle_controller_job(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    tag: u8,
) -> Result<(), ()> {
    if tag == job::TAG_JOB_STATUS {
        return handle_status_query(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_CANCEL {
        return handle_cancel(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_RESOLVE {
        return handle_resolve(shared, peer, false, send, payload).await;
    }
    if tag == job::TAG_JOB_RESUME {
        return handle_resume(shared, peer, false, send, payload).await;
    }
    Err(())
}

/// Whether `generation` names a live session with `peer`: one it hosts on
/// the incoming side, else one this side controls. A peer may hold both a
/// control session and a services-only link; the message's generation
/// says which it came over.
fn peer_session_live(state: &State, peer: &[u8; 32], incoming_side: bool, generation: u64) -> bool {
    let sessions = if incoming_side {
        &state.incoming
    } else {
        &state.outgoing
    };
    sessions.values().any(|session| {
        session.peer_key == *peer
            && session.wire_stamp.generation == generation
            && session.phase == crate::session::SessionPhase::Active
    })
}

/// The host answers a controller asking for a prepared download's
/// manifest: still preparing (no manifest), the manifest, or a refusal
/// once preparation failed or the job ended.
async fn handle_manifest_query(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    let query: job::JobStatus = job::decode(payload).map_err(|_| ())?;
    job::check_version(query.v).map_err(|_| ())?;
    let answer = |status: u8, manifest: Option<Manifest>| job::ManifestReply {
        v: job::JOB_VERSION,
        id: query.id,
        status,
        job_id: query.job_id,
        manifest,
    };
    let reply = if !peer_session_live(&lock(&shared.state), peer, true, query.generation) {
        answer(job::STATUS_STALE, None)
    } else {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, query.job_id).filter(|record| record.sender) {
            Some(record) if record.state == JobState::Preparing => answer(job::STATUS_OK, None),
            Some(record)
                if matches!(
                    record.state,
                    JobState::Serving | JobState::Transferring | JobState::WaitingDecision
                ) =>
            {
                answer(job::STATUS_OK, record.manifest.clone())
            }
            _ => answer(job::STATUS_ERROR, None),
        }
    };
    let bytes = job::encode(&reply).map_err(|_| ())?;
    let mut out = Vec::with_capacity(4 + bytes.len());
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&bytes);
    crate::sidestream::write_frame(
        send,
        &out,
        |framed: &Vec<u8>| Ok(framed.clone()),
        "the job frame was not written",
    )
    .await
    .map_err(|_| ())
}

/// Symmetric cancellation: either side tells the peer one job is cancelled.
/// Generation is fenced like status queries; the peer marks its record
/// cancelled and signals its in-flight fetch so publication stops.
async fn handle_cancel(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let cancel: job::JobCancel = job::decode(payload).map_err(|_| ())?;
    job::check_version(cancel.v).map_err(|_| ())?;
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, cancel.generation);
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: cancel.id,
            status: job::STATUS_STALE,
            job_id: cancel.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let id = {
        let mut jobs = lock(&shared.jobs);
        match wire_job_mut(&mut jobs, peer, cancel.job_id) {
            Some(record) if record.peer_key == *peer && !record.state.is_terminal() => {
                record.enter(JobState::Cancelled);
                record.error = None;
                record.waiting_file = None;
                Some(record.id)
            }
            _ => None,
        }
    };
    if let Some(id) = id {
        if let Some(stop) = lock(&shared.job_cancel).remove(&id) {
            crate::ignored!(
                crate::trace::FILES,
                "a fetch that already ended needs no stop",
                stop.send(true)
            );
        }
        stop_prefetch(shared, id);
        persist_jobs(shared);
    }
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: cancel.id,
            status: job::STATUS_OK,
            job_id: cancel.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

async fn handle_status_query(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let query: job::JobStatus = job::decode(payload).map_err(|_| ())?;
    job::check_version(query.v).map_err(|_| ())?;
    // The querying peer must own the other end of the live session.
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, query.generation);
    if !live {
        let stale = job::JobStateReply {
            v: job::JOB_VERSION,
            id: query.id,
            status: job::STATUS_STALE,
            state: JobState::Interrupted.name().to_owned(),
            error: None,
            waiting_file: None,
            done_bytes: 0,
            files_done: 0,
            skipped_files: 0,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let answer = {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, query.job_id) {
            Some(record) if record.peer_key == *peer => {
                let waiting_file = if record.state == JobState::WaitingDecision {
                    record.waiting_file.and_then(|index| {
                        record
                            .manifest
                            .as_ref()?
                            .entries
                            .iter()
                            .filter(|entry| !entry.is_dir)
                            .nth(index)
                            .map(|entry| entry.path.clone())
                    })
                } else {
                    None
                };
                job::JobStateReply {
                    v: job::JOB_VERSION,
                    id: query.id,
                    status: job::STATUS_OK,
                    state: record.state.name().to_owned(),
                    error: record.error.clone(),
                    waiting_file,
                    done_bytes: record.done(),
                    files_done: record.files_done,
                    skipped_files: record.skipped_files,
                }
            }
            _ => job::JobStateReply {
                v: job::JOB_VERSION,
                id: query.id,
                status: job::STATUS_ERROR,
                state: JobState::Interrupted.name().to_owned(),
                error: None,
                waiting_file: None,
                done_bytes: 0,
                files_done: 0,
                skipped_files: 0,
            },
        }
    };
    reply(send, job::encode(&answer).map_err(|_| ())?).await?;
    Ok(())
}

/// Authenticated conflict decision: the sender forwards the owner's choice
/// to the receiver that owns the waiting file. Generation fenced; stale
/// dialogs (wrong job, wrong file, or no longer waiting) are refused without
/// touching state.
async fn handle_resolve(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let resolve: job::JobResolve = job::decode(payload).map_err(|_| ())?;
    job::check_version(resolve.v).map_err(|_| ())?;
    let live = peer_session_live(
        &lock(&shared.state),
        peer,
        incoming_side,
        resolve.generation,
    );
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: resolve.id,
            status: job::STATUS_STALE,
            job_id: resolve.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    let choice = match resolve.choice.as_str() {
        "skip" => ConflictChoice::Skip,
        "replace" => ConflictChoice::Replace,
        "keep-both" => ConflictChoice::KeepBoth,
        _ => {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resolve.id,
                status: job::STATUS_ERROR,
                job_id: resolve.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
    };
    let status = {
        let mut jobs = lock(&shared.jobs);
        match wire_job_mut(&mut jobs, peer, resolve.job_id) {
            Some(record)
                if record.peer_key == *peer
                    && !record.sender
                    && record.state == JobState::WaitingDecision =>
            {
                // Stale-dialog fence: the waiting file must still name the
                // same manifest path; choices apply only to their job/file.
                let expected = record.waiting_file.and_then(|index| {
                    record
                        .manifest
                        .as_ref()?
                        .entries
                        .iter()
                        .filter(|entry| !entry.is_dir)
                        .nth(index)
                        .map(|entry| entry.path.clone())
                });
                // The sender forwards the owner's decision without naming a
                // file (it mirrors state); the receiver applies to its
                // current waiting file. If the protocol later names files,
                // fence here. For now, apply to the current waiting file.
                let _ = expected;
                let file = record.waiting_file.ok_or(())?;
                record.choices.insert(file, choice);
                if resolve.apply_to_remaining {
                    record.apply_remaining = Some(choice);
                }
                record.enter(JobState::Transferring);
                record.waiting_file = None;
                job::STATUS_OK
            }
            _ => job::STATUS_ERROR,
        }
    };
    // `status` computed under lock; save outside (save takes jobs lock).
    persist_jobs(shared);
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: resolve.id,
            status,
            job_id: resolve.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

/// Coordinated resume after symmetric cancellation: the peer returns a
/// Cancelled/Interrupted/Failed job to active, so retry works from either
/// side. Senders serve again after verifying their sources through handles;
/// receivers return to `Transferring`.
async fn handle_resume(
    shared: &Shared,
    peer: &[u8; 32],
    incoming_side: bool,
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let resume: job::JobResume = job::decode(payload).map_err(|_| ())?;
    job::check_version(resume.v).map_err(|_| ())?;
    let live = peer_session_live(&lock(&shared.state), peer, incoming_side, resume.generation);
    if !live {
        let stale = job::JobReply {
            v: job::JOB_VERSION,
            id: resume.id,
            status: job::STATUS_STALE,
            job_id: resume.job_id,
        };
        reply(send, job::encode(&stale).map_err(|_| ())?).await?;
        return Ok(());
    }
    // Snapshot the job without holding locks across awaits.
    enum ResumeSnap {
        Go {
            sender: bool,
            local_root: u64,
            local_rel: String,
            manifest: Option<Manifest>,
            source_ids: Vec<u64>,
        },
        AckOk,
        AckErr,
    }
    let snap = {
        let jobs = lock(&shared.jobs);
        match wire_job(&jobs, peer, resume.job_id) {
            Some(record)
                if record.peer_key == *peer
                    && super::service_jobs::check_owner(&lock(&shared.state), record.owner)
                        .is_ok() =>
            {
                match record.state {
                    JobState::Interrupted | JobState::Cancelled | JobState::Failed => {
                        ResumeSnap::Go {
                            sender: record.sender,
                            local_root: record.local_root,
                            local_rel: record.local_rel.clone(),
                            manifest: record.manifest.clone(),
                            source_ids: record.source_ids.clone(),
                        }
                    }
                    _ => ResumeSnap::AckOk,
                }
            }
            _ => ResumeSnap::AckErr,
        }
    };
    let (sender, local_root, local_rel, manifest, source_ids) = match snap {
        ResumeSnap::AckOk => {
            let ok = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_OK,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&ok).map_err(|_| ())?).await?;
            return Ok(());
        }
        ResumeSnap::AckErr => {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
        ResumeSnap::Go {
            sender,
            local_root,
            local_rel,
            manifest,
            source_ids,
        } => (sender, local_root, local_rel, manifest, source_ids),
    };
    // Peer-triggered resumes respect the same concurrency bound as local
    // resumes; excess resumes are refused without touching state.
    {
        let active = lock(&shared.jobs)
            .values()
            .filter(|record| ACTIVE_STATES.contains(&record.state))
            .count();
        if active >= MAX_ACTIVE_JOBS {
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
    }
    if sender {
        // Re-verify each source through handles: a changed source fails
        // instead of moving new bytes.
        let manifest = manifest.ok_or(())?;
        // Adopt the leg's pin up front (jobs->roots order); a rebound
        // source fails here with its own cause instead of re-authorizing
        // the new directory below. Decided under the guards, acted on
        // after: no mutex is held across the reply await.
        enum PinDecision {
            Go(fs::PinnedRoot),
            Gone,
            Rebound,
        }
        let decision = {
            let mut jobs = lock(&shared.jobs);
            match wire_job_mut(&mut jobs, peer, resume.job_id) {
                None => PinDecision::Gone,
                Some(record) => {
                    if verify_leg_pin(
                        &lock(&shared.file_roots),
                        record.local_root,
                        &mut record.local_pin,
                    )
                    .is_err()
                    {
                        record.enter(JobState::Failed);
                        record.error = Some(fs::ROOTS_REBOUND.to_owned());
                        PinDecision::Rebound
                    } else {
                        PinDecision::Go(record.local_pin.clone().expect("leg pin just verified"))
                    }
                }
            }
        };
        let pin = match decision {
            PinDecision::Go(pin) => pin,
            PinDecision::Gone => {
                let err = job::JobReply {
                    v: job::JOB_VERSION,
                    id: resume.id,
                    status: job::STATUS_ERROR,
                    job_id: resume.job_id,
                };
                reply(send, job::encode(&err).map_err(|_| ())?).await?;
                return Ok(());
            }
            PinDecision::Rebound => {
                persist_jobs(shared);
                let err = job::JobReply {
                    v: job::JOB_VERSION,
                    id: resume.id,
                    status: job::STATUS_ERROR,
                    job_id: resume.job_id,
                };
                reply(send, job::encode(&err).map_err(|_| ())?).await?;
                return Ok(());
            }
        };
        // Decided under the guards, acted on after: no mutex is held
        // across the reply await.
        let checked =
            check_sending_sources(shared, local_root, &local_rel, &pin, &manifest, &source_ids);
        if let Err(reason) = checked {
            {
                let mut jobs = lock(&shared.jobs);
                if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                    record.enter(JobState::Failed);
                    record.error = Some(reason.to_owned());
                }
            }
            persist_jobs(shared);
            let err = job::JobReply {
                v: job::JOB_VERSION,
                id: resume.id,
                status: job::STATUS_ERROR,
                job_id: resume.job_id,
            };
            reply(send, job::encode(&err).map_err(|_| ())?).await?;
            return Ok(());
        }
        {
            let mut jobs = lock(&shared.jobs);
            if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
                if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                    return Err(());
                }
                record.enter(JobState::Serving);
                record.error = None;
            }
        }
        persist_jobs(shared);
    } else {
        let mut jobs = lock(&shared.jobs);
        if let Some(record) = wire_job_mut(&mut jobs, peer, resume.job_id) {
            if super::service_jobs::check_owner(&lock(&shared.state), record.owner).is_err() {
                return Err(());
            }
            record.enter(JobState::Transferring);
            record.waiting_file = None;
            record.error = None;
        }
        drop(jobs);
        persist_jobs(shared);
    }
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: resume.id,
            status: job::STATUS_OK,
            job_id: resume.job_id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

pub(super) async fn handle_upload_offer(
    shared: &Shared,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    owner: Option<super::service_jobs::JobOwner>,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let offer: job::UploadOffer = job::decode(payload).map_err(|_| ())?;
    job::check_version(offer.v).map_err(|_| ())?;
    let id = offer.sender_job;
    // Decide under short locks; the reply goes out after every guard is
    // dropped so no mutex is held across an await.
    let accept: Result<(), u8> = (|| {
        if lock(&shared.transfer_dir).is_none() {
            return Err(job::STATUS_ERROR);
        }
        if !lock(&shared.state).trust.granted(peer, Grant::Transfer) {
            return Err(job::STATUS_DENIED);
        }
        if manifest::validate(&offer.manifest).is_err() || fs::check_rel(&offer.dst_rel).is_err() {
            return Err(job::STATUS_ERROR);
        }
        // Pin the destination leg before insertion (sequential roots
        // hold, never nested): a rebind between here and first use then
        // fails the leg instead of landing bytes in the new directory.
        let local_pin = lock(&shared.file_roots).pin(offer.dst_root).ok();
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        super::service_jobs::check_owner(&state, owner).map_err(|_| job::STATUS_STALE)?;
        if wire_job(&jobs, peer, id).is_some() {
            return Err(job::STATUS_ERROR);
        }
        let local_id = allocate_job_id(shared).map_err(|_| job::STATUS_ERROR)?;
        shared
            .job_next
            .fetch_max(id.saturating_add(1), Ordering::Relaxed);
        jobs.insert(
            local_id,
            JobRecord {
                owner,
                destination: None,
                id: local_id,
                wire_id: id,
                created_at: retention_now(),
                peer_key: *peer,
                peer_id: endpoint_id_of(peer),
                direction: JobDirection::Upload,
                sender: false,
                state: JobState::Transferring,
                manifest: Some(offer.manifest),
                local_root: offer.dst_root,
                local_rel: offer.dst_rel,
                names: Vec::new(),
                local_pin,
                outgoing: false,
                landed: HashMap::new(),
                file_index: 0,
                done_bytes: 0,
                files_done: 0,
                resumed_files: 0,
                skipped_files: 0,
                waiting_file: None,
                skipped_symlinks: 0,
                choices: HashMap::new(),
                apply_remaining: None,
                error: None,
                in_flight: 0,
                attempt: 0,
                unsaved_landed: Vec::new(),
                retry: None,
                sent_at: None,
                source_ids: Vec::new(),
            },
        );
        Ok(())
    })();
    persist_jobs(shared);
    let status = match accept {
        Ok(()) => job::STATUS_OK,
        Err(status) => status,
    };
    reply(
        send,
        job::encode(&job::JobReply {
            v: job::JOB_VERSION,
            id: offer.id,
            status,
            job_id: id,
        })
        .map_err(|_| ())?,
    )
    .await?;
    Ok(())
}

fn prepared_download_permitted(
    state: &State,
    peer: &[u8; 32],
    session_id: u64,
    generation: u64,
    permission_epoch: u64,
) -> bool {
    state.trust.granted(peer, Grant::Transfer)
        && *state.transfer_epochs.get(peer).unwrap_or(&0) == permission_epoch
        && matches!(state.incoming.get(&session_id), Some(session)
            if session.generation == generation
            && session.phase == SessionPhase::Active && session.peer_key == *peer)
}

pub(super) async fn handle_download_request(
    shared: &Shared,
    session_id: u64,
    peer: &[u8; 32],
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
    owner: Option<super::service_jobs::JobOwner>,
) -> Result<(), ()> {
    async fn reply(send: &mut iroh::endpoint::SendStream, bytes: Vec<u8>) -> Result<(), ()> {
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        out.extend_from_slice(&bytes);
        crate::sidestream::write_frame(
            send,
            &out,
            |framed: &Vec<u8>| Ok(framed.clone()),
            "the job frame was not written",
        )
        .await
        .map_err(|_| ())
    }
    let request: job::DownloadRequest = job::decode(payload).map_err(|_| ())?;
    job::check_version(request.v).map_err(|_| ())?;
    let respond = |status: u8, job_id: u64, manifest: Option<Manifest>| {
        job::encode(&job::ManifestReply {
            v: job::JOB_VERSION,
            id: request.id,
            status,
            job_id,
            manifest,
        })
    };
    // Fast gates first, without holding anything across an await.
    if lock(&shared.transfer_dir).is_none() {
        reply(send, respond(job::STATUS_ERROR, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    }
    let permission_epoch = {
        let state = lock(&shared.state);
        state
            .trust
            .granted(peer, Grant::Transfer)
            .then(|| *state.transfer_epochs.get(peer).unwrap_or(&0))
    };
    let Some(permission_epoch) = permission_epoch else {
        reply(send, respond(job::STATUS_DENIED, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    };
    if check_sources(&request.src_folder, &request.names).is_err() {
        reply(send, respond(job::STATUS_ERROR, 0, None).map_err(|_| ())?).await?;
        return Ok(());
    }
    // The preparation is a job from the start: the controller sees it as
    // Preparing, can cancel it, and loses it with its owner. Same order as
    // publication/rename: jobs -> state, so activation and permission
    // changes cannot interleave between the check and the insert. The
    // source leg is pinned now: a rebind before the first serve fails the
    // leg instead of serving moved bytes.
    let local_pin = lock(&shared.file_roots).pin(request.src_root).ok();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let admitted: Result<u64, u8> = (|| {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        if !prepared_download_permitted(
            &state,
            peer,
            session_id,
            request.generation,
            permission_epoch,
        ) || super::service_jobs::check_owner(&state, owner).is_err()
        {
            return Err(job::STATUS_DENIED);
        }
        // Sender job ids minted on the host share the local allocator, so
        // adopted controller ids can never collide with a later mint.
        let id = allocate_job_id(shared).map_err(|_| job::STATUS_ERROR)?;
        jobs.insert(
            id,
            JobRecord {
                owner,
                destination: None,
                id,
                wire_id: id,
                created_at: retention_now(),
                peer_key: *peer,
                peer_id: endpoint_id_of(peer),
                direction: JobDirection::Upload,
                sender: true,
                state: JobState::Preparing,
                manifest: None,
                local_root: request.src_root,
                local_rel: request.src_folder.clone(),
                names: request.names.clone(),
                local_pin,
                outgoing: false,
                skipped_symlinks: 0,
                file_index: 0,
                done_bytes: 0,
                files_done: 0,
                resumed_files: 0,
                skipped_files: 0,
                waiting_file: None,
                landed: HashMap::new(),
                choices: HashMap::new(),
                apply_remaining: None,
                error: None,
                in_flight: 0,
                attempt: 0,
                unsaved_landed: Vec::new(),
                retry: None,
                sent_at: None,
                source_ids: Vec::new(),
            },
        );
        lock(&shared.job_cancel).insert(id, stop);
        Ok(id)
    })();
    let id = match admitted {
        Ok(id) => id,
        Err(status) => {
            reply(send, respond(status, 0, None).map_err(|_| ())?).await?;
            return Ok(());
        }
    };
    persist_jobs(shared);
    // Whatever ends this request before preparation does, the job does
    // not stay Preparing.
    let mut unfinished = Unprepared { shared, id };
    // Scan, hash and offer from this host's roots; nothing is announced
    // before every hash is served. A quick preparation answers with its
    // manifest; a longer one answers at once that it is preparing, and the
    // controller asks for the manifest until it is ready.
    let preparing = prepare_host_manifest(
        shared,
        request.src_root,
        &request.src_folder,
        &request.names,
        prepare_scan_hooks(shared),
        stopped,
    );
    tokio::pin!(preparing);
    let quick = tokio::select! {
        prepared = &mut preparing => Some(prepared),
        _ = tokio::time::sleep(PREPARE_ANSWER_WITHIN) => None,
    };
    let Some(prepared) = quick else {
        reply(send, respond(job::STATUS_OK, id, None).map_err(|_| ())?).await?;
        let prepared = preparing.await;
        if let Err(status) =
            finish_preparation(shared, &request, session_id, permission_epoch, id, prepared)
        {
            tracing::info!(
                target: crate::trace::FILES,
                job = id,
                status,
                "a download prepared after its answer failed"
            );
        }
        unfinished.id = 0;
        return Ok(());
    };
    let finished = finish_preparation(shared, &request, session_id, permission_epoch, id, prepared);
    unfinished.id = 0;
    match finished {
        Ok(manifest) => {
            reply(
                send,
                respond(job::STATUS_OK, id, Some(manifest)).map_err(|_| ())?,
            )
            .await?;
        }
        Err(status) => {
            reply(send, respond(status, 0, None).map_err(|_| ())?).await?;
        }
    }
    Ok(())
}

/// How long a download request waits for its preparation before it
/// answers that the job is preparing.
const PREPARE_ANSWER_WITHIN: Duration = Duration::from_secs(1);
const PREPARE_FAILED: &str = "the other computer could not prepare the transfer";
const PREPARE_STOPPED: &str = "the transfer stopped before it was prepared";

/// A download job still Preparing when its request ends fails: nothing
/// else would ever finish preparing it. `id` 0 once it finished.
struct Unprepared<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for Unprepared<'_> {
    fn drop(&mut self) {
        if self.id == 0 {
            return;
        }
        lock(&self.shared.job_cancel).remove(&self.id);
        let stopped = lock(&self.shared.jobs)
            .get_mut(&self.id)
            .filter(|record| record.state == JobState::Preparing)
            .map(|record| {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_STOPPED.to_owned());
            })
            .is_some();
        if stopped {
            persist_jobs(self.shared);
        }
    }
}

/// Settles a download job once its preparation ends: Serving with its
/// manifest, or Failed. A job cancelled, or whose owner went,
/// meanwhile keeps that state; a permission or session that changed
/// meanwhile fails it. Returns the manifest or the status to answer with.
fn finish_preparation(
    shared: &Shared,
    request: &job::DownloadRequest,
    session_id: u64,
    permission_epoch: u64,
    id: u64,
    prepared: Result<(Manifest, u64, Vec<u64>), u8>,
) -> Result<Manifest, u8> {
    lock(&shared.job_cancel).remove(&id);
    let settled = {
        let mut jobs = lock(&shared.jobs);
        let state = lock(&shared.state);
        let record = jobs.get_mut(&id).ok_or(job::STATUS_ERROR)?;
        if record.state != JobState::Preparing {
            return Err(job::STATUS_ERROR);
        }
        let peer = record.peer_key;
        let permitted = prepared_download_permitted(
            &state,
            &peer,
            session_id,
            request.generation,
            permission_epoch,
        ) && super::service_jobs::check_owner(&state, record.owner).is_ok();
        match prepared {
            Ok(_) if !permitted => {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_FAILED.to_owned());
                Err(job::STATUS_DENIED)
            }
            Ok((manifest, skipped, source_ids)) => {
                record.manifest = Some(manifest.clone());
                record.skipped_symlinks = skipped;
                record.source_ids = source_ids;
                record.enter(JobState::Serving);
                Ok(manifest)
            }
            Err(status) => {
                record.enter(JobState::Failed);
                record.error = Some(PREPARE_FAILED.to_owned());
                Err(status)
            }
        }
    };
    persist_jobs(shared);
    settled
}

/// Scan probes for one engine's download preparations; production
/// leaves them unset. Keyed by [`Core::prestage_key`].
#[cfg(any(test, feature = "test-probes"))]
impl Core {
    /// Scan probes for this engine's download preparations, or none.
    pub fn set_prepare_scan_hooks(&self, hooks: Option<manifest::ScanHooks>) {
        let _engine = self.engine();
        *lock(&PREPARE_SCAN_HOOKS) = hooks.map(|hooks| (self.prestage_key(), hooks));
    }
}

#[cfg(any(test, feature = "test-probes"))]
pub(crate) static PREPARE_SCAN_HOOKS: std::sync::Mutex<Option<(usize, manifest::ScanHooks)>> =
    std::sync::Mutex::new(None);

fn prepare_scan_hooks(shared: &Shared) -> manifest::ScanHooks {
    #[cfg(any(test, feature = "test-probes"))]
    if let Some((engine, hooks)) = *lock(&PREPARE_SCAN_HOOKS) {
        if engine == shared as *const Shared as usize {
            return hooks;
        }
    }
    let _ = shared;
    manifest::ScanHooks::none()
}

/// Scans and imports every file of the chosen entries, returning the
/// filled manifest and the symlinks left out, until `stopped` says the
/// job was cancelled or lost its owner. Runs on the host inside its
/// runtime.
async fn prepare_host_manifest(
    shared: &Shared,
    src_root: u64,
    src_folder: &str,
    names: &[String],
    scan_hooks: manifest::ScanHooks,
    mut stopped: tokio::sync::watch::Receiver<bool>,
) -> Result<(Manifest, u64, Vec<u64>), u8> {
    tokio::select! {
        biased;
        _ = stopped.wait_for(|stop| *stop) => Err(job::STATUS_ERROR),
        prepared = prepare_all(shared, src_root, src_folder, names, scan_hooks) => prepared,
    }
}

async fn prepare_all(
    shared: &Shared,
    src_root: u64,
    src_folder: &str,
    names: &[String],
    scan_hooks: manifest::ScanHooks,
) -> Result<(Manifest, u64, Vec<u64>), u8> {
    let mut manifest = Manifest {
        entries: Vec::new(),
    };
    let mut skipped_symlinks = 0;
    let mut scanned = HashMap::new();
    for rel in source_rels(src_folder, names) {
        let (part, skipped, ids) = prepare_source(shared, src_root, &rel, scan_hooks).await?;
        skipped_symlinks += skipped;
        scanned.extend(ids);
        manifest.entries.extend(part.entries);
        if manifest.entries.len() > manifest::MAX_MANIFEST_FILES {
            return Err(job::STATUS_ERROR);
        }
    }
    manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
    manifest::validate(&manifest).map_err(|_| job::STATUS_ERROR)?;
    let source_ids = source_ids(&manifest, src_folder, &scanned);
    Ok((manifest, skipped_symlinks, source_ids))
}

/// Scans one chosen entry `src_rel`, returning its part of the manifest,
/// the symlinks left out and each file's id by root-relative path.
/// Nothing is read or hashed: each file is opened and checked against the
/// manifest when it is served.
async fn prepare_source(
    shared: &Shared,
    src_root: u64,
    src_rel: &str,
    scan_hooks: manifest::ScanHooks,
) -> Result<(Manifest, u64, HashMap<String, u64>), u8> {
    // Snapshot the approved roots under a short lock; the recursive walk
    // runs on a bounded worker with no locks held.
    let snapshot = lock(&shared.file_roots).clone();
    let scan = manifest::scan_roots_async(snapshot, src_root, src_rel.to_owned(), scan_hooks)
        .await
        .map_err(|_| job::STATUS_ERROR)?;
    let mut manifest = manifest::skeleton(&scan);
    // Directory sources keep their top folder.
    {
        let roots = lock(&shared.file_roots);
        if roots.resolve_dir(src_root, src_rel).is_ok() {
            manifest::prefix_source_dir(src_rel, &mut manifest);
        }
    }
    let ids = scan
        .files
        .into_iter()
        .map(|file| (file.root_rel, file.ino))
        .collect();
    Ok((manifest, scan.skipped_symlinks, ids))
}

fn endpoint_id_of(peer: &[u8; 32]) -> String {
    iroh::PublicKey::from_bytes(peer)
        .map(|key| key.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn prepared_download_cannot_survive_permission_toggle_or_new_generation() {
        let boot = || {
            Core::boot(
                CoreConfig {
                    listen: Listen::Loopback,
                    ..CoreConfig::default()
                },
                iroh::RelayMode::Disabled,
            )
            .unwrap()
        };
        let host = boot();
        let controller = boot();
        host.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        controller
            .set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(
                &host
                    .create_invitation(std::time::Duration::from_secs(60))
                    .unwrap(),
            )
            .unwrap();
        host.accept_invitation(
            &controller
                .create_invitation(std::time::Duration::from_secs(60))
                .unwrap(),
        )
        .unwrap();
        let peer = parse_endpoint_id(&controller.endpoint_id()).unwrap();
        let grants = crate::session::Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: true,
        };
        host.set_desktop_sharing(grants).unwrap();
        controller.connect_files(&host.endpoint_id()).unwrap();
        let (id, generation, epoch) = {
            let state = lock(&host.shared.state);
            let session = state.incoming_live().unwrap();
            let epoch = *state.transfer_epochs.get(&peer).unwrap();
            assert!(prepared_download_permitted(
                &state,
                &peer,
                session.id,
                session.generation,
                epoch
            ));
            (session.id, session.generation, epoch)
        };
        host.set_desktop_sharing(crate::session::Grants {
            transfer: false,
            ..grants
        })
        .unwrap();
        host.set_desktop_sharing(grants).unwrap();
        let state = lock(&host.shared.state);
        assert!(!prepared_download_permitted(
            &state, &peer, id, generation, epoch
        ));
        let current = *state.transfer_epochs.get(&peer).unwrap();
        assert!(prepared_download_permitted(
            &state, &peer, id, generation, current
        ));
        assert!(!prepared_download_permitted(
            &state,
            &peer,
            id,
            generation + 1,
            current
        ));
        drop(state);
        host.shutdown();
        controller.shutdown();
    }

    fn record(id: u64, size: u64, age: u64, state: JobState) -> JobRecord {
        JobRecord {
            owner: None,
            destination: None,
            id,
            wire_id: id,
            created_at: retention_now().saturating_sub(age),
            peer_key: [0; 32],
            peer_id: String::new(),
            direction: JobDirection::Download,
            sender: false,
            state,
            manifest: Some(Manifest {
                entries: vec![ManifestEntry {
                    path: format!("file-{id}"),
                    size,
                    source_mtime_ms: 0,
                    is_dir: false,
                }],
            }),
            local_root: 0,
            local_rel: String::new(),
            names: Vec::new(),
            local_pin: None,
            outgoing: false,
            file_index: 0,
            done_bytes: 0,
            files_done: 0,
            resumed_files: 0,
            skipped_files: 0,
            waiting_file: None,
            skipped_symlinks: 0,
            landed: HashMap::new(),
            choices: HashMap::new(),
            apply_remaining: None,
            error: None,
            in_flight: 0,
            attempt: 0,
            unsaved_landed: Vec::new(),
            retry: None,
            sent_at: None,
            source_ids: Vec::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn legacy_leg_adopts_pin_on_first_destination_use() {
        let core = Core::boot(
            CoreConfig {
                listen: Listen::Loopback,
                ..CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .unwrap();
        let scratch = std::env::temp_dir().join(format!(
            "vnilink-adopt-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(scratch.join("root/got")).unwrap();
        std::fs::create_dir_all(scratch.join("elsewhere")).unwrap();
        // Roots canonicalize (macOS /var is a symlink); compare against
        // the canonical form.
        let scratch = std::fs::canonicalize(&scratch).unwrap();
        core.set_transfer_directory(scratch.join("blobs")).unwrap();
        core.set_file_roots(vec![scratch.join("root")]).unwrap();
        // Legacy record: persisted before pinning, so the pin is None.
        let mut legacy = record(7, 10, 0, JobState::Transferring);
        legacy.local_rel = "got".to_owned();
        assert!(legacy.local_pin.is_none());
        lock(&core.shared.jobs).insert(7, legacy);
        // First destination use adopts the current roots and persists.
        let dest = core.receiver_dest(7, 0, "got", "file-7").unwrap();
        assert_eq!(dest, scratch.join("root/got/file-7"));
        let adopted = lock(&core.shared.jobs).get(&7).unwrap().local_pin.clone();
        assert!(adopted.is_some());
        // The adopted pin is strict: a rebound entry fails the leg.
        core.set_file_roots(vec![scratch.join("elsewhere")])
            .unwrap();
        let err = core.receiver_dest(7, 0, "got", "file-7").unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidSetting(msg) if msg == crate::fs::ROOTS_REBOUND
        ));
        let _ = std::fs::remove_dir_all(&scratch);
        core.shutdown();
    }

    #[test]
    fn configured_retention_has_exact_age_budget_and_shared_active_bounds() {
        let mut first = record(1, 30, 0, JobState::Interrupted);
        first.created_at = 90;
        let mut second = record(2, 80, 0, JobState::Cancelled);
        second.created_at = 95;
        let mut active = record(3, 200, 0, JobState::Transferring);
        active.created_at = 0;
        let jobs = HashMap::from([(1, first), (2, second), (3, active)]);
        let policy = crate::job::FileRetentionPolicy {
            resume_cache_bytes: 110,
            resume_cache_age_secs: 10,
            ..Default::default()
        };
        assert_eq!(
            retained_job_ids(&jobs, policy, 99),
            std::collections::HashSet::from([1, 2, 3])
        );
        assert_eq!(
            retained_job_ids(&jobs, policy, 100),
            std::collections::HashSet::from([2, 3])
        );
        let mut small = policy;
        small.resume_cache_bytes = 79;
        assert_eq!(
            retained_job_ids(&jobs, small, 100),
            std::collections::HashSet::from([3])
        );
        small.resume_cache_bytes = 0;
        assert_eq!(
            retained_job_ids(&jobs, small, 99),
            std::collections::HashSet::from([3])
        );
        small.resume_cache_bytes = 110;
        small.resume_cache_age_secs = 0;
        assert_eq!(
            retained_job_ids(&jobs, small, 99),
            std::collections::HashSet::from([3])
        );
    }

    #[test]
    fn retention_bounds_inactive_partials_and_keeps_active_jobs() {
        let gib = 1024 * 1024 * 1024;
        let jobs = HashMap::from([
            (1, record(1, 3 * gib, 20, JobState::Cancelled)),
            (2, record(2, 3 * gib, 10, JobState::Interrupted)),
            (
                3,
                record(
                    3,
                    100,
                    crate::job::FileRetentionPolicy::default().resume_cache_age_secs + 1,
                    JobState::Interrupted,
                ),
            ),
            (4, record(4, 100, 0, JobState::Complete)),
            (5, record(5, 100, 0, JobState::Failed)),
            (
                6,
                record(
                    6,
                    8 * gib,
                    crate::job::FileRetentionPolicy::default().resume_cache_age_secs + 1,
                    JobState::Transferring,
                ),
            ),
        ]);
        let owners = retained_job_ids(
            &jobs,
            crate::job::FileRetentionPolicy::default(),
            retention_now(),
        );
        assert!(!owners.contains(&1));
        assert!(!owners.contains(&3));
        assert!(!owners.contains(&4));
        assert!(owners.contains(&2));
        assert!(owners.contains(&6));
    }
}

impl Core {
    pub fn file_retention_policy(&self) -> Result<crate::job::FileRetentionPolicy, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.file_retention_policy)
    }
    /// Persist before publishing to GC. A failed save cannot cause eviction
    /// under a policy that was never committed. Active jobs remain protected.
    pub fn set_file_retention_policy(
        &self,
        policy: crate::job::FileRetentionPolicy,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let policy = policy.validate()?;
        let mut state = self.alive_state()?;
        let previous = std::mem::replace(&mut state.file_retention_policy, policy);
        if let Err(error) = persist(&self.shared, &mut state) {
            state.file_retention_policy = previous;
            return Err(error);
        }
        *lock(&self.shared.file_retention_policy) = policy;
        Ok(())
    }
}

impl Core {
    pub fn file_resource_policy(&self) -> Result<crate::job::FileResourcePolicy, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.file_resource_policy)
    }
    /// Persist first. Reductions govern new reservations; active work retains
    /// its existing reservation until completion/cancel to avoid partial loss.
    pub fn set_file_resource_policy(
        &self,
        policy: crate::job::FileResourcePolicy,
    ) -> Result<(), Error> {
        let _engine = self.engine();
        let policy = policy.validate()?;
        let mut state = self.alive_state()?;
        let previous = std::mem::replace(&mut state.file_resource_policy, policy);
        if let Err(error) = persist(&self.shared, &mut state) {
            state.file_resource_policy = previous;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod crash_staging_tests {
    use super::*;
    #[test]
    fn reconciliation_keeps_resumable_work_and_removes_only_managed_orphans() {
        let base =
            std::env::temp_dir().join(format!("vnilink-stage-reconcile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(base.clone());
        for name in ["job-1", "job-2", "job-3", "job-02", "unrelated"] {
            std::fs::create_dir_all(base.join(STAGING_DIR).join(name)).unwrap();
            std::fs::write(base.join(STAGING_DIR).join(name).join("bytes"), name).unwrap();
        }
        reconcile_staging_at(&base, &std::collections::HashSet::from([1]));
        assert_eq!(
            std::fs::read(base.join("staging/job-1/bytes")).unwrap(),
            b"job-1"
        );
        assert!(!base.join("staging/job-2").exists());
        assert!(!base.join("staging/job-3").exists());
        assert!(base.join("staging/job-02/bytes").exists());
        assert!(base.join("staging/unrelated/bytes").exists());
        #[cfg(unix)]
        {
            std::fs::create_dir(base.join("external")).unwrap();
            std::fs::write(base.join("external/preserve"), "payload").unwrap();
            std::os::unix::fs::symlink(base.join("external"), base.join("staging/job-4")).unwrap();
            reconcile_staging_at(&base, &std::collections::HashSet::from([1]));
            assert!(std::fs::symlink_metadata(base.join("staging/job-4")).is_err());
            assert_eq!(
                std::fs::read(base.join("external/preserve")).unwrap(),
                b"payload"
            );
        }
    }
}

#[cfg(test)]
mod arrival_tests {
    use super::*;

    /// Bytes arriving mid-file reach the job and tell watchers, without
    /// waiting for the file to finish.
    #[test]
    fn arriving_bytes_show_on_the_job_and_advance_the_revision() {
        let core = crate::core::Core::boot(
            crate::core::CoreConfig {
                listen: crate::core::Listen::Loopback,
                ..crate::core::CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .unwrap();
        let shared = core.shared.clone();
        lock(&shared.jobs).insert(7, JobRecord::test_receiver(7, 1000));
        let before = core.job_revision();
        let (progress, arriving) = std::sync::mpsc::channel();
        let watcher = follow_arrival(shared.jobs.clone(), shared.clone(), 7, arriving);
        progress.send(400).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while lock(&shared.jobs)[&7].snapshot().done_bytes != 400 {
            assert!(
                std::time::Instant::now() < deadline,
                "arriving bytes never showed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(core.job_revision() > before);
        drop(progress);
        watcher.join().unwrap();
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;

    fn core() -> Core {
        Core::boot(
            crate::core::CoreConfig {
                listen: crate::core::Listen::Loopback,
                ..crate::core::CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .unwrap()
    }

    /// Failures that may clear are retried with growing pauses while the
    /// job keeps its state and shows why; bytes moved start the count
    /// again, and after the last try the job is interrupted for the cause.
    #[test]
    fn retries_back_off_reset_on_progress_and_give_up() {
        let core = core();
        let mut record = JobRecord::test_receiver(7, 1000);
        record.enter(JobState::Transferring);
        lock(&core.shared.jobs).insert(7, record);
        let retry = |core: &Core| lock(&core.shared.jobs)[&7].retry.clone();

        let first = core.retry_later(7, STALLED).unwrap();
        assert_eq!(first.state, JobState::Transferring);
        assert_eq!(first.error.as_deref(), Some(STALLED));
        let pause = retry(&core).unwrap().next_at - std::time::Instant::now();
        assert!(pause > Duration::from_millis(700) && pause <= Duration::from_millis(1200));
        // Polls wait out the pause.
        assert_eq!(core.job_poll(7).unwrap().state, JobState::Transferring);

        core.retry_later(7, STALLED).unwrap();
        assert_eq!(retry(&core).unwrap().attempts, 2);
        // Bytes arrived since: a new run of failures.
        lock(&core.shared.jobs).get_mut(&7).unwrap().in_flight = 100;
        core.retry_later(7, UNREACHABLE).unwrap();
        assert_eq!(retry(&core).unwrap().attempts, 1);

        for _ in 1..MAX_RETRIES {
            core.retry_later(7, UNREACHABLE).unwrap();
        }
        assert_eq!(retry(&core).unwrap().attempts, MAX_RETRIES);
        let stopped = core.retry_later(7, UNREACHABLE).unwrap();
        assert_eq!(stopped.state, JobState::Interrupted);
        assert_eq!(stopped.error.as_deref(), Some(UNREACHABLE));
        assert!(retry(&core).is_none());
        core.shutdown();
    }

    /// A step that works ends the retries and the shown cause.
    #[test]
    fn a_working_step_settles_retries() {
        let mut record = JobRecord::test_receiver(7, 1000);
        record.error = Some(STALLED.to_owned());
        record.retry = Some(Retry {
            attempts: 3,
            first_at: std::time::Instant::now(),
            next_at: std::time::Instant::now(),
            done_at: 0,
        });
        Core::retry_settled(&mut record);
        assert!(record.retry.is_none());
        assert!(record.error.is_none());
    }

    /// The file moving now is the receiver's own place, or for a sender the
    /// file after those landed or skipped; none once files stop moving.
    #[test]
    fn the_current_file_follows_each_side() {
        let mut record = JobRecord::test_receiver_of(1, 3);
        let path = |index: usize| record_path(&record, index);
        fn record_path(record: &JobRecord, index: usize) -> Option<String> {
            record.files().get(index).map(|entry| entry.path.clone())
        }
        let paths: Vec<_> = (0..3).map(path).collect();
        record.enter(JobState::Transferring);
        record.file_index = 1;
        assert_eq!(record.snapshot().current_file, paths[1]);
        record.sender = true;
        record.enter(JobState::Serving);
        record.files_done = 1;
        record.skipped_files = 1;
        assert_eq!(record.snapshot().current_file, paths[2]);
        record.enter(JobState::Interrupted);
        assert_eq!(record.snapshot().current_file, None);
    }

    /// A sender's progress follows its own stream, counting earlier files
    /// whole, and is told at most once a megabyte until the file is sent.
    #[test]
    fn a_sender_shows_what_its_stream_took() {
        let core = core();
        let mut record = JobRecord::test_receiver_of(7, 3);
        record.sender = true;
        record.enter(JobState::Serving);
        record.peer_key = [4; 32];
        lock(&core.shared.jobs).insert(7, record);
        let sent_at = |core: &Core| lock(&core.shared.jobs)[&7].sent_at;

        sending_at(&core.shared, &[4; 32], 7, 2, 0, false);
        assert_eq!(sent_at(&core), Some((2, 0)));
        // Two earlier files of one byte each count whole.
        assert_eq!(core.job_snapshot(7).unwrap().done_bytes, 2);
        sending_at(&core.shared, &[4; 32], 7, 2, 100, false);
        assert_eq!(sent_at(&core), Some((2, 0)), "under a megabyte waits");
        sending_at(&core.shared, &[4; 32], 7, 2, 100, true);
        assert_eq!(sent_at(&core), Some((2, 100)));
        // Another peer's request never moves it.
        sending_at(&core.shared, &[5; 32], 7, 2, 500, true);
        assert_eq!(sent_at(&core), Some((2, 100)));
        core.shutdown();
    }

    #[test]
    fn pauses_grow_to_half_a_minute() {
        for (attempt, seconds) in [
            (1, 1.0),
            (2, 2.0),
            (3, 4.0),
            (5, 16.0),
            (6, 30.0),
            (9, 30.0),
        ] {
            let pause = retry_pause(attempt).as_secs_f64();
            assert!(
                pause >= seconds * 0.8 && pause <= seconds * 1.2,
                "{attempt}: {pause}"
            );
        }
    }
}

#[cfg(test)]
mod served_job_tests {
    use super::*;

    fn sender(id: u64, wire_id: u64, peer: u8, state: JobState) -> JobRecord {
        let mut record = JobRecord::test_receiver_of(id, 1);
        record.sender = true;
        record.wire_id = wire_id;
        record.peer_key = [peer; 32];
        record.enter(state);
        record
    }

    /// A request finds the peer's own sending job by its wire id while it
    /// serves; a receiver record sharing that id, another peer's job and a
    /// stopped job never match.
    #[test]
    fn a_request_finds_only_the_peers_serving_sender() {
        let mut receiver = JobRecord::test_receiver_of(1, 1);
        receiver.wire_id = 5;
        receiver.peer_key = [9; 32];
        let jobs = HashMap::from([
            (1, receiver),
            (2, sender(2, 5, 9, JobState::Transferring)),
            (3, sender(3, 6, 8, JobState::Serving)),
            (4, sender(4, 7, 9, JobState::Cancelled)),
        ]);
        assert_eq!(serving_job(&jobs, &[9; 32], 5).map(|r| r.id), Some(2));
        assert!(serving_job(&jobs, &[9; 32], 6).is_none(), "another peer");
        assert!(serving_job(&jobs, &[9; 32], 7).is_none(), "stopped");
        assert!(serving_job(&jobs, &[8; 32], 5).is_none());
    }
}

#[cfg(test)]
#[path = "jobs_resume_tests.rs"]
mod resume_tests;

#[cfg(test)]
#[path = "jobs_impaired_tests.rs"]
mod impaired_transfer_tests;
