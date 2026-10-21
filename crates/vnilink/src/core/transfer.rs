//! The engine side of `vnilink/files/1` (see [`crate::transfer`]):
//! connections, which job file a peer may read, and the transfer
//! directory.
//!
//! The receiver keeps one files connection per peer for the files that
//! follow. Serving finds a request by the sending job and the file's place
//! in its manifest, so only a live sending job for that peer, under the
//! Transfer grant, serves anything, and only the file the manifest
//! describes (`jobs::open_served_file`).

use super::*;

use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream};

use crate::error::Error;
use crate::transfer::{self, Answer, FileRequest, Partial, Received, FILES_ALPN};

const DIAL_WITHIN: Duration = Duration::from_secs(30);
/// Files served at once, to one peer and in all. Each holds a few chunks
/// read ahead, so these bound what peers can make this computer hold.
const SERVE_PER_PEER: usize = 8;
const SERVE_IN_ALL: usize = 32;
/// How long a request waits for a serving slot before it is told busy.
const SLOT_WITHIN: Duration = Duration::from_secs(5);

/// Bounds the files this computer serves at once, per peer and in all,
/// so no peer can make it read and buffer without limit.
pub(super) struct ServeGate {
    all: Arc<tokio::sync::Semaphore>,
    per_peer: usize,
    peers: Mutex<HashMap<[u8; 32], Arc<tokio::sync::Semaphore>>>,
}

/// One file's place in [`ServeGate`], given back when it drops.
struct ServeSlot {
    _peer: tokio::sync::OwnedSemaphorePermit,
    _all: tokio::sync::OwnedSemaphorePermit,
}

impl ServeGate {
    fn new(in_all: usize, per_peer: usize) -> Self {
        Self {
            all: Arc::new(tokio::sync::Semaphore::new(in_all)),
            per_peer,
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// A slot for one file to `peer`, or none when no slot frees within
    /// `within`. The peer's own bound is taken first, so a peer waiting on
    /// its own files never holds a slot others could use.
    async fn admit(&self, peer: &[u8; 32], within: Duration) -> Option<ServeSlot> {
        let own = lock(&self.peers)
            .entry(*peer)
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(self.per_peer)))
            .clone();
        tokio::time::timeout(within, async {
            let peer = own.acquire_owned().await.ok()?;
            let all = self.all.clone().acquire_owned().await.ok()?;
            Some(ServeSlot {
                _peer: peer,
                _all: all,
            })
        })
        .await
        .ok()
        .flatten()
    }
}

impl Default for ServeGate {
    fn default() -> Self {
        Self::new(SERVE_IN_ALL, SERVE_PER_PEER)
    }
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

/// The managed transfer directory, created the first time a job needs it:
/// job journals and the partial files of arriving jobs.
pub(super) fn transfer_directory(shared: &Shared) -> Result<std::path::PathBuf, Error> {
    let dir = lock(&shared.transfer_dir)
        .clone()
        .ok_or(Error::Unsupported("no transfer directory is installed"))?;
    std::fs::create_dir_all(&dir).map_err(|_| Error::InvalidSetting(TRANSFER_DIR_FAILED))?;
    Ok(dir)
}

const TRANSFER_DIR_FAILED: &str = "the transfer directory could not be created";

impl Core {
    /// Chooses the managed transfer directory: an absolute path, created
    /// the first time a job needs it rather than here. Saved jobs load
    /// from it when it already exists. Refused while a controlling session
    /// is active.
    pub fn set_transfer_directory(&self, path: impl AsRef<std::path::Path>) -> Result<(), Error> {
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
            return Err(Error::InvalidSetting(TRANSFER_DIR_FAILED));
        };
        if !path.is_absolute() {
            return Err(Error::InvalidSetting(TRANSFER_DIR_FAILED));
        }
        // Canonical through the parent, which exists, so the path is stable
        // whether or not the directory itself has been created yet.
        let canonical = std::fs::canonicalize(parent)
            .map_err(|_| Error::InvalidSetting(TRANSFER_DIR_FAILED))?
            .join(name);
        if canonical.exists() && !canonical.is_dir() {
            return Err(Error::InvalidSetting(TRANSFER_DIR_FAILED));
        }
        *lock(&self.shared.transfer_dir) = Some(canonical);
        super::jobs::load_jobs(&self.shared);
        Ok(())
    }
}

/// Serves one incoming files connection, one request per stream, until
/// the peer closes it. Tracked by peer so revoke and unpair close it.
pub(super) async fn serve_files_connection(shared: Arc<Shared>, connection: Connection) {
    let peer = *connection.remote_id().as_bytes();
    if lock(&shared.state).trust.get(&peer).is_none() {
        connection.close(0u8.into(), b"denied");
        return;
    }
    lock(&shared.files_serving)
        .entry(peer)
        .or_default()
        .push(connection.clone());
    let mut streams = tokio::task::JoinSet::new();
    while let Ok((send, recv)) = connection.accept_bi().await {
        while streams.try_join_next().is_some() {}
        streams.spawn(serve_stream(shared.clone(), peer, send, recv));
    }
    streams.abort_all();
    let mut serving = lock(&shared.files_serving);
    if let Some(connections) = serving.get_mut(&peer) {
        connections.retain(|kept| kept.stable_id() != connection.stable_id());
        if connections.is_empty() {
            serving.remove(&peer);
        }
    }
}

async fn serve_stream(
    shared: Arc<Shared>,
    peer: [u8; 32],
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let Some(request) = transfer::read_request(&mut recv).await else {
        return;
    };
    let Some(_slot) = shared.files_gate.admit(&peer, SLOT_WITHIN).await else {
        transfer::refuse(&mut send, transfer::STATUS_BUSY).await;
        return;
    };
    // Opening walks the approved root's handles: slow storage parks a
    // blocking thread, never a runtime worker.
    let opened = {
        let shared = shared.clone();
        tokio::task::spawn_blocking(move || {
            super::jobs::open_served_file(&shared, &peer, request.job, request.file)
        })
        .await
    };
    match opened {
        Ok(Ok(source)) => {
            // Cancelling or revoking stops a file mid-way, not only the
            // next one.
            transfer::send(&mut send, source, request.offset, || {
                super::jobs::still_serving(&shared, &peer, request.job)
            })
            .await;
        }
        Ok(Err(status)) => transfer::refuse(&mut send, status).await,
        Err(_) => transfer::refuse(&mut send, transfer::STATUS_DENIED).await,
    }
}

/// Closes one peer's files connections in both directions: its serves
/// end mid-file and the next fetch dials afresh under the new grants.
pub(super) fn close_peer_files(shared: &Shared, peer: &[u8; 32]) {
    let serving = lock(&shared.files_serving).remove(peer).unwrap_or_default();
    for connection in serving {
        connection.close(0u8.into(), b"denied");
    }
    if let Some(fetching) = lock(&shared.files_fetching).remove(peer) {
        fetching.close(0u8.into(), b"denied");
    }
}

impl Core {
    /// Fetches file `file` of the peer's sending job `job` into the
    /// partial at `partial`, whose segment journal is `journal`, continuing
    /// after its last checked segment. Reports the bytes written to
    /// `progress` as they arrive. A segment that fails its check is
    /// fetched once more before the file is given up as a mismatch.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn fetch_job_file(
        &self,
        peer: &[u8; 32],
        job: u64,
        file: u32,
        size: u64,
        partial: &std::path::Path,
        journal: &std::path::Path,
        progress: std::sync::mpsc::Sender<u64>,
        mut cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<Received, Error> {
        let addr = {
            let state = self.alive_state()?;
            device_addr(state.trust.get(peer).ok_or(Error::NotPaired)?)?
        };
        let mut partial = Partial::open(partial, journal, size)?;
        let from = partial.checked();
        let mut mismatches = 0;
        loop {
            if *cancel.borrow() {
                return Ok(Received::Cancelled);
            }
            let request = FileRequest {
                job,
                file,
                offset: partial.checked(),
            };
            let received = self.runtime.block_on(async {
                match ask_kept(&self.shared, peer, addr.clone(), request).await? {
                    Answer::Sending(recv) => {
                        transfer::receive(recv, &mut partial, size, &progress, &mut cancel).await
                    }
                    Answer::Changed => Ok(Received::Changed),
                    Answer::Denied => Ok(Received::Denied),
                    Answer::Busy => Ok(Received::Busy),
                }
            });
            match received {
                Ok(Received::Mismatch) if mismatches == 0 => mismatches += 1,
                Ok(Received::Done { .. }) => {
                    return Ok(Received::Done {
                        downloaded: size - from,
                    })
                }
                other => {
                    // Unchecked bytes never outlive the attempt.
                    partial.drop_unchecked()?;
                    return other;
                }
            }
        }
    }
}

/// Asks for one file over the peer's kept connection. A kept connection
/// that fails before the sender answers is replaced once.
async fn ask_kept(
    shared: &Shared,
    peer: &[u8; 32],
    addr: EndpointAddr,
    request: FileRequest,
) -> Result<Answer, Error> {
    let (mut connection, mut reused) = files_connection(shared, peer, addr.clone()).await?;
    let mut answered = transfer::ask(&connection, request).await;
    while reused && answered.is_err() {
        forget_files_connection(shared, peer, &connection);
        (connection, reused) = files_connection(shared, peer, addr.clone()).await?;
        answered = transfer::ask(&connection, request).await;
    }
    if answered.is_err() {
        forget_files_connection(shared, peer, &connection);
    }
    answered
}

/// The peer's kept files connection while it is open, or a new one kept
/// for the next file. The flag says it was kept.
async fn files_connection(
    shared: &Shared,
    peer: &[u8; 32],
    addr: EndpointAddr,
) -> Result<(Connection, bool), Error> {
    if let Some(kept) = lock(&shared.files_fetching).get(peer) {
        if kept.close_reason().is_none() {
            return Ok((kept.clone(), true));
        }
    }
    let connection = tokio::time::timeout(DIAL_WITHIN, shared.endpoint.connect(addr, FILES_ALPN))
        .await
        .map_err(|_| Error::Timeout)?
        .map_err(Error::network_debug)?;
    if let Some(old) = lock(&shared.files_fetching).insert(*peer, connection.clone()) {
        if old.stable_id() != connection.stable_id() {
            old.close(0u8.into(), b"done");
        }
    }
    Ok((connection, false))
}

/// Stops reusing `connection` for `peer` after a request over it failed.
fn forget_files_connection(shared: &Shared, peer: &[u8; 32], connection: &Connection) {
    let mut fetching = lock(&shared.files_fetching);
    if fetching
        .get(peer)
        .is_some_and(|kept| kept.stable_id() == connection.stable_id())
    {
        fetching.remove(peer);
    }
    drop(fetching);
    connection.close(0u8.into(), b"failed");
}

/// True when the connection negotiated the files ALPN.
pub(super) fn is_files_connection(connection: &Connection) -> bool {
    connection.alpn() == FILES_ALPN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A peer gets as many files as its bound and waits beyond it, while
    /// other peers still get theirs up to the bound in all; a slot given
    /// back is taken again.
    #[tokio::test]
    async fn serving_is_bounded_per_peer_and_in_all() {
        let gate = ServeGate::new(3, 2);
        let wait = Duration::from_millis(30);
        let first = gate.admit(&[1; 32], wait).await.unwrap();
        let _second = gate.admit(&[1; 32], wait).await.unwrap();
        assert!(
            gate.admit(&[1; 32], wait).await.is_none(),
            "past the peer's bound"
        );
        let _other = gate.admit(&[2; 32], wait).await.unwrap();
        assert!(
            gate.admit(&[3; 32], wait).await.is_none(),
            "past the bound in all"
        );
        drop(first);
        assert!(gate.admit(&[1; 32], wait).await.is_some());
    }
}
