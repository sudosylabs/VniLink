// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A round trip that proves a platform encoder's output decodes, in the right
//! colour, with the shared software decoder: the test a codec must pass before
//! a host offers it.
use crate::{EncodedSample, Error, RawFrame, SoftwareDecoder, VideoDecoder};
use vnilink::VideoCodec;

/// Width and height of the probe pictures: above NVENC's smallest sizes.
pub const PROBE_SIDE: u16 = 256;

/// Whether a red then a green picture of [`PROBE_SIDE`], put through
/// `encode` (the first as a keyframe), decode back with the software
/// decoder and the second shows green.
pub fn decodes(
    codec: VideoCodec,
    mut encode: impl FnMut(&RawFrame, bool) -> Result<EncodedSample, Error>,
) -> bool {
    const SIDE: u16 = PROBE_SIDE;
    let result = (|| -> Result<bool, Error> {
        let mut decoder = SoftwareDecoder::open(codec)?;
        let mut shown = None;
        for (index, bgra) in [[0, 0, 255, 255], [0, 255, 0, 255]].into_iter().enumerate() {
            let timestamp_us = index as u64 * 33_333;
            let picture = RawFrame::bgra(
                SIDE,
                SIDE,
                timestamp_us,
                bgra.repeat(usize::from(SIDE) * usize::from(SIDE)),
            )?;
            let sample = encode(&picture, index == 0)?;
            let frame = vnilink::EncodedFrame::new(
                codec,
                sample.keyframe(),
                timestamp_us,
                sample.bytes().to_vec(),
            )
            .map_err(|_| Error::EncodeRejected("probe packet"))?;
            shown = Some(decoder.decode(&frame)?);
        }
        let Some(shown) = shown else {
            return Ok(false);
        };
        let middle = usize::from(SIDE) / 2 * shown.stride() + usize::from(SIDE) / 2 * 4;
        let pixel = shown.pixels().get(middle..middle + 4).unwrap_or(&[0; 4]);
        Ok((shown.width(), shown.height()) == (SIDE, SIDE)
            && pixel[1] > 128
            && pixel[0] < 96
            && pixel[2] < 96)
    })();
    result.unwrap_or(false)
}
