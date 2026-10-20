use std::fmt;

/// A frame, encoder, or capture setup failure.
///
/// Messages name the reason. They do not include pixel or bitstream bytes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("{0}")]
    InvalidFrame(&'static str),
    #[error("{0}")]
    InvalidSetting(&'static str),
    #[error("{0} is not available on this system")]
    Unsupported(&'static str),
    #[error("{0}")]
    EncodeRejected(&'static str),
    #[error("{0}")]
    DecodeRejected(&'static str),
    #[error("capture failed ({0})")]
    CaptureFailed(i32),
    #[error("{0}")]
    CaptureRejected(&'static str),
}

/// Why an encoded frame was refused for its size: larger than one frame may be.
pub const FRAME_TOO_LARGE: &str = "encoded frame is larger than 1 MiB";

impl Error {
    /// The encoder produced a frame larger than one frame may be.
    pub fn is_too_large(&self) -> bool {
        matches!(self, Self::InvalidFrame(message) if *message == FRAME_TOO_LARGE)
    }
}

/// Publishing a paced frame failed in the pacer or in the session.
#[derive(Debug)]
pub enum PublishError {
    Media(Error),
    Session(vnilink::Error),
}

impl fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Media(err) => write!(formatter, "{err}"),
            Self::Session(err) => write!(formatter, "{err}"),
        }
    }
}

impl std::error::Error for PublishError {}

impl From<Error> for PublishError {
    fn from(err: Error) -> Self {
        Self::Media(err)
    }
}

impl From<vnilink::Error> for PublishError {
    fn from(err: vnilink::Error) -> Self {
        Self::Session(err)
    }
}
