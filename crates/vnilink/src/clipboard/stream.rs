//! The clipboard stream for one control session.

use crate::error::Error;

use super::frame::{
    check_len, decode, encode, status_code, status_result, ClipboardRequest, Frame,
};

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
        .map_err(|_| Error::network("the clipboard frame was not written"))?;
    Ok(())
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

/// Owns one clipboard stream.
///
/// `dial` is the controller side, which opens the stream and waits for a
/// status after each paste. The host accepts the stream, applies text, and
/// writes that status. `on_text` sees incoming text. `allow_outgoing` runs
/// before this side writes text.
pub(crate) async fn attach<OnText, Allow>(
    connection: iroh::endpoint::Connection,
    dial: bool,
    requests: tokio::sync::mpsc::Receiver<ClipboardRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    opened: Option<tokio::sync::oneshot::Sender<Result<(), Error>>>,
    on_text: OnText,
    allow_outgoing: Allow,
    expect_status: bool,
) where
    OnText: FnMut(&str) -> Result<(), Error> + Send,
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
                on_text,
                allow_outgoing,
                expect_status,
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

pub(crate) async fn exchange<OnText, Allow>(
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
    mut requests: tokio::sync::mpsc::Receiver<ClipboardRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    mut on_text: OnText,
    mut allow_outgoing: Allow,
    expect_status: bool,
) where
    OnText: FnMut(&str) -> Result<(), Error> + Send,
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
            request = requests.recv(), if pending.is_none() => {
                let Some(ClipboardRequest { text, reply }) = request else {
                    break;
                };
                if let Err(error) = allow_outgoing() {
                    let _ = reply.send(Err(error));
                    continue;
                }
                if let Err(error) = write_frame(&mut send, &Frame::Text(text)).await {
                    let _ = reply.send(Err(error));
                    break;
                }
                if expect_status {
                    pending = Some(reply);
                } else {
                    let _ = reply.send(Ok(()));
                }
            }
            frame = frames_rx.recv() => {
                match frame {
                    Some(Ok(Frame::Text(text))) => {
                        let result = on_text(&text);
                        if !expect_status
                            && write_frame(&mut send, &Frame::Status(status_code(&result)))
                                .await
                                .is_err()
                        {
                            break;
                        }
                    }
                    Some(Ok(Frame::Ready)) => {}
                    Some(Ok(Frame::Status(code))) => {
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
