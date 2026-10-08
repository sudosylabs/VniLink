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
/// Version 2 adds H.264 High profile negotiation (`CODEC_H264_HIGH`).
/// Version 3 negotiates an independent cursor stream in the Accept codec byte.
/// Version 4 adds files-only sessions (`files_only` in Hello): Transfer-grant
/// admission without video, input or screen sharing. Version 5 adds a neutral
/// service-only connection; service permissions are checked on Open. Hosts admit
/// 1 through 5;
/// newer controllers fall back to a version-1 Baseline hello when an older
/// host rejects it as incompatible (files-only needs v4 on both sides).
pub const PROTOCOL_VERSION: u16 = 5;

/// Version-3 Accept codec bit: independent cursor stream is available.
pub(crate) const SEPARATE_CURSOR: u8 = 0x80;

/// Oldest hello a version-2 host still admits. Version-1 sessions are
/// Baseline-only.
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// H.264 video codec identifier carried in Hello/Accept negotiation.
/// Baseline decodes everywhere; every decoder we ship also decodes High.
pub const CODEC_H264: u8 = 1;

/// H.264 High profile. Offered by version-2 controllers whose decoder
/// handles it; the host replies with the lower of offered and locally
/// encodable, so a Baseline-only host answers `CODEC_H264`.
pub const CODEC_H264_HIGH: u8 = 2;

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
/// `h264_encode` is true once the platform reports a usable H.264 encoder
/// (VA-API on Linux); `h264_decode` is true once it reports a usable H.264
/// decoder. Those grants stay off until `authorize` sets them. Audio stays false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub desktop_capture: bool,
    pub keyboard: bool,
    pub pointer: bool,
    pub clipboard: bool,
    pub file_transfer: bool,
    pub audio: bool,
    pub monitors: u8,
    pub h264_encode: bool,
    pub h264_decode: bool,
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
        h264_encode: false,
        h264_decode: false,
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
            Self::Unsupported => Error::Unsupported("the host cannot share its desktop"),
            Self::Malformed => Error::Malformed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlMessage {
    Hello {
        version: u16,
        codec: u8,
        quality: Quality,
        /// Whether the dialing controller can decode H.264. The host rejects
        /// with `Unsupported` instead of admitting a desktop session the peer
        /// could never display. Ignored for files-only sessions (no video).
        decode: bool,
        /// Files-only session for roots/listings/transfers: Transfer-grant
        /// admission, no video/input/portal. Desktop sessions set false.
        files_only: bool,
        /// Neutral transport admission (v5), without granting any feature.
        /// Must also set files_only so no desktop transport is started.
        services_only: bool,
        /// The controller's display name, if it has one. Stored on the peer
        /// record so the host's device list can name it. Display-only.
        hostname: Option<String>,
    },
    Accept {
        stamp: SessionStamp,
        monitor_width: u16,
        monitor_height: u16,
        codec: u8,
        quality: Quality,
        /// The host's display name, if it has one. Stored on the peer record
        /// so the controller's device list can name it. Display-only.
        hostname: Option<String>,
    },
    Reject(RejectCode),
    Input {
        stamp: SessionStamp,
        event: InputEvent,
    },
    /// Asks the host to encode a keyframe. Sent on the control stream, not the video stream.
    Keyframe {
        stamp: SessionStamp,
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
        stamp: SessionStamp,
        text: KeyText,
    },
    /// Characters the host types by pressing its own layout's chord.
    /// The bytes are UTF-8 and are not logged.
    Chord {
        stamp: SessionStamp,
        text: KeyText,
    },
    /// Agreed encoder limits for the live session. The controller sends this
    /// when the quality changes; the host applies it and re-emits
    /// `QualityUpdated` for its media layer.
    Quality {
        stamp: SessionStamp,
        quality: Quality,
    },
}

/// Session identity stamped on every control message that belongs to one.
/// The id names the session; the generation fences out a previous dial of it.
/// The wire order is unchanged: id bytes, then generation bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SessionStamp {
    pub id: u64,
    pub generation: u64,
}

impl SessionStamp {
    pub(crate) fn new(id: u64, generation: u64) -> Self {
        Self { id, generation }
    }
}

impl ControlMessage {
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            Self::Hello {
                version,
                codec,
                quality,
                decode,
                files_only,
                services_only,
                hostname,
            } => {
                let mut bytes = Vec::with_capacity(16);
                bytes.push(1);
                bytes.extend_from_slice(&version.to_be_bytes());
                bytes.push(*codec);
                bytes.extend_from_slice(&quality.max_width.to_be_bytes());
                bytes.extend_from_slice(&quality.max_height.to_be_bytes());
                bytes.push(quality.max_frame_rate);
                bytes.extend_from_slice(&quality.max_bitrate_kbps.to_be_bytes());
                bytes.push(u8::from(*decode));
                if *version >= 4 {
                    bytes.push(u8::from(*files_only));
                }
                if *version >= 5 {
                    bytes.push(u8::from(*services_only));
                }
                write_hostname(&mut bytes, hostname);
                bytes
            }
            Self::Accept {
                stamp,
                monitor_width,
                monitor_height,
                codec,
                quality,
                hostname,
            } => {
                let mut bytes = Vec::with_capacity(32);
                bytes.push(2);
                write_stamp(stamp, &mut bytes);
                bytes.extend_from_slice(&monitor_width.to_be_bytes());
                bytes.extend_from_slice(&monitor_height.to_be_bytes());
                bytes.push(*codec);
                bytes.extend_from_slice(&quality.max_width.to_be_bytes());
                bytes.extend_from_slice(&quality.max_height.to_be_bytes());
                bytes.push(quality.max_frame_rate);
                bytes.extend_from_slice(&quality.max_bitrate_kbps.to_be_bytes());
                write_hostname(&mut bytes, hostname);
                bytes
            }
            Self::Reject(code) => {
                let value = *code as u16;
                vec![3, (value >> 8) as u8, value as u8]
            }
            Self::Input { stamp, event } => {
                let mut bytes = Vec::with_capacity(22);
                bytes.push(4);
                write_stamp(stamp, &mut bytes);
                event.encode(&mut bytes);
                bytes
            }
            Self::Keyframe { stamp } => {
                let mut bytes = Vec::with_capacity(17);
                bytes.push(5);
                write_stamp(stamp, &mut bytes);
                bytes
            }
            Self::Addresses { relays, direct } => address::encode(relays, direct),
            Self::Close => vec![7],
            Self::Text { stamp, text } => encode_text(8, *stamp, text),
            Self::Chord { stamp, text } => encode_text(9, *stamp, text),
            Self::Quality { stamp, quality } => {
                let mut bytes = Vec::with_capacity(26);
                bytes.push(10);
                write_stamp(stamp, &mut bytes);
                bytes.extend_from_slice(&quality.max_bitrate_kbps.to_be_bytes());
                bytes.push(quality.max_frame_rate);
                bytes.extend_from_slice(&quality.max_width.to_be_bytes());
                bytes.extend_from_slice(&quality.max_height.to_be_bytes());
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
            1 if bytes.len() >= 15 => {
                let quality = Quality::new(
                    u32::from_be_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]),
                    bytes[8],
                    u16::from_be_bytes([bytes[4], bytes[5]]),
                    u16::from_be_bytes([bytes[6], bytes[7]]),
                )
                .map_err(|_| Error::Malformed)?;
                let version = u16::from_be_bytes([bytes[1], bytes[2]]);
                let decode = match bytes[13] {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Malformed),
                };
                // Version 4+: files-only flag follows decode; v1-v3 omit it.
                let (files_only, hostname_start) = if version >= 4 {
                    if bytes.len() < 16 {
                        return Err(Error::Malformed);
                    }
                    let files_only = match bytes[14] {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Malformed),
                    };
                    (files_only, 15)
                } else {
                    (false, 14)
                };
                let (services_only, hostname_start) = if version >= 5 {
                    let flag = *bytes.get(hostname_start).ok_or(Error::Malformed)?;
                    let neutral = match flag {
                        0 => false,
                        1 => true,
                        _ => return Err(Error::Malformed),
                    };
                    if neutral && !files_only {
                        return Err(Error::Malformed);
                    }
                    (neutral, hostname_start + 1)
                } else {
                    (false, hostname_start)
                };
                Ok(Self::Hello {
                    version,
                    codec: bytes[3],
                    quality,
                    decode,
                    files_only,
                    services_only,
                    hostname: read_hostname(&bytes[hostname_start..])?,
                })
            }
            2 if bytes.len() >= 32 => {
                let quality = Quality::new(
                    u32::from_be_bytes([bytes[27], bytes[28], bytes[29], bytes[30]]),
                    bytes[26],
                    u16::from_be_bytes([bytes[22], bytes[23]]),
                    u16::from_be_bytes([bytes[24], bytes[25]]),
                )
                .map_err(|_| Error::Malformed)?;
                Ok(Self::Accept {
                    stamp: decode_stamp(bytes),
                    monitor_width: u16::from_be_bytes([bytes[17], bytes[18]]),
                    monitor_height: u16::from_be_bytes([bytes[19], bytes[20]]),
                    codec: bytes[21],
                    quality,
                    hostname: read_hostname(&bytes[31..])?,
                })
            }
            3 if bytes.len() == 3 => Ok(Self::Reject(RejectCode::from_u16(u16::from_be_bytes([
                bytes[1], bytes[2],
            ]))?)),
            4 if bytes.len() > 17 => Ok(Self::Input {
                stamp: decode_stamp(bytes),
                event: InputEvent::decode(&bytes[17..])?,
            }),
            5 if bytes.len() == 17 => Ok(Self::Keyframe {
                stamp: decode_stamp(bytes),
            }),
            address::TAG => {
                let announcement = address::decode(bytes)?;
                Ok(Self::Addresses {
                    relays: announcement.relays,
                    direct: announcement.direct,
                })
            }
            7 if bytes.len() == 1 => Ok(Self::Close),
            8 if bytes.len() >= 17 => Ok(Self::Text {
                stamp: decode_stamp(bytes),
                text: decode_text(&bytes[17..])?,
            }),
            9 if bytes.len() >= 17 => Ok(Self::Chord {
                stamp: decode_stamp(bytes),
                text: decode_text(&bytes[17..])?,
            }),
            10 if bytes.len() == 26 => {
                let quality = Quality::new(
                    u32::from_be_bytes([bytes[17], bytes[18], bytes[19], bytes[20]]),
                    bytes[21],
                    u16::from_be_bytes([bytes[22], bytes[23]]),
                    u16::from_be_bytes([bytes[24], bytes[25]]),
                )
                .map_err(|_| Error::Malformed)?;
                Ok(Self::Quality {
                    stamp: decode_stamp(bytes),
                    quality,
                })
            }
            _ => Err(Error::Malformed),
        }
    }
}

fn write_stamp(stamp: &SessionStamp, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&stamp.id.to_be_bytes());
    bytes.extend_from_slice(&stamp.generation.to_be_bytes());
}

/// Appends a display name as one length byte plus UTF-8. An absent or
/// invalid name encodes as zero length, so a display name never breaks a
/// session; the reader stays strict and rejects malformed bytes.
fn write_hostname(bytes: &mut Vec<u8>, hostname: &Option<String>) {
    let valid = hostname
        .as_deref()
        .and_then(|name| {
            (crate::hostname::clean_hostname(name).as_deref() == Some(name)).then_some(name)
        })
        .filter(|name| name.len() <= crate::hostname::MAX_HOSTNAME_BYTES);
    match valid {
        Some(name) => {
            bytes.push(name.len() as u8);
            bytes.extend_from_slice(name.as_bytes());
        }
        None => bytes.push(0),
    }
}

/// Reads the trailing display name. The length must consume the frame
/// exactly; the text must be canonical or the frame is malformed.
fn read_hostname(bytes: &[u8]) -> Result<Option<String>, Error> {
    let Some((&length, rest)) = bytes.split_first() else {
        return Err(Error::Malformed);
    };
    if rest.len() != usize::from(length) {
        return Err(Error::Malformed);
    }
    if rest.is_empty() {
        return Ok(None);
    }
    let text = std::str::from_utf8(rest).map_err(|_| Error::Malformed)?;
    match crate::hostname::clean_hostname(text) {
        Some(name) if name == text => Ok(Some(name)),
        _ => Err(Error::Malformed),
    }
}

fn decode_stamp(bytes: &[u8]) -> SessionStamp {
    SessionStamp::new(
        u64::from_be_bytes(bytes[1..9].try_into().expect("8 bytes")),
        u64::from_be_bytes(bytes[9..17].try_into().expect("8 bytes")),
    )
}

fn encode_text(tag: u8, stamp: SessionStamp, text: &KeyText) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(17 + text.0.len());
    bytes.push(tag);
    write_stamp(&stamp, &mut bytes);
    bytes.extend_from_slice(text.0.as_bytes());
    bytes
}

fn decode_text(bytes: &[u8]) -> Result<KeyText, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Malformed)?;
    Ok(KeyText(text.to_owned()))
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
