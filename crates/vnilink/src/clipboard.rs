//! Clipboard text for one control session.
//!
//! The text rides its own stream so a paste does not wait behind keyboard
//! input, and keyboard input does not wait behind a paste. The host applies
//! it only when that controller's clipboard grant is on. This module does not
//! read or write the operating-system clipboard, and it does not log the text.

use std::fmt;

use crate::error::Error;

/// Largest clipboard paste, in UTF-8 bytes. Empty text clears the clipboard.
pub const MAX_CLIPBOARD_TEXT: usize = 256 * 1024;

/// Tag, length, and text. The outer frame length is four more bytes.
pub(crate) const MAX_FRAME: usize = 1 + 4 + MAX_CLIPBOARD_TEXT;

const TEXT: u8 = 1;
const STATUS: u8 = 2;
/// First byte on the stream. Iroh delivers `accept_bi` only after some data arrives.
const READY: u8 = 3;
const STATUS_APPLIED: u8 = 1;
const STATUS_DENIED: u8 = 2;
const STATUS_UNAVAILABLE: u8 = 3;

/// Applies text on the controlled computer.
///
/// The platform owns the operating-system clipboard. This method must not
/// call back into [`crate::Core`]. The text is private: do not log it.
pub trait HostClipboard: Send {
    fn set_text(&mut self, text: &str) -> Result<(), Error>;
}

pub(crate) fn check_text(text: &str) -> Result<(), Error> {
    if text.len() > MAX_CLIPBOARD_TEXT {
        Err(Error::InvalidSetting(
            "clipboard text is larger than 256 KiB",
        ))
    } else {
        Ok(())
    }
}

/// One clipboard frame. Debug prints the length, not the characters.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Text(String),
    Status(u8),
    Ready,
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => formatter
                .debug_struct("Text")
                .field("len", &text.len())
                .finish(),
            Self::Status(code) => formatter.debug_tuple("Status").field(code).finish(),
            Self::Ready => formatter.write_str("Ready"),
        }
    }
}

pub(crate) struct ClipboardRequest {
    pub text: String,
    pub reply: std::sync::mpsc::SyncSender<Result<(), Error>>,
}

pub(crate) type ClipboardSender = tokio::sync::mpsc::Sender<ClipboardRequest>;

pub(crate) fn encode(frame: &Frame) -> Result<Vec<u8>, Error> {
    let mut payload = Vec::new();
    match frame {
        Frame::Text(text) => {
            check_text(text)?;
            payload.push(TEXT);
            let len = u32::try_from(text.len()).map_err(|_| Error::Malformed)?;
            payload.extend_from_slice(&len.to_be_bytes());
            payload.extend_from_slice(text.as_bytes());
        }
        Frame::Status(code) => {
            if !matches!(*code, STATUS_APPLIED | STATUS_DENIED | STATUS_UNAVAILABLE) {
                return Err(Error::Malformed);
            }
            payload.push(STATUS);
            payload.push(*code);
        }
        Frame::Ready => payload.push(READY),
    }
    let mut bytes = Vec::with_capacity(4 + payload.len());
    let len = u32::try_from(payload.len()).map_err(|_| Error::Malformed)?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend(payload);
    Ok(bytes)
}

pub(crate) fn decode(payload: &[u8]) -> Result<Frame, Error> {
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(Error::Malformed);
    }
    match payload[0] {
        TEXT => {
            if payload.len() < 5 {
                return Err(Error::Malformed);
            }
            let len = u32::from_be_bytes(payload[1..5].try_into().expect("4 bytes")) as usize;
            if len > MAX_CLIPBOARD_TEXT || payload.len() != 5 + len {
                return Err(Error::Malformed);
            }
            let text = std::str::from_utf8(&payload[5..]).map_err(|_| Error::Malformed)?;
            Ok(Frame::Text(text.to_owned()))
        }
        STATUS if payload.len() == 2 && matches!(payload[1], 1 | 2 | 3) => {
            Ok(Frame::Status(payload[1]))
        }
        READY if payload.len() == 1 => Ok(Frame::Ready),
        _ => Err(Error::Malformed),
    }
}

pub(crate) fn check_len(len: usize) -> Result<(), Error> {
    if len == 0 || len > MAX_FRAME {
        Err(Error::Malformed)
    } else {
        Ok(())
    }
}

fn status_result(code: u8) -> Result<(), Error> {
    match code {
        STATUS_APPLIED => Ok(()),
        STATUS_DENIED => Err(Error::ClipboardDenied),
        STATUS_UNAVAILABLE => Err(Error::Unsupported("clipboard is not available")),
        _ => Err(Error::Malformed),
    }
}

fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::ClipboardDenied) => STATUS_DENIED,
        Err(_) => STATUS_UNAVAILABLE,
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
