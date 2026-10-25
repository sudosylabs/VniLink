//! The clipboard stream for one control session.
// trace: records every ignored failure

use crate::error::Error;
use crate::ignored;
use crate::sidestream::{open_stream, write_frame};
use crate::trace::CLIPBOARD;

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

impl ClipboardPayload {
    /// What a record may say of it: its kind and size, never its content.
    fn kind(&self) -> &'static str {
        match self {
            Self::Text(_) => "text",
            Self::Files { .. } => "files",
        }
    }
    /// Text's bytes, or a selection's entries.
    fn amount(&self) -> usize {
        match self {
            Self::Text(text) => text.len(),
            Self::Files { selection, .. } => selection.entries.len(),
        }
    }
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
            ignored!(
                CLIPBOARD,
                "the session stopped waiting for the stream",
                opened.send(Err(Error::NoSession))
            );
        }
        return;
    }
    let streams = open_stream(&connection, dial, &mut stop_rx).await;
    match &streams {
        Ok(_) => tracing::debug!(target: CLIPBOARD, dial, "clipboard stream opened"),
        Err(error) => {
            tracing::info!(target: CLIPBOARD, dial, error = error.name(), "clipboard stream not opened")
        }
    }
    match streams {
        Ok((mut send, recv)) => {
            // The peer's accept_bi stays pending until this side sends a byte.
            if dial {
                let ready = write_frame(&mut send, &Frame::Ready, encode, DID_NOT_WRITE).await;
                if let Err(error) = ready {
                    tracing::info!(target: CLIPBOARD, error = error.name(), "clipboard stream not readied");
                    if let Some(opened) = opened {
                        ignored!(
                            CLIPBOARD,
                            "the session stopped waiting for the stream",
                            opened.send(Err(error))
                        );
                    }
                    return;
                }
            }
            if let Some(opened) = opened {
                ignored!(
                    CLIPBOARD,
                    "the session stopped waiting for the stream",
                    opened.send(Ok(()))
                );
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
                ignored!(
                    CLIPBOARD,
                    "the session stopped waiting for the stream",
                    opened.send(Err(error))
                );
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
    // The offer waiting for the other side's answer, and what it was.
    let mut pending: Option<std::sync::mpsc::SyncSender<Result<(), Error>>> = None;
    let mut waiting: Option<(&'static str, usize, std::time::Instant)> = None;
    let mut ended = "stopped";
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
                    ended = "the session dropped its sender";
                    break;
                };
                if let Err(error) = allow_outgoing() {
                    tracing::info!(
                        target: CLIPBOARD,
                        kind = payload.kind(),
                        refused = error.name(),
                        "clipboard not sent"
                    );
                    ignored!(CLIPBOARD, "the caller stopped waiting for its answer", reply.send(Err(error)));
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
                    tracing::info!(
                        target: CLIPBOARD,
                        kind = payload.kind(),
                        error = error.name(),
                        "clipboard not written"
                    );
                    ignored!(CLIPBOARD, "the caller stopped waiting for its answer", reply.send(Err(error)));
                    ended = "a write failed";
                    break;
                }
                tracing::debug!(
                    target: CLIPBOARD,
                    kind = payload.kind(),
                    amount = payload.amount(),
                    "clipboard sent"
                );
                if expect_status {
                    pending = Some(reply);
                    waiting = Some((payload.kind(), payload.amount(), std::time::Instant::now()));
                } else {
                    ignored!(CLIPBOARD, "the caller stopped waiting for its answer", reply.send(Ok(())));
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
                        tracing::debug!(
                            target: CLIPBOARD,
                            kind = payload.kind(),
                            amount = payload.amount(),
                            refused = result.as_ref().err().map(Error::name),
                            reason = result.as_ref().err().and_then(Error::reason),
                            "clipboard received"
                        );
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
                            ended = "an answer could not be written";
                            break;
                        }
                    }
                    Some(Ok(Frame::Ready)) => {}
                    Some(Ok(Frame::Status(code))) => {
                        let answer = status_result(code);
                        if let Some((kind, amount, sent)) = waiting.take() {
                            tracing::debug!(
                                target: CLIPBOARD,
                                kind,
                                amount,
                                refused = answer.as_ref().err().map(Error::name),
                                took_ms = u64::try_from(sent.elapsed().as_millis()).unwrap_or(u64::MAX),
                                "clipboard answered"
                            );
                        }
                        if let Some(reply) = pending.take() {
                            ignored!(CLIPBOARD, "the caller stopped waiting for its answer", reply.send(answer));
                        }
                    }
                    Some(Err(error)) => {
                        tracing::info!(target: CLIPBOARD, error = error.name(), "clipboard frame refused");
                        if let Some(reply) = pending.take() {
                            ignored!(CLIPBOARD, "the caller stopped waiting for its answer", reply.send(Err(error)));
                        }
                        ended = "a frame was refused";
                        break;
                    }
                    None => {
                        ended = "the peer closed the stream";
                        break;
                    }
                }
            }
        }
    }
    let mut abandoned = u64::from(pending.is_some());
    if let Some(reply) = pending.take() {
        ignored!(
            CLIPBOARD,
            "the caller stopped waiting for its answer",
            reply.send(Err(Error::NoSession))
        );
    }
    while let Ok(request) = requests.try_recv() {
        abandoned += 1;
        ignored!(
            CLIPBOARD,
            "the caller stopped waiting for its answer",
            request.reply.send(Err(Error::NoSession))
        );
    }
    tracing::debug!(target: CLIPBOARD, ended, abandoned, "clipboard stream ended");
}
