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

use iroh::endpoint::{Connection, RecvStream, SendStream, VarInt};

use crate::error::Error;
use crate::transfer::{self, Answer, FileRequest, Partial, Received, FILES_ALPN};

const DIAL_WITHIN: Duration = Duration::from_secs(30);

/// Why a fetch from a peer that shut down fails without dialing.
const PEER_SHUT_DOWN: &str = "the other computer shut down";
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
        let _engine = self.engine();
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
        shared.transfer_counters.refused(transfer::STATUS_BUSY);
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
            let counters = &shared.transfer_counters;
            transfer::send(
                &mut send,
                source,
                request.offset,
                request.held,
                counters,
                || super::jobs::still_serving(&shared, &peer, request.job),
                |position, finished| {
                    super::jobs::sending_at(
                        &shared,
                        &peer,
                        request.job,
                        request.file,
                        position,
                        finished,
                    )
                },
            )
            .await;
        }
        Ok(Err(status)) => {
            shared.transfer_counters.refused(status);
            transfer::refuse(&mut send, status).await;
        }
        Err(_) => {
            shared.transfer_counters.refused(transfer::STATUS_DENIED);
            transfer::refuse(&mut send, transfer::STATUS_DENIED).await;
        }
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
        fetching.connection.close(0u8.into(), b"denied");
    }
}

/// What QUIC sees on each path of this computer's files connections,
/// serving and fetching, for the impaired-network harness.
#[cfg(any(test, feature = "test-probes"))]
impl Core {
    pub fn files_path_stats(&self) -> Vec<serde_json::Value> {
        let _engine = self.engine();
        let shared = &self.shared;
        let serving: Vec<Connection> = lock(&shared.files_serving)
            .values()
            .flatten()
            .cloned()
            .collect();
        let fetching: Vec<(Connection, u64)> = lock(&shared.files_fetching)
            .values()
            .map(|fetching| {
                let window = fetching.window.load(std::sync::atomic::Ordering::Relaxed);
                (fetching.connection.clone(), window)
            })
            .collect();
        let roles = serving
            .iter()
            .map(|connection| ("serving", connection, None));
        roles
            .chain(
                fetching
                    .iter()
                    .map(|(connection, window)| ("fetching", connection, Some(*window))),
            )
            .flat_map(|(role, connection, window)| {
                connection
                    .paths()
                    .iter()
                    .map(|path| {
                        let stats = path.stats();
                        serde_json::json!({
                            "role": role,
                            "relay": path.is_relay(),
                            "selected": path.is_selected(),
                            "rtt_ms": path.rtt().as_secs_f64() * 1e3,
                            "cwnd": stats.cwnd,
                            "congestion_events": stats.congestion_events,
                            "spurious_congestion_events": stats.spurious_congestion_events,
                            "sent_datagrams": stats.udp_tx.datagrams,
                            "lost_packets": stats.lost_packets,
                            "sent_bytes": stats.udp_tx.bytes,
                            "lost_bytes": stats.lost_bytes,
                            "receive_window": window,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
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
        let addr = peer_addr(&self.shared, peer)?;
        self.runtime.block_on(fetch_file(
            &self.shared,
            peer,
            addr,
            FileRequest {
                job,
                file,
                offset: 0,
                held: 0,
            },
            size,
            partial,
            journal,
            &progress,
            &mut cancel,
            &mut None,
        ))
    }
}

/// Where to dial `peer`, while it is paired.
pub(super) fn peer_addr(shared: &Shared, peer: &[u8; 32]) -> Result<EndpointAddr, Error> {
    let state = lock(&shared.state);
    if state.shut_down {
        return Err(Error::ShutDown);
    }
    device_addr(state.trust.get(peer).ok_or(Error::NotPaired)?)
}

/// Fetches one file of a sending job into its partial, continuing after
/// the bytes it holds; `request` names the job and file, and its offset and
/// held bytes are taken from the partial. `via` learns which connection the
/// file arrived on. Run where blocking file writes are fine: the caller's
/// own thread or a blocking worker, never a runtime worker.
#[allow(clippy::too_many_arguments)]
pub(super) async fn fetch_file(
    shared: &Shared,
    peer: &[u8; 32],
    addr: EndpointAddr,
    request: FileRequest,
    size: u64,
    partial: &std::path::Path,
    journal: &std::path::Path,
    progress: &std::sync::mpsc::Sender<u64>,
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    via: &mut Option<usize>,
) -> Result<Received, Error> {
    let mut partial = Partial::open(partial, journal, size)?;
    let from = partial.written();
    let mut mismatches = 0;
    loop {
        if *cancel.borrow() {
            return Ok(Received::Cancelled);
        }
        let request = FileRequest {
            offset: partial.checked(),
            held: partial.held(),
            ..request
        };
        let received = match ask_kept(shared, peer, addr.clone(), request).await {
            Ok((Answer::Sending(recv), connection)) => {
                *via = Some(connection);
                let counters = &shared.transfer_counters;
                transfer::receive(recv, &mut partial, size, progress, counters, cancel).await
            }
            Ok((Answer::Changed, _)) => Ok(Received::Changed),
            Ok((Answer::Denied, _)) => Ok(Received::Denied),
            Ok((Answer::Busy, _)) => Ok(Received::Busy),
            Err(error) => Err(error),
        };
        let received = match received {
            Err(_) if kept_shut_down(shared, peer) => Ok(Received::ShutDown),
            other => other,
        };
        match received {
            Ok(Received::Mismatch) if mismatches == 0 => mismatches += 1,
            Ok(Received::Done { .. }) => {
                shared.transfer_counters.fetched(from > 0);
                return Ok(Received::Done {
                    downloaded: size - from,
                });
            }
            other => {
                if matches!(other, Ok(Received::Stalled)) {
                    shared.transfer_counters.stalled();
                }
                // Bytes that arrived stay for the next attempt, which asks
                // only for the rest: a link that keeps dropping still makes
                // progress. A source that changed makes them stale.
                if matches!(other, Ok(Received::Changed)) {
                    partial.drop_unchecked()?;
                }
                return other;
            }
        }
    }
}

/// Asks for one file over the peer's kept connection. A kept connection
/// that fails before the sender answers is replaced once, unless the peer
/// closed it as it shut down: that one stays, see [`files_connection`].
async fn ask_kept(
    shared: &Shared,
    peer: &[u8; 32],
    addr: EndpointAddr,
    request: FileRequest,
) -> Result<(Answer, usize), Error> {
    let (mut connection, mut reused) = files_connection(shared, peer, addr.clone()).await?;
    let mut answered = transfer::ask(&connection, request).await;
    let shut_down = crate::reconnect::peer_shut_down;
    while reused && answered.is_err() && !shut_down(&connection) {
        forget_files_connection(shared, peer, &connection);
        (connection, reused) = files_connection(shared, peer, addr.clone()).await?;
        answered = transfer::ask(&connection, request).await;
    }
    if answered.is_err() && !shut_down(&connection) {
        forget_files_connection(shared, peer, &connection);
    }
    answered.map(|answer| (answer, connection.stable_id()))
}

/// Whether `connection` is still the peer's open files connection. Revoking
/// and unpairing close it, so a file that arrived on it before then is not
/// used after.
pub(super) fn still_connected(shared: &Shared, peer: &[u8; 32], connection: usize) -> bool {
    lock(&shared.files_fetching).get(peer).is_some_and(|kept| {
        kept.connection.stable_id() == connection && kept.connection.close_reason().is_none()
    })
}

/// A files connection this computer dialed to fetch from a peer, with the
/// receive window it holds the sender to.
pub(super) struct Fetching {
    connection: Connection,
    /// The window last set, for the impaired-network harness.
    #[cfg_attr(not(any(test, feature = "test-probes")), allow(dead_code))]
    window: Arc<std::sync::atomic::AtomicU64>,
}

impl Fetching {
    pub(super) fn connection(&self) -> Connection {
        self.connection.clone()
    }
}

/// How often a fetching connection's path feeds its receive window.
const WINDOW_EVERY: Duration = Duration::from_millis(50);

/// Holds `connection`'s receive window to what its selected path carries,
/// from its round trip and the bytes it received, until the connection
/// ends; see [`transfer::Window`].
async fn follow_window(connection: Connection, shown: Arc<std::sync::atomic::AtomicU64>) {
    let mut window = transfer::Window::new(u64::from(super::host::CONNECTION_RECEIVE_WINDOW));
    let mut ticks = tokio::time::interval(WINDOW_EVERY);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut path = None;
    let mut received: Option<(u64, tokio::time::Instant)> = None;
    let mut set = window.window();
    while connection.close_reason().is_none() {
        ticks.tick().await;
        let selected = connection
            .paths()
            .iter()
            .find(|candidate| candidate.is_selected())
            .map(|selected| {
                let bytes = selected.stats().udp_rx.bytes;
                (selected.id(), selected.rtt(), bytes)
            });
        let Some((id, rtt, bytes)) = selected else {
            continue;
        };
        let now = tokio::time::Instant::now();
        let changed = path.replace(id).is_some_and(|before| before != id);
        let rate = match received.replace((bytes, now)) {
            Some((before, at)) if !changed && bytes >= before => {
                let seconds = now.saturating_duration_since(at).as_secs_f64();
                if seconds > 0.0 {
                    (bytes - before) as f64 / seconds
                } else {
                    0.0
                }
            }
            _ => 0.0,
        };
        window.observe(rtt, changed, rate, now);
        if window.window() != set {
            set = window.window();
            connection.set_receive_window(VarInt::from_u64(set).unwrap_or(VarInt::MAX));
            shown.store(set, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// The peer's kept files connection while it is open, or a new one kept
/// for the next file. The flag says it was kept. A peer that closed the
/// kept one as it shut down is not dialed until it connects again
/// ([`peer_reached`]): it serves nothing meanwhile, and dialing it waits
/// out [`DIAL_WITHIN`].
async fn files_connection(
    shared: &Shared,
    peer: &[u8; 32],
    addr: EndpointAddr,
) -> Result<(Connection, bool), Error> {
    let kept = || {
        let fetching = lock(&shared.files_fetching);
        let kept = &fetching.get(peer)?.connection;
        if crate::reconnect::peer_shut_down(kept) {
            return Some(Err(Error::network(PEER_SHUT_DOWN)));
        }
        kept.close_reason().is_none().then(|| Ok(kept.clone()))
    };
    if let Some(kept) = kept() {
        return kept.map(|kept| (kept, true));
    }
    // Files fetched together share one connection: one of them dials, and
    // the others wait and take what it dialed. A connection in use is never
    // replaced, which would end the other fetches' streams.
    let _dialing = shared.files_dialing.lock().await;
    if let Some(kept) = kept() {
        return kept.map(|kept| (kept, true));
    }
    // Dialed with a small receive window, widened by `follow_window`.
    let options = iroh::endpoint::ConnectOptions::new()
        .with_transport_config(super::host::files_transport_config());
    let connection = tokio::time::timeout(DIAL_WITHIN, async {
        shared
            .endpoint
            .connect_with_opts(addr, FILES_ALPN, options)
            .await
            .map_err(Error::network_debug)?
            .await
            .map_err(Error::network_debug)
    })
    .await
    .map_err(|_| Error::Timeout)??;
    let window = Arc::new(std::sync::atomic::AtomicU64::new(transfer::INITIAL_WINDOW));
    tokio::spawn(follow_window(connection.clone(), window.clone()));
    lock(&shared.files_fetching).insert(
        *peer,
        Fetching {
            connection: connection.clone(),
            window,
        },
    );
    Ok((connection, false))
}

/// Whether the peer closed its kept files connection as it shut down.
fn kept_shut_down(shared: &Shared, peer: &[u8; 32]) -> bool {
    lock(&shared.files_fetching)
        .get(peer)
        .is_some_and(|kept| crate::reconnect::peer_shut_down(&kept.connection))
}

/// A connection with `peer` opened: its kept files connection, if it
/// ended, no longer stands for it, and the next fetch dials.
pub(super) fn peer_reached(shared: &Shared, peer: &[u8; 32]) {
    let mut fetching = lock(&shared.files_fetching);
    if fetching
        .get(peer)
        .is_some_and(|kept| kept.connection.close_reason().is_some())
    {
        fetching.remove(peer);
    }
}

/// Stops reusing `connection` for `peer` after a request over it failed.
fn forget_files_connection(shared: &Shared, peer: &[u8; 32], connection: &Connection) {
    let mut fetching = lock(&shared.files_fetching);
    if fetching
        .get(peer)
        .is_some_and(|kept| kept.connection.stable_id() == connection.stable_id())
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
