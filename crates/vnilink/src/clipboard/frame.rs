// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Clipboard frames. Debug prints the length, not the characters.

use std::fmt;

use crate::error::Error;

use super::MAX_CLIPBOARD_TEXT;

/// Tag, length, and text. The outer frame length is four more bytes.
pub(crate) const MAX_FRAME: usize = 1 + 4 + MAX_CLIPBOARD_TEXT;

const TEXT: u8 = 1;
const STATUS: u8 = 2;
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

/// One clipboard frame: text, or the answer to it. Debug prints the
/// length, never the text.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    Text(String),
    Status(u8),
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => formatter
                .debug_struct("Text")
                .field("len", &text.len())
                .finish(),
            Self::Status(code) => formatter.debug_tuple("Status").field(code).finish(),
        }
    }
}

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
        STATUS if payload.len() == 2 && matches!(payload[1], 1..=3) => {
            Ok(Frame::Status(payload[1]))
        }
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
        _ => Err(Error::Malformed),
    }
}

pub(crate) fn status_code(result: &Result<(), Error>) -> u8 {
    match result {
        Ok(()) => STATUS_APPLIED,
        Err(Error::ClipboardDenied) => STATUS_DENIED,
        Err(_) => STATUS_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard;

    #[test]
    fn text_frames_round_trip_and_hide_the_characters() {
        let frame = Frame::Text("vnl-secret-héllo".to_string());
        assert!(!format!("{frame:?}").contains("vnl-secret"));
        let encoded = clipboard::encode(&frame).unwrap();
        let len = u32::from_be_bytes(encoded[..4].try_into().unwrap()) as usize;
        assert_eq!(encoded.len(), 4 + len);
        assert_eq!(clipboard::decode(&encoded[4..]).unwrap(), frame);
        assert_eq!(
            clipboard::decode(&clipboard::encode(&Frame::Text(String::new())).unwrap()[4..])
                .unwrap(),
            Frame::Text(String::new())
        );
        assert_eq!(
            clipboard::decode(&clipboard::encode(&Frame::Status(1)).unwrap()[4..]).unwrap(),
            Frame::Status(1)
        );
        assert!(clipboard::check_text("").is_ok());
        assert!(clipboard::check_text(&"a".repeat(MAX_CLIPBOARD_TEXT)).is_ok());
        assert!(matches!(
            clipboard::check_text(&"a".repeat(MAX_CLIPBOARD_TEXT + 1)),
            Err(Error::InvalidSetting(_))
        ));
        assert!(matches!(clipboard::check_len(0), Err(Error::Malformed)));
        assert!(matches!(
            clipboard::check_len(usize::MAX),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            clipboard::decode(&[1, 0, 0, 0, 1, 0xff]),
            Err(Error::Malformed)
        ));
        assert!(matches!(
            clipboard::decode(b"\x01\x00\x00\x00\x01aextra"),
            Err(Error::Malformed)
        ));
        assert!(matches!(clipboard::decode(&[2, 9]), Err(Error::Malformed)));
        assert!(matches!(clipboard::decode(&[2, 4]), Err(Error::Malformed)));
        for retired in [3u8, 4] {
            assert!(
                matches!(clipboard::decode(&[retired]), Err(Error::Malformed)),
                "tag {retired} is no frame"
            );
        }
        assert!(matches!(clipboard::decode(&[9]), Err(Error::Malformed)));
    }
}
