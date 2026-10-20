//! The clipboard stream for one control session.

use crate::error::Error;
use crate::sidestream::{open_stream, write_frame};

use super::frame::{
    check_len, decode, encode, status_code, status_result, ClipboardRequest, Frame,
};
use super::FileSelection;

const DID_NOT_WRITE: &str = "the clipboard frame was not written";

/// One paste: text or a file selection offer. Payload bytes never ride here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardPayload {
    Text(String),
    Files {
        revision: u64,
        selection: FileSelection,
    },
}

/// Owns one clipboard stream.
///
/// `dial` is the controller side, which opens the stream and waits for a
/// status after each paste. The host accepts the stream, applies the payload,
/// and writes that status. `on_payload` sees incoming payloads.
/// `allow_outgoing` runs before this side writes.
pub(crate) async fn attach<OnPayload, Allow>(
    connection: iroh::endpoint::Connection,
    dial: bool,
    requests: tokio::sync::mpsc::Receiver<ClipboardRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    opened: Option<tokio::sync::oneshot::Sender<Result<(), Error>>>,
    on_payload: OnPayload,
    allow_outgoing: Allow,
    expect_status: bool,
) where
    OnPayload: FnMut(&ClipboardPayload) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    if *stop_rx.borrow() {
        if let Some(opened) = opened {
            let _ = opened.send(Err(Error::NoSession));
        }
        return;
    }
    let streams = open_stream(&connection, dial, &mut stop_rx).await;
    match streams {
        Ok((mut send, recv)) => {
            // The peer's accept_bi stays pending until this side sends a byte.
            if dial {
                let ready = write_frame(&mut send, &Frame::Ready, encode, DID_NOT_WRITE).await;
                if let Err(error) = ready {
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
                on_payload,
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

pub(crate) async fn exchange<OnPayload, Allow>(
    mut send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    mut requests: tokio::sync::mpsc::Receiver<ClipboardRequest>,
    mut stop_rx: tokio::sync::watch::Receiver<bool>,
    mut on_payload: OnPayload,
    mut allow_outgoing: Allow,
    expect_status: bool,
) where
    OnPayload: FnMut(&ClipboardPayload) -> Result<(), Error> + Send,
    Allow: FnMut() -> Result<(), Error> + Send,
{
    let mut frames_rx = crate::sidestream::spawn_frame_reader(recv, check_len, decode);
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
                let Some(ClipboardRequest { payload, reply }) = request else {
                    break;
                };
                if let Err(error) = allow_outgoing() {
                    let _ = reply.send(Err(error));
                    continue;
                }
                let frame = match &payload {
                    ClipboardPayload::Text(text) => Frame::Text(text.clone()),
                    ClipboardPayload::Files { revision, selection } => Frame::Files {
                        revision: *revision,
                        selection: selection.clone(),
                    },
                };
                if let Err(error) = write_frame(
                    &mut send,
                    &frame,
                    encode,
                    DID_NOT_WRITE,
                )
                .await
                {
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
                    Some(Ok(payload @ (Frame::Text(_) | Frame::Files { .. }))) => {
                        let payload = match payload {
                            Frame::Text(text) => ClipboardPayload::Text(text),
                            Frame::Files { revision, selection } => {
                                ClipboardPayload::Files { revision, selection }
                            }
                            _ => unreachable!("matched a payload frame"),
                        };
                        let result = on_payload(&payload);
                        if !expect_status
                            && write_frame(
                                &mut send,
                                &Frame::Status(status_code(&result)),
                                encode,
                                DID_NOT_WRITE,
                            )
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
