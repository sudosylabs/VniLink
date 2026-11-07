// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use vnilink_media::{parse_annex_b, FrameSink, PumpOutput, Quality, RawFrame};

use crate::encode::WindowsEncoder;
use crate::host::HostSession;

/// The session's first picture from Media Foundation is a keyframe that
/// carries its SPS and PPS, so a viewer can start decoding from it.
#[test]
fn the_first_published_picture_carries_its_parameter_sets() {
    let quality = Quality::new(500, 30, 64, 64).unwrap();
    let mut session = HostSession::open(128, 128, quality, |w, h, fps, bps| {
        WindowsEncoder::open(w, h, fps, bps, vnilink::YuvMatrix::Bt709)
    })
    .unwrap();
    let mut sink = MemorySink::default();
    let first = session.publish(&solid(128, 128, 0), &mut sink, 7).unwrap();
    assert!(matches!(first, PumpOutput::Encoded(_)));
    let parsed = parse_annex_b(&sink.frames[0].payload).unwrap();
    assert!(sink.frames[0].keyframe && parsed.has_sps() && parsed.has_pps() && parsed.has_idr());
}

#[derive(Default)]
struct MemorySink {
    frames: Vec<vnilink::EncodedFrame>,
}

impl FrameSink for MemorySink {
    fn publish_frame(
        &mut self,
        _session_id: u64,
        frame: vnilink::EncodedFrame,
    ) -> Result<(), vnilink::Error> {
        self.frames.push(frame);
        Ok(())
    }
}

fn solid(width: u16, height: u16, timestamp_us: u64) -> RawFrame {
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    for _ in 0..usize::from(width) * usize::from(height) {
        pixels.extend_from_slice(&[0, 0, 255, 255]);
    }
    RawFrame::bgra(width, height, timestamp_us, pixels).unwrap()
}
