//! Transfer frames. Debug prints lengths, not the name or the bytes.

use std::fmt;

use crate::error::Error;

use super::path::{check_name, checked_size};
use super::{MAX_CHUNK, MAX_FRAME, REJECTED};

const READY: u8 = 1;
const BEGIN: u8 = 2;
const CHUNK: u8 = 3;
const FINISH: u8 = 4;
const STATUS: u8 = 5;

const STATUS_APPLIED: u8 = 1;
const STATUS_DENIED: u8 = 2;
const STATUS_UNAVAILABLE: u8 = 3;
const STATUS_EXISTS: u8 = 4;
const STATUS_REJECTED: u8 = 5;

/// One transfer frame. Debug prints lengths, not the name or the bytes.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Ready,
    Begin {
        replace: bool,
        name: String,
        size: u64,
    },
    Chunk(Vec<u8>),
    Finish,
    Status(u8),
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => formatter.write_str("Ready"),
            Self::Begin {
                replace,
                name,
                size,
            } => formatter
                .debug_struct("Begin")
                .field("replace", replace)
                .field("name_len", &name.len())
                .field("size", size)
                .finish(),
            Self::Chunk(bytes) => formatter
                .debug_struct("Chunk")
                .field("len", &bytes.len())
                .finish(),
            Self::Finish => formatter.write_str("Finish"),
            Self::Status(code) => formatter.debug_tuple("Status").field(code).finish(),
        }
    }
}

pub(crate) struct TransferRequest {
    pub name: String,
    pub bytes: Vec<u8>,
    pub replace: bool,
    pub reply: std::sync::mpsc::SyncSender<Result<(), Error>>,
}

pub(crate) type TransferSender = tokio::sync::mpsc::Sender<TransferRequest>;

pub(crate) fn encode(frame: &Frame) -> Result<Vec<u8>, Error> {
    let mut payload = Vec::new();
    match frame {
        Frame::Ready => payload.push(READY),
        Frame::Finish => payload.push(FINISH),
        Frame::Status(code) => {
            if !matches!(
                *code,
                STATUS_APPLIED
                    | STATUS_DENIED
                    | STATUS_UNAVAILABLE
                    | STATUS_EXISTS
                    | STATUS_REJECTED
            ) {
                return Err(Error::Malformed);
            }
            payload.push(STATUS);
            payload.push(*code);
        }
        Frame::Begin {
            replace,
            name,
            size,
        } => {
            check_name(name)?;
            checked_size(*size)?;
            payload.push(BEGIN);
            payload.push(u8::from(*replace));
            let name_len = u16::try_from(name.len()).map_err(|_| Error::Malformed)?;
            payload.extend_from_slice(&name_len.to_be_bytes());
            payload.extend_from_slice(name.as_bytes());
            payload.extend_from_slice(&size.to_be_bytes());
        }
        Frame::Chunk(bytes) => {
            if bytes.len() > MAX_CHUNK {
                return Err(Error::Malformed);
            }
            payload.push(CHUNK);
            payload.extend_from_slice(bytes);
        }
    }
    if payload.is_empty() || payload.len() > MAX_FRAME {
        return Err(Error::Malformed);
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
        READY if payload.len() == 1 => Ok(Frame::Ready),
        FINISH if payload.len() == 1 => Ok(Frame::Finish),
        STATUS if payload.len() == 2 && matches!(payload[1], 1 | 2 | 3 | 4 | 5) => {
            Ok(Frame::Status(payload[1]))
        }
        BEGIN => decode_begin(payload),
        CHUNK => Ok(Frame::Chunk(payload[1..].to_vec())),
        _ => Err(Error::Malformed),
    }
}

fn decode_begin(payload: &[u8]) -> Result<Frame, Error> {
    if payload.len() < 12 {
        return Err(Error::Malformed);
    }
    let replace = match payload[1] {
        0 => false,
        1 => true,
        _ => return Err(Error::Malformed),
    };
    let name_len = usize::from(u16::from_be_bytes([payload[2], payload[3]]));
    if payload.len() != 4 + name_len + 8 {
        return Err(Error::Malformed);
    }
    let name = std::str::from_utf8(&payload[4..4 + name_len]).map_err(|_| Error::Malformed)?;
    let size = u64::from_be_bytes(
        payload[4 + name_len..]
            .try_into()
            .map_err(|_| Error::Malformed)?,
    );
    Ok(Frame::Begin {
        replace,
        name: name.to_owned(),
        size,
    })
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
        STATUS_DENIED => Err(Error::TransferDenied),
        STATUS_UNAVAILABLE => Err(Error::Unsupported("file transfer is not available")),
        STATUS_EXISTS => Err(Error::FileExists),
        STATUS_REJECTED => Err(Error::InvalidSetting(REJECTED)),
        _ => Err(Error::Malformed),
    }
}

pub(super) fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::TransferDenied) => STATUS_DENIED,
        Err(Error::Unsupported(_)) => STATUS_UNAVAILABLE,
        Err(Error::FileExists) => STATUS_EXISTS,
        Err(_) => STATUS_REJECTED,
    }
}
