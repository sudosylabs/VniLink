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
const FILES: u8 = 4;
const PREPARE: u8 = 5;
const READY_FILES: u8 = 6;
const STATUS_APPLIED: u8 = 1;
const STATUS_DENIED: u8 = 2;
const STATUS_UNAVAILABLE: u8 = 3;
const STATUS_TRANSFER_DENIED: u8 = 4;
pub(crate) fn check_text(text: &str) -> Result<(), Error> {
    if text.len() > MAX_CLIPBOARD_TEXT {
        Err(Error::InvalidSetting(
            "clipboard text is larger than 256 KiB",
        ))
    } else {
        Ok(())
    }
}

/// One clipboard frame. Debug prints lengths and counts, never text or paths.
/// Prepare and FilesReady carry the requester's nonce so a late answer for
/// a replaced selection cannot satisfy the live waiter. Files and Prepare
/// carry the source-assigned offer revision so a same-path re-offer cannot
/// satisfy preparation pinned to an older revision.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Text(String),
    Files {
        revision: u64,
        selection: super::FileSelection,
    },
    Prepare {
        nonce: u64,
        revision: u64,
        selection: super::FileSelection,
    },
    FilesReady {
        nonce: u64,
        outcome: super::PasteReadyOutcome,
    },
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
            Self::Files {
                revision,
                selection,
            } => formatter
                .debug_struct("Files")
                .field("revision", revision)
                .field("entries", &selection.entries.len())
                .finish(),
            Self::Prepare {
                nonce,
                revision,
                selection,
            } => formatter
                .debug_struct("Prepare")
                .field("nonce", nonce)
                .field("revision", revision)
                .field("entries", &selection.entries.len())
                .finish(),
            Self::FilesReady { nonce, outcome } => match outcome {
                super::PasteReadyOutcome::Ready(files) => formatter
                    .debug_struct("FilesReady")
                    .field("nonce", nonce)
                    .field("entries", &files.len())
                    .finish(),
                super::PasteReadyOutcome::TransferDenied => {
                    formatter.write_str("FilesReady(denied)")
                }
                super::PasteReadyOutcome::Unauthorized => {
                    formatter.write_str("FilesReady(unauthorized)")
                }
                super::PasteReadyOutcome::Unavailable => {
                    formatter.write_str("FilesReady(unavailable)")
                }
            },
            Self::Status(code) => formatter.debug_tuple("Status").field(code).finish(),
            Self::Ready => formatter.write_str("Ready"),
        }
    }
}

pub(crate) struct ClipboardRequest {
    pub payload: super::ClipboardPayload,
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
        Frame::Files {
            revision,
            selection,
        } => {
            payload.push(FILES);
            payload.extend_from_slice(&revision.to_be_bytes());
            payload.extend_from_slice(&selection.encode()?);
        }
        Frame::Prepare {
            nonce,
            revision,
            selection,
        } => {
            payload.push(PREPARE);
            payload.extend_from_slice(&nonce.to_be_bytes());
            payload.extend_from_slice(&revision.to_be_bytes());
            payload.extend_from_slice(&selection.encode()?);
        }
        Frame::FilesReady { nonce, outcome } => {
            payload.push(READY_FILES);
            payload.extend_from_slice(&nonce.to_be_bytes());
            payload.extend_from_slice(&super::encode_ready(outcome)?);
        }
        Frame::Status(code) => {
            if !matches!(
                *code,
                STATUS_APPLIED | STATUS_DENIED | STATUS_UNAVAILABLE | STATUS_TRANSFER_DENIED
            ) {
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
        FILES => {
            if payload.len() < 9 {
                return Err(Error::Malformed);
            }
            let revision = u64::from_be_bytes(payload[1..9].try_into().expect("8 bytes"));
            Ok(Frame::Files {
                revision,
                selection: super::FileSelection::decode(&payload[9..])?,
            })
        }
        PREPARE => {
            if payload.len() < 17 {
                return Err(Error::Malformed);
            }
            let nonce = u64::from_be_bytes(payload[1..9].try_into().expect("8 bytes"));
            let revision = u64::from_be_bytes(payload[9..17].try_into().expect("8 bytes"));
            Ok(Frame::Prepare {
                nonce,
                revision,
                selection: super::FileSelection::decode(&payload[17..])?,
            })
        }
        READY_FILES => {
            if payload.len() < 9 {
                return Err(Error::Malformed);
            }
            let nonce = u64::from_be_bytes(payload[1..9].try_into().expect("8 bytes"));
            Ok(Frame::FilesReady {
                nonce,
                outcome: super::decode_ready(&payload[9..])?,
            })
        }
        STATUS if payload.len() == 2 && matches!(payload[1], 1..=4) => {
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

pub(crate) fn status_result(code: u8) -> Result<(), Error> {
    match code {
        STATUS_APPLIED => Ok(()),
        STATUS_DENIED => Err(Error::ClipboardDenied),
        STATUS_UNAVAILABLE => Err(Error::Unsupported("clipboard is not available")),
        STATUS_TRANSFER_DENIED => Err(Error::TransferDenied),
        _ => Err(Error::Malformed),
    }
}

pub(crate) fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::ClipboardDenied) => STATUS_DENIED,
        Err(Error::TransferDenied) => STATUS_TRANSFER_DENIED,
        Err(_) => STATUS_UNAVAILABLE,
    }
}
