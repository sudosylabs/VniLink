//! The test agent's channel: a second protocol on this engine's endpoint,
//! [`AGENT_ALPN`], through which a suite on one computer runs a scenario's
//! host half on another (`vnilink-testkit agent`). The channel carries
//! opaque frames; what they say belongs to the test kit.
//!
//! Built only with `test-probes`. Even then an engine offers the protocol
//! only while [`Core::serve_agent`]'s listener lives, and only a paired
//! computer reaches that listener: anyone else is closed with
//! [`CLOSE_DENIED`] before a frame is read.
//!
//! Each side writes an empty frame first: the dialer's lets the agent's
//! accept return, the agent's tells the dialer it was admitted. Every later
//! frame holds 1 to [`MAX_AGENT_FRAME`] bytes behind a 4-byte big-endian
//! length.
// trace: records every ignored failure

use super::*;

use std::sync::mpsc;
use std::time::Duration;

use iroh::endpoint::{ApplicationClose, Connection, ConnectionError, RecvStream, SendStream};

use crate::error::Error;
use crate::protocol::ALPN;
use crate::transfer::FILES_ALPN;

/// The agent's protocol, offered beside the session and files protocols.
pub const AGENT_ALPN: &[u8] = b"vnilink-agent/1";

/// The largest frame either side sends: a scenario's records fit.
pub const MAX_AGENT_FRAME: usize = 16 * 1024 * 1024;

/// How long dialing the agent and its admission may take.
const DIAL_WITHIN: Duration = Duration::from_secs(30);

/// Frames read ahead of the caller. The reader waits for the caller beyond
/// this, so a peer cannot make this computer hold more.
const READ_AHEAD: usize = 4;

/// The agent closes a computer it does not trust with this code.
const CLOSE_DENIED: u32 = 1;
/// Either side closes a channel it is done with with this code.
const CLOSE_DONE: u32 = 2;

/// Channels from paired computers, in the order they arrived. Dropping it
/// stops offering [`AGENT_ALPN`].
pub struct AgentListener {
    channels: Mutex<mpsc::Receiver<AgentChannel>>,
    shared: Weak<Shared>,
}

impl AgentListener {
    /// The next channel a paired computer opened, waiting up to `within`.
    pub fn accept(&self, within: Duration) -> Result<AgentChannel, Error> {
        lock(&self.channels)
            .recv_timeout(within)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                mpsc::RecvTimeoutError::Disconnected => Error::ShutDown,
            })
    }
}

impl Drop for AgentListener {
    fn drop(&mut self) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let _engine = shared.span.enter();
        *lock(&shared.agent) = None;
        shared
            .endpoint
            .set_alpns(vec![ALPN.to_vec(), FILES_ALPN.to_vec()]);
        tracing::info!(target: crate::trace::HOSTING, "agent stopped listening");
    }
}

/// One open channel to a paired computer. Its calls block the calling
/// thread, which must not be one of an engine's runtime threads; dropping
/// it closes the channel.
pub struct AgentChannel {
    peer: String,
    connection: Connection,
    /// Taken on drop, which lets the writer finish the stream.
    outgoing: Option<tokio::sync::mpsc::Sender<Vec<u8>>>,
    /// Signalled once the writer finished and the peer took every frame.
    flushed: Mutex<mpsc::Receiver<()>>,
    /// Whether a frame was queued, so dropping waits for it to arrive.
    sent: std::sync::atomic::AtomicBool,
    incoming: Mutex<mpsc::Receiver<Result<Vec<u8>, Error>>>,
    read_ahead: Arc<tokio::sync::Semaphore>,
}

impl AgentChannel {
    /// The other computer's endpoint id.
    pub fn peer_id(&self) -> &str {
        &self.peer
    }

    /// Queues `frame` to send, waiting while earlier frames are on their way.
    pub fn send(&self, frame: Vec<u8>) -> Result<(), Error> {
        if frame.is_empty() || frame.len() > MAX_AGENT_FRAME {
            return Err(Error::Malformed);
        }
        let outgoing = self.outgoing.as_ref().ok_or(Error::NoSession)?;
        self.sent.store(true, std::sync::atomic::Ordering::Relaxed);
        outgoing
            .blocking_send(frame)
            .map_err(|_| closed_error(&self.connection))
    }

    /// The next frame, waiting up to `within`.
    pub fn recv(&self, within: Duration) -> Result<Vec<u8>, Error> {
        let frame = lock(&self.incoming)
            .recv_timeout(within)
            .map_err(|error| match error {
                mpsc::RecvTimeoutError::Timeout => Error::Timeout,
                mpsc::RecvTimeoutError::Disconnected => closed_error(&self.connection),
            })?;
        self.read_ahead.add_permits(1);
        frame
    }
}

/// How long closing a channel waits for its last frames to arrive.
const FLUSH_WITHIN: Duration = Duration::from_secs(2);

impl Drop for AgentChannel {
    /// Closes the channel once the frames sent on it arrived, waiting at
    /// most [`FLUSH_WITHIN`]: an answer sent just before is not discarded.
    fn drop(&mut self) {
        drop(self.outgoing.take());
        if self.sent.load(std::sync::atomic::Ordering::Relaxed) {
            crate::ignored!(
                crate::trace::SESSION,
                "a channel whose frames did not arrive in time closes anyway",
                lock(&self.flushed)
                    .recv_timeout(FLUSH_WITHIN.saturating_add(Duration::from_secs(1)))
            );
        }
        self.connection.close(CLOSE_DONE.into(), b"done");
    }
}

impl Core {
    /// Offers [`AGENT_ALPN`] on this engine's endpoint until the returned
    /// listener drops. One listener at a time: a second is refused as busy.
    pub fn serve_agent(&self) -> Result<AgentListener, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        let (sender, channels) = mpsc::channel();
        {
            let mut agent = lock(&self.shared.agent);
            if agent.is_some() {
                return Err(Error::Busy);
            }
            *agent = Some(sender);
        }
        self.shared.endpoint.set_alpns(vec![
            ALPN.to_vec(),
            FILES_ALPN.to_vec(),
            AGENT_ALPN.to_vec(),
        ]);
        tracing::info!(target: crate::trace::HOSTING, "agent listening");
        Ok(AgentListener {
            channels: Mutex::new(channels),
            shared: Arc::downgrade(&self.shared),
        })
    }

    /// Opens a channel to the agent on the paired computer `endpoint`,
    /// once it admitted this computer.
    pub fn dial_agent(&self, endpoint: &str) -> Result<AgentChannel, Error> {
        let _engine = self.engine();
        let key = parse_endpoint_id(endpoint)?;
        let addr = super::transfer::peer_addr(&self.shared, &key)?;
        let endpoint_handle = self.shared.endpoint.clone();
        let dialed = self.runtime.block_on(async move {
            tokio::time::timeout(DIAL_WITHIN, async move {
                let connection = endpoint_handle
                    .connect(addr, AGENT_ALPN)
                    .await
                    .map_err(Error::network_debug)?;
                let opened = async {
                    let (mut send, mut recv) =
                        connection.open_bi().await.map_err(Error::network_debug)?;
                    write_frame(&mut send, &[]).await?;
                    if !read_frame(&mut recv).await?.is_empty() {
                        return Err(Error::Malformed);
                    }
                    Ok((send, recv))
                }
                .await;
                match opened {
                    Ok((send, recv)) => Ok((connection, send, recv)),
                    Err(error) => Err(refusal(&connection).unwrap_or(error)),
                }
            })
            .await
            .map_err(|_| Error::Timeout)?
        });
        let peer = crate::diagnostics::fingerprint(&key);
        let (connection, send, recv) = match dialed {
            Ok(dialed) => dialed,
            Err(error) => {
                tracing::info!(
                    target: crate::trace::SESSION,
                    peer,
                    failure = error.name(),
                    "agent channel refused"
                );
                return Err(error);
            }
        };
        tracing::info!(target: crate::trace::SESSION, peer, "agent channel opened");
        Ok(open_channel(
            self.runtime.handle(),
            endpoint.to_owned(),
            connection,
            send,
            recv,
        ))
    }
}

/// True when `connection` negotiated [`AGENT_ALPN`].
pub(super) fn is_agent_connection(connection: &Connection) -> bool {
    connection.alpn() == AGENT_ALPN
}

/// Admits `connection` to the agent's listener when its computer is
/// paired and an agent listens; closes it otherwise.
pub(super) async fn serve_agent_connection(shared: Arc<Shared>, connection: Connection) {
    let key = *connection.remote_id().as_bytes();
    let peer = crate::diagnostics::fingerprint(&key);
    let listener = lock(&shared.agent).clone();
    let trusted = lock(&shared.state).trust.get(&key).is_some();
    let Some(listener) = listener.filter(|_| trusted) else {
        tracing::info!(
            target: crate::trace::HOSTING,
            peer,
            paired = trusted,
            "agent channel denied"
        );
        connection.close(CLOSE_DENIED.into(), b"denied");
        return;
    };
    let opened = tokio::time::timeout(DIAL_WITHIN, async {
        let (mut send, mut recv) = connection.accept_bi().await.map_err(Error::network_debug)?;
        if !read_frame(&mut recv).await?.is_empty() {
            return Err(Error::Malformed);
        }
        write_frame(&mut send, &[]).await?;
        Ok((send, recv))
    })
    .await
    .map_err(|_| Error::Timeout)
    .and_then(|opened| opened);
    let (send, recv) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            tracing::info!(
                target: crate::trace::HOSTING,
                peer,
                failure = error.name(),
                "agent channel did not open"
            );
            connection.close(CLOSE_DONE.into(), b"failed");
            return;
        }
    };
    tracing::info!(target: crate::trace::HOSTING, peer, "agent channel opened");
    let channel = open_channel(
        &tokio::runtime::Handle::current(),
        connection.remote_id().to_string(),
        connection,
        send,
        recv,
    );
    // A listener that stopped drops the channel, which closes it.
    crate::ignored!(
        crate::trace::HOSTING,
        "an agent that stopped listening takes no channel",
        listener.send(channel)
    );
}

/// Starts the writer and the reader of an opened channel on `runtime`.
fn open_channel(
    runtime: &tokio::runtime::Handle,
    peer: String,
    connection: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
) -> AgentChannel {
    let (outgoing, mut to_send) = tokio::sync::mpsc::channel::<Vec<u8>>(READ_AHEAD);
    let (flush, flushed) = mpsc::channel();
    runtime.spawn(async move {
        while let Some(frame) = to_send.recv().await {
            if write_frame(&mut send, &frame).await.is_err() {
                return;
            }
        }
        if crate::ignored!(
            crate::trace::SESSION,
            "a channel already closed needs no finish",
            send.finish()
        )
        .is_some()
        {
            crate::ignored!(
                crate::trace::SESSION,
                "a peer that does not acknowledge in time is closed anyway",
                tokio::time::timeout(FLUSH_WITHIN, send.stopped()).await
            );
        }
        crate::ignored!(
            crate::trace::SESSION,
            "a channel dropped without waiting needs no signal",
            flush.send(())
        );
    });
    let (received, incoming) = mpsc::channel();
    let read_ahead = Arc::new(tokio::sync::Semaphore::new(READ_AHEAD));
    let permits = Arc::clone(&read_ahead);
    runtime.spawn(async move {
        loop {
            let Ok(permit) = permits.acquire().await else {
                return;
            };
            // Given back by `recv` when the caller takes the frame.
            permit.forget();
            let frame = match read_frame(&mut recv).await {
                // A closed stream ends the channel; `recv` says why.
                Err(Error::Network(_)) => return,
                frame => frame,
            };
            let failed = frame.is_err();
            if received.send(frame).is_err() || failed {
                return;
            }
        }
    });
    AgentChannel {
        peer,
        connection,
        outgoing: Some(outgoing),
        flushed: Mutex::new(flushed),
        sent: std::sync::atomic::AtomicBool::new(false),
        incoming: Mutex::new(incoming),
        read_ahead,
    }
}

async fn write_frame(send: &mut SendStream, frame: &[u8]) -> Result<(), Error> {
    let len = u32::try_from(frame.len()).map_err(|_| Error::Malformed)?;
    send.write_all(&len.to_be_bytes())
        .await
        .map_err(Error::network_debug)?;
    send.write_all(frame).await.map_err(Error::network_debug)
}

async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    crate::sidestream::read_frame(
        recv,
        |len| {
            if len > MAX_AGENT_FRAME {
                Err(Error::Malformed)
            } else {
                Ok(())
            }
        },
        |bytes| Ok(bytes.to_vec()),
    )
    .await
}

/// The agent's refusal, when it closed `connection` as not trusted.
fn refusal(connection: &Connection) -> Option<Error> {
    matches!(
        connection.close_reason(),
        Some(ConnectionError::ApplicationClosed(ApplicationClose { error_code, .. }))
            if error_code == CLOSE_DENIED.into()
    )
    .then_some(Error::Unauthorized)
}

/// Why a channel can no longer carry frames.
fn closed_error(connection: &Connection) -> Error {
    refusal(connection).unwrap_or(Error::NoSession)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{loopback_core, pair_for_control};

    const WAIT: Duration = Duration::from_secs(10);

    /// A paired computer's frames reach the agent and its answers come
    /// back, in order, at sizes up to the bound; past it is refused before
    /// anything is sent.
    #[test]
    fn a_paired_computer_exchanges_frames_with_the_agent() {
        let agent = loopback_core();
        let suite = loopback_core();
        pair_for_control(&agent, &suite);
        let listener = agent.serve_agent().unwrap();
        let dialed = std::thread::scope(|scope| {
            let dialing = scope.spawn(|| suite.dial_agent(&agent.endpoint_id()));
            let accepted = listener.accept(WAIT).unwrap();
            (dialing.join().unwrap().unwrap(), accepted)
        });
        let (suite_side, agent_side) = dialed;
        assert_eq!(
            agent_side.peer_id(),
            suite.endpoint_id(),
            "the agent knows who dialed"
        );
        assert_eq!(suite_side.peer_id(), agent.endpoint_id());
        suite_side.send(b"hello".to_vec()).unwrap();
        suite_side.send(vec![7; MAX_AGENT_FRAME]).unwrap();
        assert_eq!(agent_side.recv(WAIT).unwrap(), b"hello");
        assert_eq!(agent_side.recv(WAIT).unwrap().len(), MAX_AGENT_FRAME);
        agent_side.send(b"ready".to_vec()).unwrap();
        assert_eq!(suite_side.recv(WAIT).unwrap(), b"ready");
        assert!(
            matches!(
                suite_side.send(vec![0; MAX_AGENT_FRAME + 1]),
                Err(Error::Malformed)
            ),
            "a frame past the bound is refused"
        );
        assert!(
            matches!(suite_side.send(Vec::new()), Err(Error::Malformed)),
            "an empty frame only opens a channel"
        );
        drop(agent_side);
        assert!(
            matches!(suite_side.recv(WAIT), Err(Error::NoSession)),
            "a channel the agent closed says so"
        );
    }

    /// A computer the agent does not trust is closed before a frame is
    /// read, and its dial says it was refused; one that never paired with
    /// the agent cannot even address it.
    #[test]
    fn a_computer_the_agent_does_not_trust_is_refused() {
        let agent = loopback_core();
        let stranger = loopback_core();
        // The stranger knows the agent; the agent does not know it.
        stranger
            .accept_invitation(&agent.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        let listener = agent.serve_agent().unwrap();
        assert!(
            matches!(
                stranger.dial_agent(&agent.endpoint_id()),
                Err(Error::Unauthorized)
            ),
            "an untrusted computer is refused"
        );
        assert!(
            matches!(
                listener.accept(Duration::from_millis(200)),
                Err(Error::Timeout)
            ),
            "no channel reaches the agent"
        );
        let unknown = loopback_core();
        assert!(
            matches!(
                unknown.dial_agent(&agent.endpoint_id()),
                Err(Error::NotPaired)
            ),
            "a computer dials only an agent it paired with"
        );
    }

    /// One listener at a time; once it drops, the protocol is no longer
    /// offered, and a paired computer is refused again.
    #[test]
    fn the_protocol_is_offered_only_while_a_listener_lives() {
        let agent = loopback_core();
        let suite = loopback_core();
        pair_for_control(&agent, &suite);
        let listener = agent.serve_agent().unwrap();
        assert!(
            matches!(agent.serve_agent(), Err(Error::Busy)),
            "a second listener is refused"
        );
        drop(listener);
        assert!(
            suite.dial_agent(&agent.endpoint_id()).is_err(),
            "an engine no longer serving the agent refuses its protocol"
        );
        let _again = agent.serve_agent().unwrap();
    }
}
