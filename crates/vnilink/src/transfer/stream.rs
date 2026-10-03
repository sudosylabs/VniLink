//! The transfer stream for one control session.

use crate::error::Error;

use super::frame::{check_len, decode, encode, status_code, status_result, Frame, TransferRequest};
use super::path::{check_submission, checked_size};
use super::{MAX_CHUNK, REJECTED};

/// A finished transfer writes only when the byte count matches and no earlier
/// check failed. A mismatch drops the buffer.
pub(crate) fn finish_decision(
    got: u64,
    size: u64,
    failed: bool,
    preset: Option<Error>,
) -> Result<(), Error> {
    if failed || got != size {
        Err(Error::InvalidSetting(REJECTED))
    } else if let Some(error) = preset {
        Err(error)
    } else {
        Ok(())
    }
}

async fn read_frame(recv: &mut iroh::endpoint::RecvStream) -> Result<Frame, Error> {
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

async fn write_frame(send: &mut iroh::endpoint::SendStream, frame: &Frame) -> Result<(), Error> {
    let bytes = encode(frame)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Error::network("the transfer frame was not written"))?;
    Ok(())
}

async fn write_transfer(
    send: &mut iroh::endpoint::SendStream,
    name: &str,
    bytes: &[u8],
    replace: bool,
) -> Result<(), Error> {
    check_submission(name, bytes)?;
    write_frame(
        send,
        &Frame::Begin {
            replace,
            name: name.to_owned(),
            size: bytes.len() as u64,
        },
    )
    .await?;
    for chunk in bytes.chunks(MAX_CHUNK) {
        write_frame(send, &Frame::Chunk(chunk.to_vec())).await?;
    }
    write_frame(send, &Frame::Finish).await
}

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

struct Incoming {
    replace: bool,
    name: String,
    size: u64,
    got: u64,
    buf: Option<Vec<u8>>,
    failed: bool,
    preset: Option<Error>,
}

/// Owns one transfer stream.
///
/// `dial` is the side that opens the stream. Both sides wait for a status
/// after sending a file, and both write a status after receiving one.
/// `inspect` runs before the body is buffered. `on_file` runs only when the
/// received length matches the declared size.
pub(crate) async fn attach<Inspect, OnFile, Allow>(
    connection: iroh::endpoint::Connection,
    dial: bool,
    requests: tokio::sync::mpsc::Receiver<TransferRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    opened: Option<tokio::sync::oneshot::Sender<Result<(), Error>>>,
    inspect: Inspect,
    on_file: OnFile,
    allow_outgoing: Allow,
) where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error> + Send,
    OnFile: FnMut(&str, &[u8], bool) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    if *stop_rx.borrow() {
        if let Some(opened) = opened {
            let _ = opened.send(Err(Error::NoSession));
        }
        return;
    }
    let streams = tokio::select! {
        result = open_stream(&connection, dial) => result,
        _ = stop_rx.changed() => Err(Error::NoSession),
    };
    match streams {
        Ok((mut send, recv)) => {
            // The peer's accept_bi stays pending until this side sends a byte.
            if dial {
                if let Err(error) = write_frame(&mut send, &Frame::Ready).await {
                    if let Some(opened) = opened {
                        let _ = opened.send(Err(error));
                    }
                    return;
                }
            }
            if let Some(opened) = opened {
                let _ = opened.send(Ok(()));
            }
            exchange(
                send,
                recv,
                requests,
                stop_rx,
                inspect,
                on_file,
                allow_outgoing,
            )
            .await;
        }
        Err(error) => {
            if let Some(opened) = opened {
                let _ = opened.send(Err(error));
            }
        }
    }
}

async fn open_stream(
    connection: &iroh::endpoint::Connection,
    dial: bool,
) -> Result<(iroh::endpoint::SendStream, iroh::endpoint::RecvStream), Error> {
    if dial {
        connection.open_bi().await.map_err(Error::network_debug)
    } else {
        connection.accept_bi().await.map_err(Error::network_debug)
    }
}

pub(crate) async fn exchange<Inspect, OnFile, Allow>(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut requests: tokio::sync::mpsc::Receiver<TransferRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    mut inspect: Inspect,
    mut on_file: OnFile,
    mut allow_outgoing: Allow,
) where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error> + Send,
    OnFile: FnMut(&str, &[u8], bool) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    let (frames_tx, mut frames_rx) = tokio::sync::mpsc::channel(4);
    tokio::spawn(async move {
        loop {
            match read_frame(&mut recv).await {
                Ok(frame) => {
                    if frames_tx.send(Ok(frame)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = frames_tx.send(Err(error)).await;
                    break;
                }
            }
        }
    });
    let mut pending: Option<std::sync::mpsc::SyncSender<Result<(), Error>>> = None;
    let mut incoming: Option<Incoming> = None;
    loop {
        if *stop_rx.borrow() {
            break;
        }
        tokio::select! {
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
            request = requests.recv(), if pending.is_none() && incoming.is_none() => {
                let Some(TransferRequest { name, bytes, replace, reply }) = request else {
                    break;
                };
                if let Err(error) = allow_outgoing() {
                    let _ = reply.send(Err(error));
                    continue;
                }
                if let Err(error) = write_transfer(&mut send, &name, &bytes, replace).await {
                    let _ = reply.send(Err(error));
                    break;
                }
                pending = Some(reply);
            }
            frame = frames_rx.recv() => {
                match frame {
                    Some(Ok(Frame::Ready)) => {}
                    Some(Ok(Frame::Begin { replace, name, size })) => {
                        if pending.is_some() || incoming.is_some() {
                            if let Some(reply) = pending.take() {
                                let _ = reply.send(Err(Error::Malformed));
                            }
                            break;
                        }
                        incoming = Some(begin_incoming(&mut inspect, replace, name, size));
                    }
                    Some(Ok(Frame::Chunk(chunk))) => {
                        let Some(current) = incoming.as_mut() else {
                            break;
                        };
                        push_chunk(current, &chunk);
                    }
                    Some(Ok(Frame::Finish)) => {
                        let Some(current) = incoming.take() else {
                            break;
                        };
                        let result = match finish_decision(
                            current.got,
                            current.size,
                            current.failed,
                            current.preset,
                        ) {
                            Ok(()) => on_file(
                                &current.name,
                                current.buf.as_deref().unwrap_or(&[]),
                                current.replace,
                            ),
                            Err(error) => Err(error),
                        };
                        if write_frame(&mut send, &Frame::Status(status_code(&result)))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Frame::Status(code))) => {
                        if incoming.is_some() {
                            break;
                        }
                        if let Some(reply) = pending.take() {
                            let _ = reply.send(status_result(code));
                        }
                    }
                    Some(Err(error)) => {
                        if let Some(reply) = pending.take() {
                            let _ = reply.send(Err(error));
                        }
                        break;
                    }
                    None => break,
                }
            }
        }
    }
    if let Some(reply) = pending.take() {
        let _ = reply.send(Err(Error::NoSession));
    }
    while let Ok(request) = requests.try_recv() {
        let _ = request.reply.send(Err(Error::NoSession));
    }
}

fn begin_incoming<Inspect>(
    inspect: &mut Inspect,
    replace: bool,
    name: String,
    size: u64,
) -> Incoming
where
    Inspect: FnMut(&str, u64, bool) -> Result<(), Error>,
{
    match inspect(&name, size, replace) {
        Err(error) => Incoming {
            replace,
            name,
            size,
            got: 0,
            buf: None,
            failed: false,
            preset: Some(error),
        },
        Ok(()) => {
            let mut buf = Vec::new();
            let reserved = checked_size(size)
                .ok()
                .and_then(|cap| buf.try_reserve_exact(cap).ok());
            if reserved.is_none() {
                return Incoming {
                    replace,
                    name,
                    size,
                    got: 0,
                    buf: None,
                    failed: false,
                    preset: Some(Error::InvalidSetting(REJECTED)),
                };
            }
            Incoming {
                replace,
                name,
                size,
                got: 0,
                buf: Some(buf),
                failed: false,
                preset: None,
            }
        }
    }
}

fn push_chunk(incoming: &mut Incoming, chunk: &[u8]) {
    let added = chunk.len() as u64;
    if chunk.len() > MAX_CHUNK || incoming.got.saturating_add(added) > incoming.size {
        incoming.failed = true;
        incoming.buf = None;
        return;
    }
    incoming.got += added;
    if let Some(buf) = incoming.buf.as_mut() {
        buf.extend_from_slice(chunk);
    }
}
