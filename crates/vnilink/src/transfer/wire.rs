//! Wire codec of `vnilink/files/1`.
//!
//! A stream starts with a fixed request: the sender's job id, the file's
//! place among the manifest's files and the offset to start from, always
//! a segment boundary (see `hash`). The sender answers with one status
//! byte; when it sends, each segment from the offset to the manifest size
//! follows as its bytes and then its 32-byte BLAKE3 chaining value, and
//! the sender finishes the stream after the last one.

use crate::error::Error;

/// ALPN for file-bytes connections. Control stays on `vnilink/1`.
pub const FILES_ALPN: &[u8] = b"vnilink/files/1";

/// Encoded size of a [`FileRequest`].
pub const REQUEST_LEN: usize = 20;

/// Size of the chaining value after each segment.
pub const VALUE_LEN: usize = 32;

/// The sender sends the segments.
pub const STATUS_SENDING: u8 = 0;
/// The sender does not serve this file to this peer now: no such sending
/// job, a revoked grant, a job that stopped or an offset off a segment
/// boundary. Retryable by resuming.
pub const STATUS_DENIED: u8 = 1;
/// The source is no longer the file the manifest describes.
pub const STATUS_CHANGED: u8 = 2;
/// The sender is serving as many files as it allows; ask again later.
pub const STATUS_BUSY: u8 = 3;

/// Stream reset code for a source that changed while it was being sent.
pub const RESET_CHANGED: u32 = 2;
/// Stream reset or stop code for a peer that made no progress in time.
pub const RESET_STALLED: u32 = 3;

/// One file of one sending job, from `offset` on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileRequest {
    /// The sender's id for the job.
    pub job: u64,
    /// The file's place among the manifest's files, folders left out.
    pub file: u32,
    pub offset: u64,
}

impl FileRequest {
    pub fn encode(&self) -> [u8; REQUEST_LEN] {
        let mut bytes = [0u8; REQUEST_LEN];
        bytes[..8].copy_from_slice(&self.job.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.file.to_be_bytes());
        bytes[12..].copy_from_slice(&self.offset.to_be_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: &[u8; REQUEST_LEN] = bytes.try_into().map_err(|_| Error::Malformed)?;
        Ok(Self {
            job: u64::from_be_bytes(bytes[..8].try_into().expect("8 bytes")),
            file: u32::from_be_bytes(bytes[8..12].try_into().expect("4 bytes")),
            offset: u64::from_be_bytes(bytes[12..].try_into().expect("8 bytes")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips() {
        let request = FileRequest {
            job: u64::MAX - 3,
            file: 70_000,
            offset: 5 << 33,
        };
        assert_eq!(FileRequest::decode(&request.encode()).unwrap(), request);
    }

    #[test]
    fn request_of_the_wrong_length_is_malformed() {
        let bytes = FileRequest {
            job: 1,
            file: 2,
            offset: 3,
        }
        .encode();
        assert!(matches!(
            FileRequest::decode(&bytes[..REQUEST_LEN - 1]),
            Err(Error::Malformed)
        ));
        let mut long = bytes.to_vec();
        long.push(0);
        assert!(matches!(FileRequest::decode(&long), Err(Error::Malformed)));
    }
}
