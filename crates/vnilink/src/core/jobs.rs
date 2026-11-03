// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Transfer jobs: scheduling, progress, resume and peer binding.
//!
//! A job moves one manifest between this computer and one peer, with
//! partial files, conflict choices, cancellation and restartable progress.
//! Each side keeps its own record and polls it; a sender learns how far its
//! receiver got by asking it, so both directions complete honestly. File
//! bytes move over `vnilink/files/1` ([`crate::transfer`], with
//! [`Core::fetch_job_file`] as the receiver's entry point). The sender
//! scans its sources into a [`Manifest`] of paths, sizes and modification
//! times and reads nothing more ahead of time. A download the other
//! computer takes long to prepare is `Preparing` at once: the host answers
//! within a second, and the receiver asks for the manifest
//! (`TAG_JOB_MANIFEST`) until it is ready. A preparation cut short has
//! nothing to resume from and fails.
//!
//! Jobs are peer-scoped: a request names a job by the peer's key, so one
//! peer cannot resume, list or adopt another's. A job a Files service
//! owns rides that service's connection for its control, and a state change
//! it cannot survive is decided here, not by the window:
//!
//! - Revoking Files sharing interrupts that peer's jobs
//!   ([`interrupt_peer_jobs`]); a controller session that ends interrupts
//!   the jobs on it ([`interrupt_controller_jobs`]) rather than replaying
//!   them; a restart leaves what was in flight `Interrupted`.
//! - A failure that may clear by itself (a lost or redialing connection, a
//!   stalled stream, a sender busy or shut down) keeps the job going and
//!   tries again after about 1, 2, 4, 8, 16, then 30 s ([`retry_pause`]);
//!   once [`MAX_RETRIES`] tries or [`RETRY_WINDOW`] pass without a byte
//!   moving, the job is interrupted. An interrupted job says why until it
//!   resumes.
//! - A job whose Files owner is gone is interrupted and names which part
//!   went (see `service_jobs::orphan_reason`); the next owner for that peer
//!   adopts and resumes it ([`Core::files_service_resume_orphans`]), and
//!   [`Core::job_resume`] resumes any other.
//! - Interrupted jobs do not count against [`MAX_ACTIVE_JOBS`] until they
//!   resume, and resuming re-checks the bound.
//!
//! [`Core::start_job_driver`] runs jobs in the background, independent of
//! any window, with a worker per job that has a step to take: a receiver
//! takes file after file until it waits on a decision, on admission or on
//! its peer; a sender asks its receiver how far it got, soon while that
//! changes and seldom while it does not. [`Core::job_poll`] takes one step
//! for a caller driving a job itself, and at most one file per call. Calls
//! come from outside the runtime, preferably off the UI thread.
//!
//! While a receiver takes one file it fetches up to [`PREFETCH_FILES`] of
//! the next small files (one segment or less) ahead into their partials,
//! stopping once [`PREFETCH_BYTES`] are fetching, so many small files do
//! not cost a round trip each. They still publish one at a time in manifest
//! order through the same conflict and permission checks, and a file is
//! used only if it arrived whole on the files connection still open:
//! revoking closes that connection, so nothing fetched after a revoke
//! lands, though a file that had already arrived whole when the revoke
//! reached this computer may.
//!
//! Publication follows the conflict choice and re-checks cancellation,
//! grant and owner under the commit ([`Core::publish_staged`]). Partials
//! live under the job's staging in the transfer directory. Complete,
//! failed and forgotten jobs remove them; cancelled and interrupted jobs
//! keep them for resume, within the retention policy
//! ([`crate::job::FileRetentionPolicy`]) applied when jobs load. Records
//! persist in per-job journals (`job_journal`).
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
#[cfg(any(unix, windows))]
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

#[cfg(any(unix, windows))]
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

const FLUSH_FAILED: &str = "the files could not be flushed to disk";

// Why a job stopped short of finishing, shown with the job until it
// resumes.
const UNREACHABLE: &str = "the other computer could not be reached";

pub(super) const CONNECTION_ENDED: &str = "the connection to the other computer ended";

const STALLED: &str = "the transfer stalled";

const SENDER_BUSY: &str = "the other computer is busy sending other files";

pub(super) const SENDER_STOPPED: &str = "the other computer stopped sending";

pub(super) const RECEIVER_STOPPED: &str = "the other computer stopped receiving";

pub(super) const SENDER_SHUT_DOWN: &str = "the other computer shut down";

pub(super) const PERMISSION_OFF: &str = "file transfer is no longer allowed";

const UNPAIRED: &str = "the computers are no longer paired";

pub(super) const OWNER_GONE: &str = "the Files window that started it closed";

const STOPPED: &str = "the transfer stopped before the file was written";

const VOLUME_CHANGED: &str = "the destination's disk changed";

pub(super) const APP_CLOSED: &str = "VniLink closed during the transfer";

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
    /// Sender only: its resume is being told to its receiver. Until the
    /// receiver has heard, what it reports is from before the resume, so
    /// the sender does not follow it. Not saved.
    #[serde(skip)]
    telling_resume: bool,
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
            manifest: Some(Manifest::new(vec![ManifestEntry {
                path: "file.bin".to_owned(),
                is_dir: false,
                size,
                source_mtime_ms: 0,
            }])),
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
            telling_resume: false,
            sent_at: None,
            source_ids: Vec::new(),
        }
    }

    /// A download of `files` files, as a receiver holds it.
    #[cfg(test)]
    pub(super) fn test_receiver_of(id: u64, files: usize) -> Self {
        let mut record = Self::test_receiver(id, 0);
        record.manifest = Some(Manifest::new(
            (0..files)
                .map(|index| ManifestEntry {
                    path: format!("file-{index:05}.bin"),
                    is_dir: false,
                    size: 1,
                    source_mtime_ms: 0,
                })
                .collect(),
        ));
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
        let before = self
            .manifest
            .as_ref()
            .map_or(0, |manifest| manifest.bytes_before(file));
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
        self.file(index).map(|entry| entry.path.clone())
    }

    /// Bytes done, including the file arriving now.
    fn done(&self) -> u64 {
        self.done_bytes.saturating_add(self.in_flight)
    }

    /// File number `number` of the manifest, folders not counted.
    fn file(&self, number: usize) -> Option<&ManifestEntry> {
        self.manifest.as_ref()?.file(number)
    }

    fn file_count(&self) -> usize {
        self.manifest.as_ref().map_or(0, Manifest::file_count)
    }

    pub(super) fn snapshot(&self) -> JobSnapshot {
        let (total_bytes, files_total, label) = self
            .manifest
            .as_ref()
            .map(|manifest| {
                // A snapshot is taken at every step: no walk of the entries.
                let count = manifest.file_count();
                let label = match (manifest.file(0), count) {
                    (None, _) => "Transfer".to_owned(),
                    (Some(first), 1) => first.path.clone(),
                    (Some(first), count) => format!("{} +{} more", first.path, count - 1),
                };
                (manifest.total_bytes(), count as u64, label)
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
            waiting_file: self
                .waiting_file
                .and_then(|index| Some(self.file(index)?.path.clone())),
            current_file: self.current_file(),
            error: self.error.clone(),
            destination: self.destination.as_ref().map(|landing| {
                // A chosen folder is what the manifest's paths lie under.
                crate::job::JobDestination {
                    root: landing.root,
                    folder: landing.folder.clone(),
                    items: landing
                        .names
                        .iter()
                        .map(|name| crate::job::JobItem {
                            name: name.clone(),
                            folder: self
                                .manifest
                                .as_ref()
                                .is_some_and(|manifest| manifest.has_folder(name)),
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
    /// Fetches stopped and perhaps still writing their partials, which the
    /// job's next fetch waits for.
    stopped: Vec<tokio::task::JoinHandle<Fetched>>,
}

/// How a file fetched ahead ended, and the connection it arrived on.
type Fetched = (Result<crate::transfer::Received, Error>, Option<usize>);

/// Files a job fetches ahead of the one it takes now, and their bytes at
/// most. With the file taken now, a job stays within a sender's serving
/// bound for one peer.
const PREFETCH_FILES: usize = 7;

const PREFETCH_BYTES: u64 = 64 * 1024 * 1024;

/// Stops a job's fetches ahead. A stopped fetch may still be writing its
/// partial, so it is kept until the job's next fetch has waited for it
/// ([`take_stopped_prefetch`]); no two fetches write one partial. Their
/// partials stay, and what they hold is checked again whenever it is used.
pub(super) fn stop_prefetch(shared: &Shared, id: u64) {
    let mut prefetches = lock(&shared.job_prefetch);
    let Some(prefetch) = prefetches.get_mut(&id) else {
        return;
    };
    if let Some(stop) = prefetch.stop.take() {
        crate::ignored!(
            crate::trace::FILES,
            "a fetch that already ended needs no stop",
            stop.send(true)
        );
    }
    let stopped = prefetch.files.drain().map(|(_, (_, fetching))| fetching);
    prefetch.stopped.extend(stopped);
}

/// The fetches of job `id` that were stopped, for its next fetch to wait for.
pub(super) fn take_stopped_prefetch(
    shared: &Shared,
    id: u64,
) -> Vec<tokio::task::JoinHandle<Fetched>> {
    lock(&shared.job_prefetch)
        .get_mut(&id)
        .map(|prefetch| std::mem::take(&mut prefetch.stopped))
        .unwrap_or_default()
}

/// Stops a job's fetches ahead and forgets them, when its partials go.
pub(super) fn forget_prefetch(shared: &Shared, id: u64) {
    stop_prefetch(shared, id);
    lock(&shared.job_prefetch).remove(&id);
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
/// root for legacy records. A replaced drive fails the leg: callers must
/// serve nothing on error. Runs under the roots lock together with the
/// resolution it guards, so the drives cannot be read again between check
/// and use.
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

// Retention: partials live under the transfer directory per job
// (`staging/job-{id}`), removed on Complete (verified publish), Failed and
// forget; retained on Cancelled/Interrupted for byte-reusing resume until
// the resume policy evicts them at the next load.
// Manifests bound each job's file count, free-space admission bounds its
// bytes and `MAX_ACTIVE_JOBS` bounds concurrency, so disk stays
// proportional to in-flight plus resumable work, not unbounded history.

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
/// shared root's handles, never retained absolute paths: each must still
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
            roots.verify_pin(pin).map_err(|_| fs::DRIVE_CHANGED)?;
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
    #[cfg(any(unix, windows))]
    {
        use vnilink_rename::anchored::AsDir;
        let parent = roots.open_dir_fd(root, parent_rel).ok()?;
        let st = vnilink_rename::anchored::stat_at(parent.as_dir(), name).ok()?;
        (st.is_file && !st.is_symlink).then_some(SourceIdentity {
            id: st.ino,
            size: st.size,
            mtime_ms: st.mtime_ms,
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
pub(super) const SENT_EVERY: u64 = 1024 * 1024;

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

/// Test probe parked by the serving tests: called with the serving
/// engine (`Core::engine_key`) and job after the source opens, before it
/// is authorized. Production leaves it unset. The probe must return
/// promptly for jobs it does not recognize: suites run in parallel.
pub static SERVE_SOURCE_PROBE: std::sync::Mutex<Option<ServeSourceProbe>> =
    std::sync::Mutex::new(None);

/// A [`SERVE_SOURCE_PROBE`]: the serving engine's key and the job.
pub type ServeSourceProbe = fn(usize, u64);

/// Opens file `file` of the peer's sending job `job` for serving, or says
/// which status refuses it. The source is reached through the leg's pinned
/// root by handles, never a retained absolute path, and must still be the
/// regular file scanned for the manifest: a file renamed over it, even
/// with the same size and time, is another file. A replaced drive refuses the request
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
        let entry = record.file(file as usize).ok_or(STATUS_DENIED)?.clone();
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
    // The file is served from the handle the anchored walk opened: on
    // Windows it is shared for reading only, so it is not written, renamed
    // or deleted while it is read.
    #[cfg(any(unix, windows))]
    let (opened, identity) = {
        use vnilink_rename::anchored::AsDir;
        let roots = lock(&shared.file_roots);
        verify_leg_pin(&roots, local_root, &mut pin).map_err(|_| STATUS_DENIED)?;
        let parent = roots
            .open_dir_fd(local_root, parent_rel)
            .map_err(|_| STATUS_CHANGED)?;
        let st =
            vnilink_rename::anchored::stat_at(parent.as_dir(), name).map_err(|_| STATUS_CHANGED)?;
        if !st.is_file || st.is_symlink {
            return Err(STATUS_CHANGED);
        }
        let fd =
            vnilink_rename::anchored::open_file_verified(parent.as_dir(), name, st.dev, st.ino)
                .map_err(|_| STATUS_CHANGED)?;
        let identity = SourceIdentity {
            id: st.ino,
            size: st.size,
            mtime_ms: st.mtime_ms,
        };
        (std::fs::File::from(fd), identity)
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
    let served = crate::transfer::Source {
        file: opened,
        size: entry.size,
        mtime_ms: entry.source_mtime_ms,
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

mod api;
mod driver;
use driver::*;
mod persist;
pub(super) use persist::*;
mod publish;
mod receiver;
mod sender;
mod serve;
pub(super) use serve::*;

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
        crate::test_support::connect_link(&controller, &host);
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
            manifest: Some(Manifest::new(vec![ManifestEntry {
                path: format!("file-{id}"),
                size,
                source_mtime_ms: 0,
                is_dir: false,
            }])),
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
            telling_resume: false,
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
        // The kit's drives: the folder stands for a drive, so replacing it
        // below is a drive taken out and another put in at its letter.
        lock(&core.shared.file_roots)
            .share_as_drives(&[scratch.join("root")])
            .unwrap();
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
        // The adopted pin is strict: another drive at the letter fails the
        // leg.
        lock(&core.shared.file_roots)
            .share_as_drives(&[scratch.join("elsewhere")])
            .unwrap();
        let err = core.receiver_dest(7, 0, "got", "file-7").unwrap_err();
        assert!(matches!(
            err,
            Error::InvalidSetting(msg) if msg == crate::fs::DRIVE_CHANGED
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
        let mut seen = before;
        while lock(&shared.jobs)[&7].snapshot().done_bytes != 400 {
            let now = wait_for_job_change(&shared, seen, std::time::Duration::from_secs(2));
            assert_ne!(now, seen, "arriving bytes never showed");
            seen = now;
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
        let core = Core::boot(
            crate::core::CoreConfig {
                listen: crate::core::Listen::Loopback,
                ..crate::core::CoreConfig::default()
            },
            iroh::RelayMode::Disabled,
        )
        .unwrap();
        crate::test_support::unattended(&core);
        core
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
            record.file(index).map(|entry| entry.path.clone())
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
