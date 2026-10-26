//! Stream establishment beside the control channel.
//!
//! Clipboard and transfer each run one bidirectional stream per session and
//! share one handshake: the dialer writes first so the peer's accept unblocks,
//! then both sides report through `opened`. Two `accept_bi` calls must never
//! run at once: they can take each other's stream, so the host accepts the
//! clipboard stream before the transfer stream.
//!
//! The `attach` preamble stays per protocol on purpose: the Ready frame and
//! the exchange differ by stream, and a generic over both would trade two
//! readable wirings for higher-order plumbing. Establishment lives here;
//! framing lives in each stream.
// trace: records every ignored failure

use std::time::Duration;

use crate::error::Error;

/// How long the controller waits for a side stream to report open.
pub(crate) const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// Length-prefixed frame read shared by the side streams. The caller supplies
/// its own length limit and decoder; the 4-byte big-endian prefix is the same
/// on both streams.
pub(crate) async fn read_frame<F>(
    recv: &mut iroh::endpoint::RecvStream,
    check_len: impl FnOnce(usize) -> Result<(), Error>,
    decode: impl FnOnce(&[u8]) -> Result<F, Error>,
) -> Result<F, Error> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(Error::network_debug)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    check_len(len)?;
    let mut payload = vec![0u8; len];
    recv.read_exact(&mut payload)
        .await
        .map_err(Error::network_debug)?;
    decode(&payload)
}

/// Length-prefixed frame write shared by the side streams. `did_not_write`
/// names the stream that stayed shut.
pub(crate) async fn write_frame<F>(
    send: &mut iroh::endpoint::SendStream,
    frame: &F,
    encode: impl FnOnce(&F) -> Result<Vec<u8>, Error>,
    did_not_write: &'static str,
) -> Result<(), Error> {
    let bytes = encode(frame)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Error::network(did_not_write))?;
    Ok(())
}

/// Pumps decoded frames from the stream into a channel. Ends on a read error
/// or when the exchange drops the receiver. Both streams read the same way;
/// only their length limits and decoders differ.
pub(crate) fn spawn_frame_reader<F>(
    mut recv: iroh::endpoint::RecvStream,
    check_len: impl Fn(usize) -> Result<(), Error> + Send + Sync + 'static,
    decode: impl Fn(&[u8]) -> Result<F, Error> + Send + Sync + 'static,
) -> tokio::sync::mpsc::Receiver<Result<F, Error>>
where
    F: Send + 'static,
{
    let (frames_tx, frames_rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            match read_frame(&mut recv, &check_len, &decode).await {
                Ok(frame) => {
                    if frames_tx.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    crate::ignored!(
                        crate::trace::SESSION,
                        "a reader whose consumer left stops reading",
                        frames_tx.send(Err(error)).await
                    );
                    break;
                }
            }
        }
    });
    frames_rx
}

/// Accepts the next bidirectional stream. One accept at a time: a second
/// `accept_bi` running beside this one can take the other stream.
pub(crate) async fn accept_bi(
    connection: &iroh::endpoint::Connection,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), Error> {
    if *stop_rx.borrow() {
        return Err(Error::NoSession);
    }
    tokio::select! {
        result = connection.accept_bi() => result.map_err(Error::network_debug),
        _ = stop_rx.changed() => Err(Error::NoSession),
    }
}

/// Opens or accepts one side stream, stopping early on shutdown. The caller
/// writes its Ready frame when dialing, then runs its exchange.
pub(crate) async fn open_stream(
    connection: &iroh::endpoint::Connection,
    dial: bool,
    stop_rx: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), Error> {
    if *stop_rx.borrow() {
        return Err(Error::NoSession);
    }
    if dial {
        return connection.open_bi().await.map_err(Error::network_debug);
    }
    accept_bi(connection, stop_rx).await
}

/// Waits for a dialed side stream to report open. Aborts the task on any other
/// outcome. `did_not_open` names the stream that stayed shut.
pub(crate) async fn await_opened(
    task: tokio::task::JoinHandle<()>,
    opened: tokio::sync::oneshot::Receiver<Result<(), Error>>,
    did_not_open: &'static str,
) -> Result<tokio::task::JoinHandle<()>, Error> {
    match tokio::time::timeout(OPEN_TIMEOUT, opened).await {
        Ok(Ok(Ok(()))) => Ok(task),
        Ok(Ok(Err(error))) => {
            task.abort();
            Err(error)
        }
        _ => {
            task.abort();
            Err(Error::network(did_not_open))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_open_stream_reports_its_task() {
        let task = tokio::spawn(async {});
        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
        let _ = opened_tx.send(Ok(()));
        let returned = await_opened(task, opened_rx, "the clipboard stream did not open")
            .await
            .unwrap();
        returned.await.unwrap();
    }

    #[tokio::test]
    async fn a_refused_stream_returns_the_refusal() {
        let task = tokio::spawn(async {});
        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel();
        let _ = opened_tx.send(Err(Error::ClipboardDenied));
        assert!(matches!(
            await_opened(task, opened_rx, "the clipboard stream did not open").await,
            Err(Error::ClipboardDenied)
        ));
    }

    #[tokio::test]
    async fn a_silent_stream_reports_that_it_did_not_open() {
        let task = tokio::spawn(async {});
        let (opened_tx, opened_rx) = tokio::sync::oneshot::channel::<Result<(), Error>>();
        drop(opened_tx);
        assert!(matches!(
            await_opened(task, opened_rx, "the transfer stream did not open").await,
            Err(Error::Network(_))
        ));
    }
}
