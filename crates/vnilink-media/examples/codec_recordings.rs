// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Synthetic codec/container validation; never captures a real screen.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink::{EncodedFrame, VideoCodec};
    use vnilink_media::{
        EncodeParams, MatroskaRecorder, RawFrame, RecordingLimits, VideoDecoder, VideoEncoder,
    };
    let directory = std::env::args().nth(1).ok_or("output directory required")?;
    std::fs::create_dir_all(&directory)?;
    for codec in [
        VideoCodec::H265,
        VideoCodec::Vp8,
        VideoCodec::Vp9,
        VideoCodec::Av1,
    ] {
        let mut encoder = vnilink_media::SoftwareEncoder::open(
            codec,
            vnilink_media::YuvMatrix::Bt709,
            64,
            64,
            30,
            500_000,
        )?;
        let mut decoder = vnilink_media::SoftwareDecoder::open(codec)?;
        let mut recorder: Option<MatroskaRecorder<std::fs::File>> = None;
        for index in 0..3 {
            let raw = RawFrame::bgra(64, 64, index * 33333, [32u8, 64, 128, 255].repeat(4096))?;
            let sample = encoder.encode(
                &raw,
                &EncodeParams {
                    force_keyframe: index == 0,
                    recovery: false,
                    bitrate_bps: 500_000,
                    frame_rate: 30,
                },
            )?;
            let frame = EncodedFrame::new(
                sample.codec(),
                sample.keyframe(),
                raw.timestamp_us(),
                sample.bytes().to_vec(),
            )?;
            let picture = decoder.decode(&frame)?;
            if picture.width() != 64 || picture.pixels()[2].abs_diff(128) > 20 {
                return Err("decoded pixels differ".into());
            }
            if let Some(recording) = recorder.as_mut() {
                recording.push_video(&frame, index * 33)?;
            } else {
                let path = std::path::Path::new(&directory).join(format!("{}.mkv", codec.label()));
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .read(true)
                    .create_new(true)
                    .open(path)?;
                recorder = Some(MatroskaRecorder::start(
                    file,
                    64,
                    64,
                    &frame,
                    RecordingLimits {
                        max_bytes: 1048576,
                        max_duration_ms: 1000,
                    },
                )?);
            }
        }
        recorder.unwrap().finish()?;
        println!(
            "{}: 3 synthetic frames encoded, decoded and recorded",
            codec.label()
        );
    }
    Ok(())
}
