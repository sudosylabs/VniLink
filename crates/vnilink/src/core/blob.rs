//! Authenticated blob transfers (Stages A and C).
//!
//! The stock `iroh-blobs` provider serves every hash in its store to every
//! peer that dials it. A blob hash is a content address, not authorization,
//! so this module gates serving on a per-peer allowlist: a hash is served
//! to a peer only after this installation offered it there, and only while
//! the transfer grant is still on. Push, observe and multi-get requests are
//! never served; each file moves as one raw blob.
//!
//! The store is a persistent [`FsStore`] under the managed blob directory,
//! so validated ranges survive process restart and resume reuses them.
//! The whole-bytes `blob_fetch` API stays capped at [`BLOB_MAX_BYTES`];
//! larger files move through the export path without whole-file vectors.

use super::*;
use iroh_blobs::api::blobs::AddPathOptions;
use iroh_blobs::api::proto::ImportMode;
use iroh_blobs::BlobFormat;

use std::collections::HashSet;
use std::time::Duration;

use iroh_blobs::provider::{handle_get, StreamPair};
use iroh_blobs::store::fs::FsStore;

use crate::error::Error;

/// ALPN for blob connections. Control stays on `vnilink/1`.
pub const BLOB_ALPN: &[u8] = b"vnilink/blob/1";

/// Largest single blob the whole-bytes API moves: 32 MiB. Above the legacy
/// 8 MiB cap, small enough to keep whole-file vectors bounded. Larger files
/// move through the streaming export path under manifest limits.
pub const BLOB_MAX_BYTES: usize = 32 * 1024 * 1024;

const BLOB_OFFER_TOO_BIG: &str = "the blob is larger than 32 MiB";
const BLOB_SIZE_MISMATCH: &str = "the received blob size does not match the offer";
const NO_BLOB_DIR: &str = "no blob directory is installed";
const BLOB_DIR_FAILED: &str = "the blob directory could not be created";
pub(crate) const FETCH_CANCELLED: &str = "the fetch was cancelled";

/// How a streamed fetch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchResult {
    /// The blob is complete in the local store; carries the payload bytes
    /// read from the peer during this call. Below the expected size after
    /// an interruption proves validated ranges were reused.
    Done {
        downloaded: u64,
    },
    Cancelled,
}

impl Core {
    /// Chooses the managed directory for blob storage. Created when
    /// missing. Refused while a controlling session is active.
    pub fn set_blob_directory(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .as_ref()
            .is_some_and(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        let path = path.as_ref();
        std::fs::create_dir_all(path).map_err(|_| Error::InvalidSetting(BLOB_DIR_FAILED))?;
        let canonical =
            std::fs::canonicalize(path).map_err(|_| Error::InvalidSetting(BLOB_DIR_FAILED))?;
        if !canonical.is_dir() {
            return Err(Error::InvalidSetting(BLOB_DIR_FAILED));
        }
        *lock(&self.shared.blob_dir) = Some(canonical);
        *lock(&self.shared.blob_store) = None;
        super::jobs::load_jobs(&self.shared);
        Ok(())
    }

    /// Imports `data` into the store and authorizes one paired peer to
    /// fetch its hash. The transfer grant is enforced when the peer dials,
    /// not here.
    pub fn blob_offer(
        &self,
        peer_endpoint_id: &str,
        data: &[u8],
    ) -> Result<(iroh_blobs::Hash, u64), Error> {
        if data.len() > BLOB_MAX_BYTES {
            return Err(Error::InvalidSetting(BLOB_OFFER_TOO_BIG));
        }
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        let owned = data.to_vec();
        self.runtime.block_on(async {
            let store = open_store(&self.shared).await?;
            let tag = store
                .add_slice(&owned)
                .temp_tag()
                .await
                .map_err(|error| Error::network(error.to_string()))?;
            lock(&self.shared.blob_offers).insert((peer, *tag.hash().as_bytes()));
            let hash = tag.hash();
            lock(&self.shared.blob_import_pins)
                .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
            Ok((hash, owned.len() as u64))
        })
    }

    /// Streams one file from disk into the store without holding it in
    /// memory and authorizes one paired peer to fetch it. `max_bytes`
    /// bounds the import; the manifest limits own the policy value.
    pub fn blob_offer_file(
        &self,
        peer_endpoint_id: &str,
        path: &std::path::Path,
        max_bytes: u64,
    ) -> Result<(iroh_blobs::Hash, u64), Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        let size = std::fs::metadata(path)
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?
            .len();
        if size > max_bytes {
            return Err(Error::InvalidSetting("the source file is too large"));
        }
        let path = path.to_path_buf();
        self.runtime.block_on(async {
            let store = open_store(&self.shared).await?;
            let tag = store
                .add_path(&path)
                .temp_tag()
                .await
                .map_err(|error| Error::network(error.to_string()))?;
            lock(&self.shared.blob_offers).insert((peer, *tag.hash().as_bytes()));
            let hash = tag.hash();
            lock(&self.shared.blob_import_pins)
                .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
            Ok((hash, size))
        })
    }

    /// Anchored source import for job manifests: re-opens `root_rel`
    /// (root-relative, e.g. `docs/sub/a.txt`) through directory handles
    /// (`open_dir_fd` + `open_file_at` with `O_NOFOLLOW`) and imports via
    /// the open handle (`/proc/self/fd`/`/dev/fd`), never reopening the
    /// absolute pathname retained by the scan. A parent swapped for a
    /// symlink between scan and import fails here instead of disclosing an
    /// outside file.
    pub fn blob_offer_file_at(
        &self,
        peer_endpoint_id: &str,
        root: u64,
        file: &crate::manifest::ScannedFile,
        max_bytes: u64,
    ) -> Result<(iroh_blobs::Hash, u64), Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        let root_rel = file.root_rel.as_str();
        crate::fs::check_rel(root_rel)
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
        if root_rel.is_empty() {
            return Err(Error::InvalidSetting("the source file cannot be read"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsFd;
            let (parent_rel, name) = match root_rel.rfind('/') {
                Some(slash) => (&root_rel[..slash], &root_rel[slash + 1..]),
                None => ("", root_rel),
            };
            crate::fs::check_name(name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let roots = lock(&self.shared.file_roots);
            let parent_fd = roots
                .open_dir_fd(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            if !st.is_file || st.is_symlink {
                return Err(Error::InvalidSetting("the source file cannot be read"));
            }
            if st.size > max_bytes {
                return Err(Error::InvalidSetting("the source file is too large"));
            }
            // Preserve validated source association (scan vs import): same object
            // (dev/ino), size and mtime. Rename/replacement/modification during
            // preparation fails here without announcing.
            if st.dev != file.dev
                || st.ino != file.ino
                || st.size != file.size
                || st.mtime_ms != file.mtime_ms
            {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            // Re-open and verify same object (dev+ino); refuses swaps between
            // stat and open without any unsafe code in this crate. Hash through
            // the validated handle to preserve source association, then import
            // by stable absolute path with TryReference (no second full copy
            // when the store can reference). The import hash must match the FD
            // hash, else the source changed/swapped between validation and
            // import and the import fails without announcing. Temp fd paths are
            // never persisted as references.
            let file_fd = vnilink_rename::anchored::open_file_verified(
                parent_fd.as_fd(),
                name,
                st.dev,
                st.ino,
            )
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let fd_hash = Self::hash_fd(file_fd)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let parent_abs = roots
                .resolve_dir(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let absolute = parent_abs.join(name);
            drop(parent_fd);
            drop(roots);
            let size = st.size;
            self.runtime.block_on(async {
                let store = open_store(&self.shared).await?;
                let tag = store
                    .add_path_with_opts(AddPathOptions {
                        path: absolute,
                        format: BlobFormat::Raw,
                        mode: ImportMode::TryReference,
                    })
                    .temp_tag()
                    .await
                    .map_err(|error| Error::network(error.to_string()))?;
                if tag.hash().as_bytes() != &fd_hash {
                    return Err(Error::InvalidSetting(
                        "the source changed since preparation",
                    ));
                }
                lock(&self.shared.blob_job_offers).insert((peer, *tag.hash().as_bytes()));
                let hash = tag.hash();
                lock(&self.shared.blob_import_pins)
                    .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
                Ok((hash, size))
            })
        }
        #[cfg(not(unix))]
        {
            let roots = lock(&self.shared.file_roots);
            let parent = roots
                .resolve_dir(root, {
                    match root_rel.rfind('/') {
                        Some(slash) => &root_rel[..slash],
                        None => "",
                    }
                })
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            drop(roots);
            let name = match root_rel.rfind('/') {
                Some(slash) => &root_rel[slash + 1..],
                None => root_rel,
            };
            let full = parent.join(name);
            drop(parent);
            // Non-Unix job fallback: pathname import (residual race documented),
            // but accounted as a job offer (active-job gated), not legacy.
            let (hash, size) = self.blob_offer_file(peer_endpoint_id, &full, max_bytes)?;
            // `blob_offer_file` inserted into legacy offers; move to job offers.
            lock(&self.shared.blob_offers).remove(&(peer, *hash.as_bytes()));
            lock(&self.shared.blob_job_offers).insert((peer, *hash.as_bytes()));
            Ok((hash, size))
        }
    }

    /// Hashes an already-open verified file handle via streaming BLAKE3
    /// (64 KiB chunks, no whole-file vectors). Used to preserve the validated
    /// source association through import: FD hash must match the absolute
    /// TryReference import hash, else the source changed/swapped between
    /// validation and import and the import fails without announcing.
    #[cfg(unix)]
    pub(crate) fn hash_fd(fd: std::os::unix::io::OwnedFd) -> Result<[u8; 32], Error> {
        use std::io::Read;
        #[cfg(unix)]
        {
            let mut file = std::fs::File::from(fd);
            let mut hasher = blake3::Hasher::new();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = file
                    .read(&mut buf)
                    .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            Ok(*hasher.finalize().as_bytes())
        }
        #[cfg(not(unix))]
        {
            let _ = fd;
            Err(Error::InvalidSetting("the source file cannot be read"))
        }
    }

    /// Re-offers for resume (no ScannedFile with dev/ino; old hash/size/mtime
    /// from the manifest authenticate): fd verification (open parent NOFOLLOW,
    /// stat regular/non-symlink/size+mtime match old), FD BLAKE3 hash must match
    /// old hash, then stable absolute TryReference import must hash to old hash,
    /// else source changed/swapped and the re-offer fails without serving.
    /// Sync (caller outside runtime); blocking hashing on caller worker thread.
    pub fn blob_reoffer_file_at(
        &self,
        peer_endpoint_id: &str,
        root: u64,
        root_rel: &str,
        old_hash: [u8; 32],
        old_size: u64,
        old_mtime_ms: u64,
        max_bytes: u64,
    ) -> Result<(iroh_blobs::Hash, u64), Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        crate::fs::check_rel(root_rel)
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
        if root_rel.is_empty() {
            return Err(Error::InvalidSetting("the source file cannot be read"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::io::AsFd;
            let (parent_rel, name) = match root_rel.rfind('/') {
                Some(slash) => (&root_rel[..slash], &root_rel[slash + 1..]),
                None => ("", root_rel),
            };
            crate::fs::check_name(name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let roots = lock(&self.shared.file_roots);
            let parent_fd = roots
                .open_dir_fd(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let st = vnilink_rename::anchored::stat_at(parent_fd.as_fd(), name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            if !st.is_file || st.is_symlink || st.size != old_size || st.mtime_ms != old_mtime_ms {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            let file_fd = vnilink_rename::anchored::open_file_verified(
                parent_fd.as_fd(),
                name,
                st.dev,
                st.ino,
            )
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let fd_hash = Self::hash_fd(file_fd)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            if fd_hash != old_hash {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            let parent_abs = roots
                .resolve_dir(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let absolute = parent_abs.join(name);
            drop(parent_fd);
            drop(roots);
            self.runtime.block_on(async {
                let store = open_store(&self.shared).await?;
                let tag = store
                    .add_path_with_opts(AddPathOptions {
                        path: absolute,
                        format: BlobFormat::Raw,
                        mode: ImportMode::TryReference,
                    })
                    .temp_tag()
                    .await
                    .map_err(|error| Error::network(error.to_string()))?;
                if tag.hash().as_bytes() != &fd_hash || tag.hash().as_bytes() != &old_hash {
                    return Err(Error::InvalidSetting(
                        "the source changed since preparation",
                    ));
                }
                lock(&self.shared.blob_job_offers).insert((peer, *tag.hash().as_bytes()));
                let hash = tag.hash();
                lock(&self.shared.blob_import_pins)
                    .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
                Ok((hash, old_size))
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (root, root_rel, old_hash, old_size, old_mtime_ms, max_bytes);
            Err(Error::InvalidSetting("the source file cannot be read"))
        }
    }

    /// Fetches `hash` from one paired peer into the local store and
    /// returns its bytes. Capped at [`BLOB_MAX_BYTES`]; larger blobs use
    /// [`Core::blob_fetch_streamed`] plus [`Core::blob_export`].
    pub fn blob_fetch(
        &self,
        peer_endpoint_id: &str,
        hash: iroh_blobs::Hash,
        expected_size: u64,
    ) -> Result<Vec<u8>, Error> {
        if expected_size > BLOB_MAX_BYTES as u64 {
            return Err(Error::InvalidSetting(BLOB_OFFER_TOO_BIG));
        }
        match self.blob_fetch_streamed(peer_endpoint_id, hash, None, None)? {
            FetchResult::Done { .. } => {}
            FetchResult::Cancelled => return Err(Error::InvalidSetting(FETCH_CANCELLED)),
        }
        self.runtime.block_on(async {
            let store = open_store(&self.shared).await?;
            let bytes = store
                .get_bytes(hash)
                .await
                .map_err(|error| Error::network(error.to_string()))?;
            if bytes.len() as u64 != expected_size {
                return Err(Error::InvalidSetting(BLOB_SIZE_MISMATCH));
            }
            Ok(bytes.to_vec())
        })
    }

    /// Fetches `hash` into the local persistent store and reports how many
    /// payload bytes the peer sent during this call. The data stays in the
    /// store; the caller exports or reads it afterwards. Partial ranges
    /// verified in earlier calls are reused by the blob protocol, so a
    /// resumed fetch reads fewer bytes than the full size.
    pub fn blob_fetch_streamed(
        &self,
        peer_endpoint_id: &str,
        hash: iroh_blobs::Hash,
        progress: Option<std::sync::mpsc::Sender<u64>>,
        cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<FetchResult, Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        let device = {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?
        };
        // Capture ownership before the fetch: Forget may remove the job
        // while cancellation is settling, and must not create a new cache lease.
        let job_owned = lock(&self.shared.jobs)
            .values()
            .any(|record| super::jobs::record_hashes(record).contains(hash.as_bytes()));
        let addr = device_addr(&device)?;
        self.runtime.block_on(async {
            let store = open_store(&self.shared).await?;
            let connection = tokio::time::timeout(Duration::from_secs(30), async {
                self.shared.endpoint.connect(addr, BLOB_ALPN).await
            })
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(Error::network_debug)?;
            let batch = store
                .batch()
                .await
                .map_err(|e| Error::network(e.to_string()))?;
            let pin = batch
                .temp_tag((hash, BlobFormat::Raw))
                .await
                .map_err(|e| Error::network(e.to_string()))?;
            let outcome = fetch_with_cancel(&store, &connection, hash, progress, cancel).await;
            // Standalone blob callers have no durable job. Give them a
            // bounded export/retry window; job payloads use their manifest.
            if !job_owned {
                let size = match store
                    .status(hash)
                    .await
                    .map_err(|e| Error::network(e.to_string()))?
                {
                    iroh_blobs::api::blobs::BlobStatus::Complete { size } => size,
                    iroh_blobs::api::blobs::BlobStatus::Partial { size } => size.unwrap_or(0),
                    _ => 0,
                };
                lock(&self.shared.blob_recent_fetches)
                    .insert(*hash.as_bytes(), (std::time::Instant::now(), size));
            }
            drop(pin);
            drop(batch);
            connection.close(0u8.into(), b"done");
            outcome
        })
    }

    /// Exports one stored blob to `dest` without whole-file vectors and
    /// returns the exported size.
    pub fn blob_export(
        &self,
        hash: iroh_blobs::Hash,
        dest: &std::path::Path,
    ) -> Result<u64, Error> {
        self.runtime.block_on(async {
            let store = open_store(&self.shared).await?;
            store
                .export(hash, dest)
                .await
                .map_err(|error| Error::network(error.to_string()))
        })
    }

    /// Closes the blob store gracefully, waiting briefly for its
    /// actors to settle. The store reopens cleanly afterwards; without
    /// this, reopening races the asynchronous teardown (see
    /// `open_store`). Called by `Core::shutdown` and by tests that
    /// reopen one directory in-process.
    pub(crate) fn blob_close_store(&self) {
        lock(&self.shared.blob_import_pins).clear();
        let store = lock(&self.shared.blob_store).take();
        if let Some(store) = store {
            self.runtime.block_on(async {
                let _ = store.shutdown().await;
            });
        }
    }
    /// True when the hash is already complete in the local store.
    /// Never opens the store: absent storage reads as absent data.
    pub fn blob_has(&self, hash: iroh_blobs::Hash) -> Result<bool, Error> {
        let Some(store) = lock(&self.shared.blob_store).clone() else {
            return Ok(false);
        };
        self.runtime.block_on(async {
            store
                .has(hash)
                .await
                .map_err(|error| Error::network(error.to_string()))
        })
    }

    /// Verified bytes currently stored for `hash`: `None` when absent,
    /// `Some(size)` for partial progress, `Some(full)` when complete.
    /// The native queue uses this for honest progress; tests gate
    /// deterministic interruption on verified presence rather than on
    /// bytes merely read.
    pub fn blob_verified(&self, hash: iroh_blobs::Hash) -> Result<Option<u64>, Error> {
        let Some(store) = lock(&self.shared.blob_store).clone() else {
            return Ok(None);
        };
        self.runtime.block_on(async {
            let status = store
                .status(hash)
                .await
                .map_err(|error| Error::network(error.to_string()))?;
            Ok(match status {
                iroh_blobs::api::blobs::BlobStatus::NotFound => None,
                iroh_blobs::api::blobs::BlobStatus::Partial { size } => size.or(Some(0)),
                iroh_blobs::api::blobs::BlobStatus::Complete { size } => Some(size),
            })
        })
    }
}

async fn fetch_with_cancel(
    store: &FsStore,
    connection: &iroh::endpoint::Connection,
    hash: iroh_blobs::Hash,
    progress: Option<std::sync::mpsc::Sender<u64>>,
    cancel: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<FetchResult, Error> {
    use n0_future::StreamExt;
    let mut updates = store.remote().fetch(connection.clone(), hash).stream();
    let mut cancel = cancel;
    // A pre-cancelled call never dials into work.
    if cancel.as_ref().is_some_and(|stop| *stop.borrow()) {
        return Ok(FetchResult::Cancelled);
    }
    loop {
        let next = if let Some(stop) = cancel.as_mut() {
            tokio::select! {
                item = updates.next() => Some(item),
                _ = stop.changed() => None,
            }
        } else {
            updates.next().await.map(Some)
        };
        match next {
            // Cancellation fired: the fetch future is dropped with the
            // stream, verified ranges stay in the persistent store.
            None => return Ok(FetchResult::Cancelled),
            Some(Some(item)) => match item {
                iroh_blobs::api::remote::GetProgressItem::Progress(bytes) => {
                    if let Some(tx) = &progress {
                        let _ = tx.send(bytes);
                    }
                }
                iroh_blobs::api::remote::GetProgressItem::Done(stats) => {
                    return Ok(FetchResult::Done {
                        downloaded: stats.payload_bytes_read,
                    });
                }
                iroh_blobs::api::remote::GetProgressItem::Error(error) => {
                    return Err(Error::network(error.to_string()));
                }
            },
            Some(None) => return Err(Error::network("the blob stream ended early")),
        }
    }
}

pub(super) async fn open_store(shared: &Shared) -> Result<FsStore, Error> {
    if let Some(store) = lock(&shared.blob_store).clone() {
        return Ok(store);
    }
    // Only one task may open the store: concurrent opens of one
    // directory race its lock and hang each other past any timeout.
    let _opening = shared.blob_open.lock().await;
    if let Some(store) = lock(&shared.blob_store).clone() {
        return Ok(store);
    }
    let dir = lock(&shared.blob_dir)
        .clone()
        .ok_or(Error::Unsupported(NO_BLOB_DIR))?;
    // The upstream store shuts down asynchronously: reopening a directory
    // whose previous store is still winding down races its lock, so a
    // fresh load can stall. Bounded retries ride that out; steady-state
    // opens succeed on the first try. Genuine open errors are not retried.
    let mut last = Error::network("the blob store timed out opening");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("serve-") {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
    let mut options = iroh_blobs::store::fs::options::Options::new(&dir);
    options.inline.max_data_inlined = 0;
    let jobs = shared.jobs.clone();
    let offers = shared.blob_offers.clone();
    let pins = shared.blob_import_pins.clone();
    let recent = shared.blob_recent_fetches.clone();
    let staging_dir = dir.join("staging");
    options.gc = Some(iroh_blobs::store::GcConfig {
        interval: Duration::from_secs(if cfg!(test) { 2 } else { 30 }),
        add_protected: Some(Arc::new(move |live| {
            let records = lock(&jobs);
            let retained = super::jobs::retained_hashes(&records);
            let retained_jobs = super::jobs::retained_job_ids(&records);
            // Expired/evicted resume data includes staging exports too.
            for (id, record) in records.iter() {
                if super::jobs::record_reclaimable(record, &retained_jobs) {
                    let _ = std::fs::remove_dir_all(staging_dir.join(format!("job-{id}")));
                }
            }
            let owned: HashSet<_> = records
                .values()
                .flat_map(super::jobs::record_hashes)
                .collect();
            drop(records);
            live.extend(retained.iter().copied());
            live.extend(
                lock(&offers)
                    .iter()
                    .map(|(_, hash)| iroh_blobs::Hash::from_bytes(*hash)),
            );
            // Import tags cover the gap before the manifest is installed.
            // Failed preparations expire; terminal jobs release their tags.
            lock(&pins).retain(|hash, (created, _)| {
                retained.contains(&iroh_blobs::Hash::from_bytes(*hash))
                    || (!owned.contains(hash) && created.elapsed() < Duration::from_secs(300))
            });
            let mut recent = lock(&recent);
            recent.retain(|_, (created, _)| created.elapsed() < Duration::from_secs(300));
            let mut entries: Vec<_> = recent
                .iter()
                .map(|(hash, (created, size))| (*hash, *created, *size))
                .collect();
            entries.sort_by_key(|(_, created, _)| std::cmp::Reverse(*created));
            let mut budget = 1024 * 1024 * 1024u64;
            for (hash, _, size) in entries {
                if size <= budget {
                    budget -= size;
                    live.insert(iroh_blobs::Hash::from_bytes(hash));
                } else {
                    recent.remove(&hash);
                }
            }
            Box::pin(async { iroh_blobs::store::ProtectOutcome::Continue })
        })),
    });
    for _ in 0..4 {
        match tokio::time::timeout(
            Duration::from_secs(10),
            FsStore::load_with_opts(dir.join("blobs.db"), options.clone()),
        )
        .await
        {
            Ok(Ok(store)) => {
                // Previous builds used automatically named tags, retaining
                // every import forever. This store belongs only to VniLink;
                // durable job records and current offers now own all pins.
                store
                    .tags()
                    .delete_all()
                    .await
                    .map_err(|e| Error::network(e.to_string()))?;
                *lock(&shared.blob_store) = Some(store.clone());
                return Ok(store);
            }
            Ok(Err(error)) => {
                last = Error::network(error.to_string());
                break;
            }
            Err(_) => {
                last = Error::network("the blob store timed out opening");
            }
        }
    }
    Err(last)
}

/// Serves one incoming blob connection. Each stream carries one blob
/// request; only single-blob gets for an allowlisted peer hash reach the
/// store. Anything else closes the connection without serving any bytes.
/// Tracked by peer so revoke/unpair closes in-flight serves instead of
/// letting an accepted request finish after its offers are removed.
pub(super) async fn serve_blob_connection(
    shared: std::sync::Arc<Shared>,
    connection: iroh::endpoint::Connection,
) {
    let peer = *connection.remote_id().as_bytes();
    lock(&shared.blob_serving)
        .entry(peer)
        .or_default()
        .push(connection.clone());
    loop {
        let accepted = connection.accept_bi().await;
        let Ok((send, recv)) = accepted else {
            break;
        };
        let peer = *connection.remote_id().as_bytes();
        if handle_blob_stream(&shared, &connection, peer, send, recv)
            .await
            .is_err()
        {
            break;
        }
    }
    // Unregister (retain others for the same peer, if any).
    {
        let mut serving = lock(&shared.blob_serving);
        if let Some(conns) = serving.get_mut(&peer) {
            conns.retain(|c| c.stable_id() != connection.stable_id());
            if conns.is_empty() {
                serving.remove(&peer);
            }
        }
    }
}

async fn handle_blob_stream(
    shared: &Shared,
    connection: &iroh::endpoint::Connection,
    peer: [u8; 32],
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
) -> Result<(), ()> {
    use iroh_blobs::protocol::Request as BlobRequest;

    let mut pair = StreamPair::new(
        connection.stable_id() as u64,
        recv,
        send,
        iroh_blobs::provider::events::EventSender::DEFAULT,
    );
    let request = pair.read_request().await.map_err(|_| ())?;
    let BlobRequest::Get(get) = request else {
        connection.close(0u8.into(), b"denied");
        return Err(());
    };
    enum Allow {
        Deny,
        Legacy,
        Job,
    }
    let allowed = {
        // Sequential, never nested: pairing + legacy/job offer sets first
        // (dropped), then active-job check (locks jobs separately). No
        // state->jobs nesting, so no inversion with publication's jobs->state.
        let endpoint_id = lock(&shared.state)
            .trust
            .get(&peer)
            .map(|device| device.endpoint_id.clone());
        match endpoint_id {
            None => Allow::Deny,
            Some(endpoint_id) => {
                if lock(&shared.blob_offers).contains(&(peer, *get.hash.as_bytes())) {
                    // Legacy blob API: pairing + offer + Transfer grant (no active-job
                    // check; spike/legacy paths have no jobs). Pairing alone never
                    // authorizes file bytes. Bytes already copied into the store via
                    // add_slice (no source file to re-verify).
                    if lock(&shared.state)
                        .trust
                        .granted(&endpoint_id, crate::session::Grant::Transfer)
                    {
                        Allow::Legacy
                    } else {
                        Allow::Deny
                    }
                } else if !lock(&shared.blob_job_offers).contains(&(peer, *get.hash.as_bytes())) {
                    Allow::Deny
                } else if super::jobs::is_active_serve(shared, &peer, get.hash.as_bytes()) {
                    Allow::Job
                } else {
                    Allow::Deny
                }
            }
        }
    };
    #[cfg(unix)]
    let mut source = None;
    match allowed {
        Allow::Deny => {
            connection.close(0u8.into(), b"denied");
            return Err(());
        }
        Allow::Legacy => {}
        Allow::Job => {
            if !lock(&shared.state)
                .trust
                .granted(&peer, crate::session::Grant::Transfer)
            {
                connection.close(0u8.into(), b"denied");
                return Err(());
            }
            // Serve-time source re-verification through handles (no absolute
            // reopen race): find the active sender entry for this hash,
            // reconstruct its root-relative source, open parent via FD and
            // stat the file (NOFOLLOW, regular, size matches). A swapped parent
            // or replaced file denies here, so TryReference absolute paths can
            // never disclose outside bytes even when the reference was taken
            // while honest.
            #[cfg(unix)]
            {
                source = Some(
                    super::jobs::verify_serve_source(shared, &peer, get.hash.as_bytes())
                        .map_err(|_| ())?,
                );
            }
        }
    }
    #[cfg(unix)]
    if let Some(fd) = source {
        // Provider reads the verified open object for the entire request,
        // never the mutable pathname. The private store references this FD
        // only while it is alive, then removes all metadata.
        use std::os::fd::AsRawFd;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = lock(&shared.blob_dir).clone().ok_or(())?;
        let dir = base.join(format!(
            "serve-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut options = iroh_blobs::store::fs::options::Options::new(&dir);
        options.inline.max_data_inlined = 0;
        let transient = FsStore::load_with_opts(dir.join("blobs.db"), options)
            .await
            .map_err(|_| ())?;
        #[cfg(target_os = "linux")]
        let path = std::path::PathBuf::from(format!("/proc/self/fd/{}", fd.as_raw_fd()));
        #[cfg(not(target_os = "linux"))]
        let path = std::path::PathBuf::from(format!("/dev/fd/{}", fd.as_raw_fd()));
        let result = async {
            let pin = transient
                .add_path_with_opts(AddPathOptions {
                    path,
                    format: BlobFormat::Raw,
                    mode: ImportMode::TryReference,
                })
                .temp_tag()
                .await
                .map_err(|_| ())?;
            if pin.hash() != get.hash {
                return Err(());
            }
            if !lock(&shared.state)
                .trust
                .granted(&peer, crate::session::Grant::Transfer)
                || !super::jobs::is_active_serve(shared, &peer, get.hash.as_bytes())
            {
                return Err(());
            }
            handle_get(pair, transient.as_ref().clone(), get)
                .await
                .map_err(|_| ())?;
            drop(pin);
            Ok(())
        }
        .await;
        let _ = transient.shutdown().await;
        drop(fd);
        let _ = std::fs::remove_dir_all(dir);
        return result;
    }
    let store = open_store(shared).await.map_err(|_| ())?;
    let api: iroh_blobs::api::Store = store.as_ref().clone();
    handle_get(pair, api, get).await.map_err(|_| ())?;
    Ok(())
}

/// Drops one peer's fetch authorizations (legacy + job) and closes its
/// in-flight blob serves. New blob connections from that peer are denied
/// afterwards; accepted requests cannot finish after revocation because their
/// connections close here (receivers also abort their fetches via the job
/// cancel watches). Verified ranges stay for resume.
pub(super) fn drop_blob_offers(shared: &Shared, peer: &[u8; 32]) {
    lock(&shared.blob_offers).retain(|(key, _)| key != peer);
    lock(&shared.blob_job_offers).retain(|(key, _)| key != peer);
    let conns = lock(&shared.blob_serving).remove(peer).unwrap_or_default();
    for conn in conns {
        conn.close(0u8.into(), b"denied");
    }
}

/// True when the connection negotiated the blob ALPN.
pub(super) fn is_blob_connection(connection: &iroh::endpoint::Connection) -> bool {
    connection.alpn() == BLOB_ALPN
}

/// Type alias so `core/mod.rs` can name the offer set.
pub(super) type OfferSet = HashSet<([u8; 32], [u8; 32])>;
