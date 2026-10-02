use std::fmt;

use crate::address;
use crate::error::Error;
use crate::input::InputEvent;

/// Finished text carried on the control stream. Debug shows the length only.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct KeyText(pub String);

impl fmt::Debug for KeyText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyText")
            .field("len", &self.0.len())
            .finish()
    }
}

/// Application protocol version carried in the control hello.
pub const PROTOCOL_VERSION: u16 = 1;

/// QUIC ALPN for the VniLink control channel. Bytes, not a string type, so the
/// value matches what Iroh negotiates.
pub const ALPN: &[u8] = b"vnilink/1";

pub(crate) const MAX_CONTROL_FRAME: usize = 256;

/// Address announcements are larger than an input frame and use this cap.
pub(crate) const MAX_ADDRESS_FRAME: usize = address::MAX_FRAME;

/// What this installation can do today.
///
/// `desktop_capture` is true once a capture adapter is installed. The library
/// still does not capture or encode. Keyboard and pointer are true once a
/// desktop adapter is installed. Clipboard is true once a clipboard adapter
/// is installed. File transfer is true once a transfer directory is set.
/// Those grants stay off until `authorize` sets them. Audio stays false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub desktop_capture: bool,
    pub keyboard: bool,
    pub pointer: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub audio: bool,
    pub monitors: u8,
}

impl Capabilities {
    pub const UNSUPPORTED: Self = Self {
        desktop_capture: false,
        keyboard: false,
        pointer: false,
        clipboard: false,
        file_transfer: false,
        audio: false,
        monitors: 0,
    };
}

impl Default for Capabilities {
    fn default() -> Self {
        Self::UNSUPPORTED
    }
}

/// Preferred limits for the platform encoder.
///
/// The shared library stores them with the session and does not reshape
/// frames itself. `vnilink-media` reads them when it paces capture: size,
/// frame rate, and bitrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quality {
    pub max_bitrate_kbps: u32,
    pub max_frame_rate: u8,
    pub max_width: u16,
    pub max_height: u16,
}

impl Quality {
    pub fn new(
        max_bitrate_kbps: u32,
        max_frame_rate: u8,
        max_width: u16,
        max_height: u16,
    ) -> Result<Self, Error> {
        if !(1..=50_000).contains(&max_bitrate_kbps) {
            return Err(Error::InvalidSetting(
                "bitrate must be between 1 and 50000 kbps",
            ));
        }
        if !(1..=60).contains(&max_frame_rate) {
            return Err(Error::InvalidSetting("frame rate must be between 1 and 60"));
        }
        if !(1..=7680).contains(&max_width) || !(1..=7680).contains(&max_height) {
            return Err(Error::InvalidSetting(
                "width and height must be between 1 and 7680",
            ));
        }
        Ok(Self {
            max_bitrate_kbps,
            max_frame_rate,
            max_width,
            max_height,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RejectCode {
    Unauthorized = 1,
    NotHosting = 2,
    Busy = 3,
    IncompatibleVersion = 4,
    StaleSession = 5,
    Unsupported = 6,
    Malformed = 7,
}

impl RejectCode {
    fn from_u16(value: u16) -> Result<Self, Error> {
        Ok(match value {
            1 => Self::Unauthorized,
            2 => Self::NotHosting,
            3 => Self::Busy,
            4 => Self::IncompatibleVersion,
            5 => Self::StaleSession,
            6 => Self::Unsupported,
            7 => Self::Malformed,
            _ => return Err(Error::Malformed),
        })
    }

    pub(crate) fn to_error(self) -> Error {
        match self {
            Self::Unauthorized => Error::Unauthorized,
            Self::NotHosting => Error::NotHosting,
            Self::Busy => Error::Busy,
            Self::IncompatibleVersion => Error::IncompatibleVersion,
            Self::StaleSession => Error::StaleSession,
            Self::Unsupported => Error::Unsupported("no desktop adapter is installed"),
            Self::Malformed => Error::Malformed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlMessage {
    Hello {
        version: u16,
    },
    Accept {
        session_id: u64,
        generation: u64,
        monitor_width: u16,
        monitor_height: u16,
    },
    Reject(RejectCode),
    Input {
        session_id: u64,
        generation: u64,
        event: InputEvent,
    },
    /// Asks the host to encode a keyframe. Sent on the control stream, not the video stream.
    Keyframe {
        session_id: u64,
        generation: u64,
    },
    /// Home relay URLs and direct sockets for the sender. No credential, no grants.
    Addresses {
        relays: Vec<String>,
        direct: Vec<std::net::SocketAddr>,
    },
    /// The host ended the session on purpose. The controller does not redial.
    Close,
    /// Finished characters for the host to type. Not a key position and not
    /// a clipboard paste. The bytes are UTF-8 and are not logged.
    Text {
        session_id: u64,
        generation: u64,
        text: KeyText,
    },
}

impl ControlMessage {
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            Self::Hello { version } => vec![1, (version >> 8) as u8, *version as u8],
            Self::Accept {
                session_id,
                generation,
                monitor_width,
                monitor_height,
            } => {
                let mut bytes = Vec::with_capacity(21);
                bytes.push(2);
                bytes.extend_from_slice(&session_id.to_be_bytes());
                bytes.extend_from_slice(&generation.to_be_bytes());
                bytes.extend_from_slice(&monitor_width.to_be_bytes());
                bytes.extend_from_slice(&monitor_height.to_be_bytes());
                bytes
            }
            Self::Reject(code) => {
                let value = *code as u16;
                vec![3, (value >> 8) as u8, value as u8]
            }
            Self::Input {
                session_id,
                generation,
                event,
            } => {
                let mut bytes = Vec::with_capacity(22);
                bytes.push(4);
                bytes.extend_from_slice(&session_id.to_be_bytes());
                bytes.extend_from_slice(&generation.to_be_bytes());
                event.encode(&mut bytes);
                bytes
            }
            Self::Keyframe {
                session_id,
                generation,
            } => {
                let mut bytes = Vec::with_capacity(17);
                bytes.push(5);
                bytes.extend_from_slice(&session_id.to_be_bytes());
                bytes.extend_from_slice(&generation.to_be_bytes());
                bytes
            }
            Self::Addresses { relays, direct } => address::encode(relays, direct),
            Self::Close => vec![7],
            Self::Text {
                session_id,
                generation,
                text,
            } => {
                let mut bytes = Vec::with_capacity(17 + text.0.len());
                bytes.push(8);
                bytes.extend_from_slice(&session_id.to_be_bytes());
                bytes.extend_from_slice(&generation.to_be_bytes());
                bytes.extend_from_slice(text.0.as_bytes());
                bytes
            }
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > MAX_ADDRESS_FRAME {
            return Err(Error::Malformed);
        }
        if bytes[0] != address::TAG && bytes.len() > MAX_CONTROL_FRAME {
            return Err(Error::Malformed);
        }
        match bytes[0] {
            1 if bytes.len() == 3 => Ok(Self::Hello {
                version: u16::from_be_bytes([bytes[1], bytes[2]]),
            }),
            2 if bytes.len() == 21 => Ok(Self::Accept {
                session_id: u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes")),
                generation: u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes")),
                monitor_width: u16::from_be_bytes([bytes[17], bytes[18]]),
                monitor_height: u16::from_be_bytes([bytes[19], bytes[20]]),
            }),
            3 if bytes.len() == 3 => Ok(Self::Reject(RejectCode::from_u16(u16::from_be_bytes([
                bytes[1], bytes[2],
            ]))?)),
            4 if bytes.len() > 17 => Ok(Self::Input {
                session_id: u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes")),
                generation: u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes")),
                event: InputEvent::decode(&bytes[17..])?,
            }),
            5 if bytes.len() == 17 => Ok(Self::Keyframe {
                session_id: u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes")),
                generation: u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes")),
            }),
            address::TAG => {
                let announcement = address::decode(bytes)?;
                Ok(Self::Addresses {
                    relays: announcement.relays,
                    direct: announcement.direct,
                })
            }
            7 if bytes.len() == 1 => Ok(Self::Close),
            8 if bytes.len() >= 17 => {
                let text = std::str::from_utf8(&bytes[17..]).map_err(|_| Error::Malformed)?;
                Ok(Self::Text {
                    session_id: u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes")),
                    generation: u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes")),
                    text: KeyText(text.to_owned()),
                })
            }
            _ => Err(Error::Malformed),
        }
    }
}

pub(crate) fn error_code(error: &Error) -> RejectCode {
    match error {
        Error::Unauthorized => RejectCode::Unauthorized,
        Error::NotHosting => RejectCode::NotHosting,
        Error::Busy => RejectCode::Busy,
        Error::IncompatibleVersion => RejectCode::IncompatibleVersion,
        Error::StaleSession => RejectCode::StaleSession,
        Error::Unsupported(_) => RejectCode::Unsupported,
        // A paste or file refusal stays on its own stream. Do not reject the
        // control session for ClipboardDenied, TransferDenied, or FileExists.
        _ => RejectCode::Malformed,
    }
}
