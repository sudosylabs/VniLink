// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Length-prefixed frames on the streams beside the control channel: a
//! service's pipe, a Desktop's clipboard pipe, a file request. Each stream
//! keeps its own length limit and codec; the 4-byte big-endian prefix is
//! shared.
// trace: records every ignored failure

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::Error;

/// Length-prefixed frame read shared by the side streams. The caller supplies
/// its own length limit and decoder; the 4-byte big-endian prefix is the same
/// on both streams. `recv` is a stream of the connection, or bytes in memory
/// where a test or the fuzzer reads a frame (`fuzzing::sidestream_frame`).
pub(crate) async fn read_frame<F>(
    recv: &mut (impl AsyncRead + Unpin + ?Sized),
    check_len: impl FnOnce(usize) -> Result<(), Error>,
    decode: impl FnOnce(&[u8]) -> Result<F, Error>,
) -> Result<F, Error> {
    let mut len_buf = [0u8; 4];
    recv.read_exact(&mut len_buf)
        .await
        .map_err(Error::network_debug)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    check_len(len)?;
    // The prefix is the peer's claim: memory follows the bytes that arrive, so
    // a stream that announces a large frame and sends nothing costs one step.
    let mut payload = Vec::with_capacity(len.min(READ_STEP));
    while payload.len() < len {
        let start = payload.len();
        payload.resize(len.min(start + READ_STEP), 0);
        recv.read_exact(&mut payload[start..])
            .await
            .map_err(Error::network_debug)?;
    }
    decode(&payload)
}

/// Bytes a frame's buffer grows by before more of the frame has arrived.
const READ_STEP: usize = 64 * 1024;

/// Length-prefixed frame write shared by the side streams. `did_not_write`
/// names the stream that stayed shut.
pub(crate) async fn write_frame<F: ?Sized>(
    send: &mut iroh::endpoint::SendStream,
    frame: &F,
    encode: impl FnOnce(&F) -> Result<Vec<u8>, Error>,
    did_not_write: &'static str,
) -> Result<(), Error> {
    let bytes = encode(frame)?;
    send.write_all(&bytes)
        .await
        .map_err(|_| Error::network(did_not_write))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads one frame of `bytes` within a limit of 8 bytes.
    async fn frame(mut bytes: &[u8]) -> Result<Vec<u8>, Error> {
        read_frame(
            &mut bytes,
            |len| {
                if len > 8 {
                    Err(Error::Malformed)
                } else {
                    Ok(())
                }
            },
            |payload| Ok(payload.to_vec()),
        )
        .await
    }

    #[tokio::test]
    async fn a_frame_within_its_limit_reads_back_the_bytes_after_its_prefix() {
        assert_eq!(
            frame(&[0, 0, 0, 3, 7, 8, 9, 10]).await.ok(),
            Some(vec![7, 8, 9]),
            "the frame is its three bytes, and the next frame's are left"
        );
    }

    #[tokio::test]
    async fn a_frame_over_its_limit_is_refused_before_its_payload() {
        assert!(
            matches!(frame(&[0, 0, 0, 9]).await, Err(Error::Malformed)),
            "a length over the limit is refused, though no payload follows"
        );
    }

    #[tokio::test]
    async fn a_stream_that_ends_inside_a_frame_is_a_network_failure() {
        for cut in [&[0, 0][..], &[0, 0, 0, 3, 7][..]] {
            assert!(
                matches!(frame(cut).await, Err(Error::Network(_))),
                "a stream cut after {} bytes ends the read",
                cut.len()
            );
        }
    }
}
