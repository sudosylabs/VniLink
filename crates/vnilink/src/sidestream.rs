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

use crate::error::Error;

/// Length-prefixed frame read shared by the side streams. The caller supplies
/// its own length limit and decoder; the 4-byte big-endian prefix is the same
/// on both streams.
pub(crate) async fn read_frame<F>(
    recv: &mut iroh::endpoint::RecvStream,
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
pub(crate) async fn write_frame<F>(
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
