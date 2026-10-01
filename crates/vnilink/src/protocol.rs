use crate::error::Error;
use crate::input::InputEvent;

/// Application protocol version carried in the control hello.
pub const PROTOCOL_VERSION: u16 = 1;

/// QUIC ALPN for the VniLink control channel. Bytes, not a string type, so the
/// value matches what Iroh negotiates.
pub const ALPN: &[u8] = b"vnilink/1";

pub(crate) const MAX_CONTROL_FRAME: usize = 256;

/// What this installation can do today.
///
/// Capture, input, clipboard, transfer, and audio stay unsupported until a
/// platform adapter reports otherwise. Callers must treat `false` as unavailable.
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

/// Preferred limits for a later encoder. Stored with the session. No media
/// path reads them yet.
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
}

impl ControlMessage {
    pub(crate) fn encode(&self) -> Vec<u8> {
        match *self {
            Self::Hello { version } => vec![1, (version >> 8) as u8, version as u8],
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
                let value = code as u16;
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
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_FRAME {
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
        _ => RejectCode::Malformed,
    }
}
