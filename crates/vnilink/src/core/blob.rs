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
use crate::session::Grant;

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

/// Per-chunk test probes for detached hashing. Passed by the caller (unit
/// tests) rather than installed globally, so concurrent suites never
/// observe them. Production passes [`HashHooks::none`].
#[derive(Clone, Copy, Default)]
pub(crate) struct HashHooks {
    /// Called before every 64 KiB chunk with the chunk index. A probe that
    /// parks (controlled slow reader) must also watch `cancelled` and
    /// return promptly once it is set.
    pub(crate) chunk_probe: Option<fn(u64, &std::sync::atomic::AtomicBool)>,
    /// Called once when the worker finishes; `true` reports cancellation.
    pub(crate) finish_probe: Option<fn(bool)>,
}

impl HashHooks {
    pub(crate) fn none() -> Self {
        Self::default()
    }
}

/// Dropping the hashing future cancels its detached blocking worker
/// between chunks; never rely on dropping the JoinHandle alone.
struct HashGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Drop for HashGuard {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// Streaming BLAKE3 over an open handle with cooperative cancellation.
/// Sync; runs on a blocking worker (async callers) or the caller's own
/// worker thread (sync offer APIs outside the runtime).
fn hash_fd_cancelled(
    mut file: std::fs::File,
    cancelled: &std::sync::atomic::AtomicBool,
    hooks: HashHooks,
) -> Result<[u8; 32], Error> {
    use std::io::Read;
    use std::sync::atomic::Ordering;
    let aborted = || -> Result<[u8; 32], Error> {
        if let Some(finish) = hooks.finish_probe {
            finish(true);
        }
        Err(Error::NoSession)
    };
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut chunk: u64 = 0;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return aborted();
        }
        if let Some(probe) = hooks.chunk_probe {
            probe(chunk, cancelled);
            // The probe may have waited out a cancellation.
            if cancelled.load(Ordering::Acquire) {
                return aborted();
            }
        }
        let n = file
            .read(&mut buf)
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        chunk += 1;
    }
    if let Some(finish) = hooks.finish_probe {
        finish(false);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// Windows counterpart of the Unix verified open: resolves `parent_rel/name`
/// under `root` and holds the file so its absolute path keeps naming this
/// object (ancestors pinned, file shared for reading only) while the result
/// lives. Callers hold the roots lock, as Unix holds it across its opens.
#[cfg(windows)]
pub(super) fn open_verified_source(
    roots: &crate::fs::RootSet,
    root: u64,
    parent_rel: &str,
    name: &str,
) -> Result<(vnilink_rename::windows::VerifiedSource, std::path::PathBuf), Error> {
    let absolute = roots
        .resolve_parent(root, parent_rel, name)
        .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?
        .1;
    let source = vnilink_rename::windows::open_source_verified(&absolute)
        .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
    Ok((source, absolute))
}

impl Core {
    /// Chooses the managed directory for blob storage: an absolute path,
    /// created the first time a transfer needs it rather than here. Saved
    /// jobs load from it when it already exists. Refused while a
    /// controlling session is active.
    pub fn set_blob_directory(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
        let state = self.alive_state()?;
        if state
            .incoming
            .values()
            .any(|session| !finished(&session.phase))
        {
            return Err(Error::Busy);
        }
        drop(state);
        let path = path.as_ref();
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(Error::InvalidSetting(BLOB_DIR_FAILED));
        };
        if !path.is_absolute() {
            return Err(Error::InvalidSetting(BLOB_DIR_FAILED));
        }
        // Canonical through the parent, which exists, so the path is stable
        // whether or not the directory itself has been created yet.
        let canonical = std::fs::canonicalize(parent)
            .map_err(|_| Error::InvalidSetting(BLOB_DIR_FAILED))?
            .join(name);
        if canonical.exists() && !canonical.is_dir() {
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
        let path = path.to_path_buf();
        self.runtime.block_on(offer_file_for_peer(
            &self.shared,
            peer,
            &path,
            max_bytes,
            false,
        ))
    }

    /// Anchored source import for job manifests: re-opens `root_rel`
    /// (root-relative, e.g. `docs/sub/a.txt`) through directory handles
    /// (`open_dir_fd` + `open_file_at` with `O_NOFOLLOW`) and imports via
    /// the open handle (`/proc/self/fd`/`/dev/fd`), never reopening the
    /// absolute pathname retained by the scan. A parent swapped for a
    /// symlink between scan and import fails here instead of disclosing an
    /// outside file.
    pub(crate) fn blob_offer_file_at(
        &self,
        peer_endpoint_id: &str,
        root: u64,
        file: &crate::manifest::ScannedFile,
        max_bytes: u64,
        pin: &crate::fs::PinnedRoot,
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
            roots.verify_pin(pin)?;
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
            // Resolve the import path under the short roots lock, then hash
            // with no locks held: whole-file reads never block root
            // operations.
            let parent_abs = roots
                .resolve_dir(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let absolute = parent_abs.join(name);
            drop(parent_fd);
            drop(roots);
            let fd_hash = Self::hash_fd(file_fd)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
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
        #[cfg(windows)]
        {
            let (parent_rel, name) = root_rel.rsplit_once('/').unwrap_or(("", root_rel));
            crate::fs::check_name(name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let (source, absolute) = {
                let roots = lock(&self.shared.file_roots);
                roots.verify_pin(pin)?;
                open_verified_source(&roots, root, parent_rel, name)?
            };
            if source.size() > max_bytes {
                return Err(Error::InvalidSetting("the source file is too large"));
            }
            // The scanned object (volume and file index), size and mtime, as on
            // Unix: a rename, replacement or edit since the scan fails here.
            if source.identity() != (file.dev, file.ino)
                || source.size() != file.size
                || source.mtime_ms() != file.mtime_ms
            {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            // Hash the held object, then reference its absolute path. While
            // `source` lives the path cannot be re-pointed, and the import hash
            // must still equal the handle hash.
            let handle_hash = Self::hash_fd(
                source
                    .file()
                    .try_clone()
                    .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?,
            )
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let size = source.size();
            let offered = self.runtime.block_on(async {
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
                if tag.hash().as_bytes() != &handle_hash {
                    return Err(Error::InvalidSetting(
                        "the source changed since preparation",
                    ));
                }
                lock(&self.shared.blob_job_offers).insert((peer, *tag.hash().as_bytes()));
                let hash = tag.hash();
                lock(&self.shared.blob_import_pins)
                    .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
                Ok((hash, size))
            });
            drop(source);
            offered
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (peer, root, file, max_bytes, pin);
            Err(Error::InvalidSetting("the source file cannot be read"))
        }
    }

    /// Hashes an already-open verified file handle via streaming BLAKE3
    /// (64 KiB chunks, no whole-file vectors). Used to preserve the validated
    /// source association through import: FD hash must match the absolute
    /// TryReference import hash, else the source changed/swapped between
    /// validation and import and the import fails without announcing.
    /// Sync; only for callers outside the runtime (sync offer APIs). Async
    /// callers use [`hash_fd_async`](Self::hash_fd_async) so whole-file
    /// reads never occupy a Tokio worker.
    pub(crate) fn hash_fd(source: impl Into<std::fs::File>) -> Result<[u8; 32], Error> {
        let never = std::sync::atomic::AtomicBool::new(false);
        hash_fd_cancelled(source.into(), &never, HashHooks::none())
    }

    /// Hashes an already-open verified handle on a bounded blocking worker
    /// (the Tokio blocking pool), checking for cancellation between 64 KiB
    /// chunks. Yields the async worker while hashing; dropping the future
    /// aborts the detached worker promptly instead of orphaning it.
    /// Callers must hold no locks across the await and must recheck
    /// owner/grant/liveness before using the digest.
    pub(crate) async fn hash_fd_async(
        source: impl Into<std::fs::File>,
        hooks: HashHooks,
    ) -> Result<[u8; 32], Error> {
        let file = source.into();
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _guard = HashGuard(cancelled.clone());
        let worker_cancelled = cancelled.clone();
        tokio::task::spawn_blocking(move || hash_fd_cancelled(file, &worker_cancelled, hooks))
            .await
            .map_err(|_| Error::NoSession)?
    }

    /// Re-offers for resume (no ScannedFile with dev/ino; old hash/size/mtime
    /// from the manifest authenticate): fd verification (open parent NOFOLLOW,
    /// stat regular/non-symlink/size+mtime match old), FD BLAKE3 hash must match
    /// old hash, then stable absolute TryReference import must hash to old hash,
    /// else source changed/swapped and the re-offer fails without serving.
    /// Sync (caller outside runtime); blocking hashing on caller worker thread.
    pub(crate) fn blob_reoffer_file_at(
        &self,
        peer_endpoint_id: &str,
        root: u64,
        root_rel: &str,
        old_hash: [u8; 32],
        old_size: u64,
        old_mtime_ms: u64,
        max_bytes: u64,
        pin: &crate::fs::PinnedRoot,
    ) -> Result<(iroh_blobs::Hash, u64), Error> {
        let peer = parse_endpoint_id(peer_endpoint_id)?;
        {
            let state = self.alive_state()?;
            state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        }
        if old_size > max_bytes {
            return Err(Error::InvalidSetting("the source file is too large"));
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
            roots.verify_pin(pin)?;
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
            // Resolve the import path under the short roots lock, then hash
            // with no locks held: whole-file reads never block root
            // operations.
            let parent_abs = roots
                .resolve_dir(root, parent_rel)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let absolute = parent_abs.join(name);
            drop(parent_fd);
            drop(roots);
            let fd_hash = Self::hash_fd(file_fd)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            if fd_hash != old_hash {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
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
        #[cfg(windows)]
        {
            let (parent_rel, name) = root_rel.rsplit_once('/').unwrap_or(("", root_rel));
            crate::fs::check_name(name)
                .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            let (source, absolute) = {
                let roots = lock(&self.shared.file_roots);
                roots.verify_pin(pin)?;
                open_verified_source(&roots, root, parent_rel, name)?
            };
            if source.size() != old_size || source.mtime_ms() != old_mtime_ms {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            let handle_hash = Self::hash_fd(
                source
                    .file()
                    .try_clone()
                    .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?,
            )
            .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?;
            if handle_hash != old_hash {
                return Err(Error::InvalidSetting(
                    "the source changed since preparation",
                ));
            }
            let offered = self.runtime.block_on(async {
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
                if tag.hash().as_bytes() != &handle_hash {
                    return Err(Error::InvalidSetting(
                        "the source changed since preparation",
                    ));
                }
                let state = self.alive_state()?;
                state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
                lock(&self.shared.file_roots).verify_pin(pin)?;
                let hash = tag.hash();
                lock(&self.shared.blob_job_offers).insert((peer, *hash.as_bytes()));
                lock(&self.shared.blob_import_pins)
                    .insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
                Ok((hash, old_size))
            });
            drop(source);
            offered
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (
                root,
                root_rel,
                old_hash,
                old_size,
                old_mtime_ms,
                max_bytes,
                pin,
            );
            Err(Error::InvalidSetting("the source file cannot be read"))
        }
    }

    /// Whether a job re-offer authorized `hash` for `peer_endpoint_id`.
    /// Tests use this to prove a rejected re-offer did not publish the
    /// replacement bytes.
    #[cfg(test)]
    pub(crate) fn blob_job_offer_contains(&self, peer_endpoint_id: &str, hash: &[u8; 32]) -> bool {
        let Ok(peer) = parse_endpoint_id(peer_endpoint_id) else {
            return false;
        };
        lock(&self.shared.blob_job_offers).contains(&(peer, *hash))
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
            let (mut connection, mut reused) = self.blob_connection(&peer, addr.clone()).await?;
            let batch = store
                .batch()
                .await
                .map_err(|e| Error::network(e.to_string()))?;
            let pin = batch
                .temp_tag((hash, BlobFormat::Raw))
                .await
                .map_err(|e| Error::network(e.to_string()))?;
            let mut outcome =
                fetch_with_cancel(&store, &connection, hash, progress.clone(), cancel.clone())
                    .await;
            // A kept connection may have ended without saying so yet; one
            // fresh connection settles whether the peer is really gone.
            while reused && outcome.is_err() {
                forget_blob_connection(&self.shared, &peer, &connection);
                (connection, reused) = self.blob_connection(&peer, addr.clone()).await?;
                outcome =
                    fetch_with_cancel(&store, &connection, hash, progress.clone(), cancel.clone())
                        .await;
            }
            if outcome.is_err() {
                forget_blob_connection(&self.shared, &peer, &connection);
            }
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
            outcome
        })
    }

    /// The peer's kept fetching connection while it is open, or a new
    /// one that is kept for the next fetch. The flag says it was kept.
    async fn blob_connection(
        &self,
        peer: &[u8; 32],
        addr: EndpointAddr,
    ) -> Result<(iroh::endpoint::Connection, bool), Error> {
        if let Some(kept) = lock(&self.shared.blob_fetching).get(peer) {
            if kept.close_reason().is_none() {
                return Ok((kept.clone(), true));
            }
        }
        let connection = tokio::time::timeout(Duration::from_secs(30), async {
            self.shared.endpoint.connect(addr, BLOB_ALPN).await
        })
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
        if let Some(old) = lock(&self.shared.blob_fetching).insert(*peer, connection.clone()) {
            if old.stable_id() != connection.stable_id() {
                old.close(0u8.into(), b"done");
            }
        }
        Ok((connection, false))
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

/// Hashes one file into the store and authorizes one peer to fetch it.
/// Admission is re-fenced after the slow import: a revoke or shutdown that
/// lands mid-hash must not leave an offer behind. Paste preparation sets
/// `transfer` to require the Transfer grant; plain offers require pairing.
pub(super) async fn offer_file_for_peer(
    shared: &Shared,
    peer: [u8; 32],
    path: &std::path::Path,
    max_bytes: u64,
    transfer: bool,
) -> Result<(iroh_blobs::Hash, u64), Error> {
    {
        let state = lock(&shared.state);
        if state.shut_down {
            return Err(Error::ShutDown);
        }
        state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        if transfer && !state.trust.granted(&peer, Grant::Transfer) {
            return Err(Error::TransferDenied);
        }
    }
    let size = std::fs::metadata(path)
        .map_err(|_| Error::InvalidSetting("the source file cannot be read"))?
        .len();
    if size > max_bytes {
        return Err(Error::InvalidSetting("the source file is too large"));
    }
    let store = open_store(shared).await?;
    let tag = store
        .add_path(path)
        .temp_tag()
        .await
        .map_err(|error| Error::network(error.to_string()))?;
    {
        let state = lock(&shared.state);
        if state.shut_down {
            return Err(Error::ShutDown);
        }
        state.trust.get(&peer).cloned().ok_or(Error::NotPaired)?;
        if transfer && !state.trust.granted(&peer, Grant::Transfer) {
            return Err(Error::TransferDenied);
        }
        lock(&shared.blob_offers).insert((peer, *tag.hash().as_bytes()));
    }
    let hash = tag.hash();
    lock(&shared.blob_import_pins).insert(*hash.as_bytes(), (std::time::Instant::now(), tag));
    Ok((hash, size))
}

/// The managed directory, created the first time a transfer needs it.
pub(super) fn blob_directory(shared: &Shared) -> Result<std::path::PathBuf, Error> {
    let dir = lock(&shared.blob_dir)
        .clone()
        .ok_or(Error::Unsupported(NO_BLOB_DIR))?;
    std::fs::create_dir_all(&dir).map_err(|_| Error::InvalidSetting(BLOB_DIR_FAILED))?;
    Ok(dir)
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
    let dir = blob_directory(shared)?;
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
    let retention_policy = shared.file_retention_policy.clone();
    options.gc = Some(iroh_blobs::store::GcConfig {
        interval: Duration::from_secs(if cfg!(test) { 2 } else { 30 }),
        add_protected: Some(Arc::new(move |live| {
            let policy = *lock(&retention_policy);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let records = lock(&jobs);
            let retained = super::jobs::retained_hashes(&records, policy, now);
            let retained_jobs = super::jobs::retained_job_ids(&records, policy, now);
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
            recent.retain(|_, (created, _)| {
                policy.recent_fetch_bytes != 0
                    && created.elapsed() < Duration::from_secs(policy.recent_fetch_age_secs)
            });
            let mut entries: Vec<_> = recent
                .iter()
                .map(|(hash, (created, size))| (*hash, *created, *size))
                .collect();
            entries.sort_by_key(|(_, created, _)| std::cmp::Reverse(*created));
            let mut budget = policy.recent_fetch_bytes;
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
    #[cfg(any(unix, windows))]
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
            #[cfg(any(unix, windows))]
            {
                source = Some(
                    super::jobs::verify_serve_source(shared, &peer, get.hash.as_bytes())
                        .await
                        .map_err(|_| ())?,
                );
            }
        }
    }
    #[cfg(any(unix, windows))]
    if let Some(source) = source {
        // Provider reads the verified open object for the entire request,
        // never a re-pointable pathname. The private store references the
        // held source only while it is alive, then removes all metadata.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = blob_directory(shared).map_err(|_| ())?;
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
        let path = source.path().to_owned();
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
        drop(source);
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
    if let Some(fetching) = lock(&shared.blob_fetching).remove(peer) {
        fetching.close(0u8.into(), b"denied");
    }
}

/// Stops reusing `connection` for `peer` after a fetch over it failed.
fn forget_blob_connection(
    shared: &Shared,
    peer: &[u8; 32],
    connection: &iroh::endpoint::Connection,
) {
    let mut fetching = lock(&shared.blob_fetching);
    if fetching
        .get(peer)
        .is_some_and(|kept| kept.stable_id() == connection.stable_id())
    {
        fetching.remove(peer);
    }
    drop(fetching);
    connection.close(0u8.into(), b"failed");
}

/// True when the connection negotiated the blob ALPN.
pub(super) fn is_blob_connection(connection: &iroh::endpoint::Connection) -> bool {
    connection.alpn() == BLOB_ALPN
}

/// Type alias so `core/mod.rs` can name the offer set.
pub(super) type OfferSet = HashSet<([u8; 32], [u8; 32])>;

#[cfg(all(test, unix))]
mod hash_detach_tests {
    use super::*;
    use std::os::unix::io::AsFd;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};

    static PARKED: AtomicBool = AtomicBool::new(false);
    static FINISHED: Mutex<Option<bool>> = Mutex::new(None);
    static FINISH_CV: Condvar = Condvar::new();

    fn reset() {
        PARKED.store(false, Ordering::SeqCst);
        *FINISHED.lock().unwrap() = None;
    }

    /// Controlled slow reader: parks the first chunk until cancellation
    /// arrives. The test never releases this barrier: sibling progress
    /// and worker cancellation must both happen while it is held.
    fn chunk_probe(_chunk: u64, cancelled: &AtomicBool) {
        if PARKED.swap(true, Ordering::SeqCst) {
            return;
        }
        while !cancelled.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn finish_probe(cancelled: bool) {
        *FINISHED.lock().unwrap() = Some(cancelled);
        FINISH_CV.notify_all();
    }

    fn scratch_file(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("vnilink-hash-{}-{tag}", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// STD04: detached hashing yields the executor (a sibling future
    /// completes while the first chunk is parked) and dropping the future
    /// aborts the worker without releasing the storage barrier.
    #[test]
    fn parked_hash_yields_sibling_and_cancels_without_release() {
        reset();
        let path = scratch_file("park", b"slow reader payload");
        let fd = std::fs::File::open(&path).unwrap();
        // `File::open` borrows the fd; the hash worker needs ownership.
        let owned = fd.as_fd().try_clone_to_owned().unwrap();
        drop(fd);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let hooks = HashHooks {
            chunk_probe: Some(chunk_probe),
            finish_probe: Some(finish_probe),
        };
        let (handle_tx, handle_rx) = std::sync::mpsc::channel();
        let (sibling_tx, sibling_rx) = std::sync::mpsc::channel();
        // A current-thread runtime only drives tasks under `block_on`; the
        // driver thread owns it while the test observes from outside. Loop
        // caps only free a driver orphaned by a failed test.
        let driver = std::thread::spawn(move || {
            runtime.block_on(async move {
                let hash = tokio::spawn(Core::hash_fd_async(owned, hooks));
                handle_tx.send(hash).unwrap();
                let cap = std::time::Instant::now() + std::time::Duration::from_secs(30);
                while !PARKED.load(Ordering::SeqCst) {
                    if std::time::Instant::now() > cap {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
                // Spawned only after the park is observed: it must complete
                // while the hash stays parked.
                tokio::spawn(async move {
                    let _ = sibling_tx.send(());
                });
                while FINISHED.lock().unwrap().is_none() {
                    if std::time::Instant::now() > cap {
                        return;
                    }
                    tokio::task::yield_now().await;
                }
            });
        });
        // The worker must reach the parked chunk; on the old inline path no
        // probe exists and this times out.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !PARKED.load(Ordering::SeqCst) {
            assert!(
                std::time::Instant::now() < deadline,
                "hash worker never parked"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Sibling progress while the hash is parked: the async worker is
        // free, so this completes without releasing the barrier.
        sibling_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("sibling blocked during parked hash");
        // Cancellation aborts the worker without releasing the barrier.
        let hash = handle_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        hash.abort();
        let finished = FINISHED.lock().unwrap();
        let (finished, timeout) = FINISH_CV
            .wait_timeout_while(finished, std::time::Duration::from_secs(5), |done| {
                done.is_none()
            })
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "hash worker never finished after abort"
        );
        assert_eq!(
            *finished,
            Some(true),
            "hash worker did not report cancellation"
        );
        drop(finished);
        driver.join().expect("driver thread panicked");
        std::fs::remove_file(&path).unwrap();
    }

    /// The detached digest matches the sync digest on identical bytes.
    #[test]
    fn detached_hash_matches_sync_digest() {
        let bytes = vec![7u8; 200 * 1024 + 13];
        let path = scratch_file("digest", &bytes);
        let expected = Core::hash_fd(
            std::fs::File::open(&path)
                .unwrap()
                .as_fd()
                .try_clone_to_owned()
                .unwrap(),
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let owned = std::fs::File::open(&path)
            .unwrap()
            .as_fd()
            .try_clone_to_owned()
            .unwrap();
        let observed = runtime
            .block_on(Core::hash_fd_async(owned, HashHooks::none()))
            .unwrap();
        assert_eq!(observed, expected);
        assert_eq!(observed, *blake3::hash(&bytes).as_bytes());
        std::fs::remove_file(&path).unwrap();
    }
}
