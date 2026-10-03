//! Clipboard frames. Debug prints the length, not the characters.

use std::fmt;

use crate::error::Error;

use super::MAX_CLIPBOARD_TEXT;

/// Tag, length, and text. The outer frame length is four more bytes.
pub(crate) const MAX_FRAME: usize = 1 + 4 + MAX_CLIPBOARD_TEXT;

const TEXT: u8 = 1;
const STATUS: u8 = 2;
/// First byte on the stream. Iroh delivers `accept_bi` only after some data arrives.
const READY: u8 = 3;
const STATUS_APPLIED: u8 = 1;
const STATUS_DENIED: u8 = 2;
const STATUS_UNAVAILABLE: u8 = 3;
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

pub(super) fn status_result(code: u8) -> Result<(), Error> {
    match code {
        STATUS_APPLIED => Ok(()),
        STATUS_DENIED => Err(Error::ClipboardDenied),
        STATUS_UNAVAILABLE => Err(Error::Unsupported("clipboard is not available")),
        _ => Err(Error::Malformed),
    }
}

pub(super) fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::ClipboardDenied) => STATUS_DENIED,
        Err(_) => STATUS_UNAVAILABLE,
    }
}
