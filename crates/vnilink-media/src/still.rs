// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Still pictures: one AV1 4:4:4 keyframe of a screen that stopped
//! changing, sharp where the stream's 4:2:0 blurs coloured text. The viewer
//! shows it until the screen changes ([`vnilink::FrameRole::Still`]).
//!
//! The picture is encoded at the capture's own size, in BT.709 limited range
//! whatever matrix the stream is in ([`STILL_SIGNAL`]): the viewer's still
//! decoder is software AV1, which follows the picture's own declaration.
//! It is screen content: do not log it.
// trace: records every ignored failure

use std::sync::mpsc;
use std::thread::JoinHandle;

use shiguredo_aom as aom;

use crate::colour::{ToRgb, VideoSignal, YuvMatrix};
use crate::error::Error;
use crate::frame::RawFrame;

/// The colour every still carries.
const STILL_SIGNAL: VideoSignal = VideoSignal::stream(YuvMatrix::Bt709);

/// How long the screen stays unchanged before its still picture is made:
/// the stream's own refinement of a still screen ends then.
pub const STILL_AFTER: std::time::Duration =
    std::time::Duration::from_micros(crate::pump::REFINE_FOR_US);

/// How much closer to the source than the best 4:2:0 picture a still must
/// be, in dB of PSNR, to be worth sending: less is not a visible gain, and a
/// still that loses would make the picture worse.
const BETTER_BY: f64 = 1.0;

/// AV1 quantizer levels tried, finest first, until a picture fits. On a
/// 1080p page of coloured text they give about 40, 36, 32, 30 and 29 dB in
/// 386, 286, 185, 139 and 104 kB, against 26 dB for the best 4:2:0 picture.
/// The coarsest two fit what a stream at 6 and at 4 Mb/s carries in
/// [`vnilink::STILL_DRAIN`]: a stream starts at 4 Mb/s, and a still screen
/// does not raise it. Coarser still beats 4:2:0 by too little to be worth
/// its bytes.
const LEVELS: [u32; 5] = [20, 28, 36, 40, 44];

/// Bytes a megapixel of dense coloured text takes at each of [`LEVELS`],
/// from the 1080p page above, rounded up. The finest level these say fits
/// is tried first: each try takes 100 to 200 ms at 1080p, and a still that
/// comes after the screen moved again is thrown away. Lighter content fits
/// that level with room to spare.
const DENSE_BYTES_PER_MEGAPIXEL: [u64; 5] = [187_000, 138_000, 90_000, 67_000, 50_000];

/// Encodes still pictures with libaom: AV1 High profile, 4:4:4.
pub struct StillEncoder {
    threads: u32,
}

impl Default for StillEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl StillEncoder {
    pub fn new() -> Self {
        let threads = std::thread::available_parallelism().map_or(4, |count| count.get().min(8));
        Self {
            threads: threads as u32,
        }
    }

    /// One AV1 4:4:4 keyframe of `frame` of at most `budget` bytes, when it
    /// is worth sending: closer to `frame` by [`BETTER_BY`] than a 4:2:0
    /// round trip of it, the best any stream can show. It starts at the
    /// finest level dense text is expected to fit, goes coarser while the
    /// picture does not fit, and finer while one that fits is not better.
    /// On mostly grey text 4:2:0 loses little, and only a fine still beats
    /// the stream. `None` when no level fits and beats the stream.
    pub fn encode(&self, frame: &RawFrame, budget: usize) -> Result<Option<Vec<u8>>, Error> {
        let (y, u, v) = bgra_to_i444(frame);
        let pixels = u64::from(frame.width()) * u64::from(frame.height());
        let first = DENSE_BYTES_PER_MEGAPIXEL
            .iter()
            .position(|per| per * pixels / 1_000_000 <= budget as u64)
            .unwrap_or(LEVELS.len() - 1);
        let encode = |level| self.encode_at(frame.width(), frame.height(), &y, &u, &v, level);
        for (index, &level) in LEVELS.iter().enumerate().skip(first) {
            let picture = encode(level)?;
            if picture.len() > budget {
                continue;
            }
            if self.beats_subsampling(frame, &picture)? {
                return Ok(Some(picture));
            }
            // Light content, such as grey text, fits finer levels than dense
            // text and may beat the stream only at one of them.
            for &finer in LEVELS[..index].iter().rev() {
                let picture = encode(finer)?;
                if picture.len() > budget {
                    break;
                }
                if self.beats_subsampling(frame, &picture)? {
                    return Ok(Some(picture));
                }
            }
            return Ok(None);
        }
        Ok(None)
    }

    /// Whether `picture`, decoded, is closer to `frame` by [`BETTER_BY`] than
    /// a 4:2:0 round trip of it in the stream's colour. A frame 4:2:0 cannot
    /// hold, of odd size, has no such bound: any still beats it.
    fn beats_subsampling(&self, frame: &RawFrame, picture: &[u8]) -> Result<bool, Error> {
        let Ok(nv12) = crate::bgra_to_nv12(frame, STILL_SIGNAL.yuv_matrix()) else {
            return Ok(true);
        };
        let subsampled = crate::nv12_to_bgra(
            frame.width(),
            frame.height(),
            0,
            nv12.y_plane(),
            nv12.uv_plane(),
            STILL_SIGNAL,
        )?;
        let shown = decode_i444(picture)?;
        Ok(crate::measure::psnr(frame, &shown)
            > crate::measure::psnr(frame, &subsampled) + BETTER_BY)
    }

    fn encode_at(
        &self,
        width: u16,
        height: u16,
        y: &[u8],
        u: &[u8],
        v: &[u8],
        level: u32,
    ) -> Result<Vec<u8>, Error> {
        let mut config =
            aom::EncoderConfig::new(width.into(), height.into(), aom::ImageFormat::I444);
        // AV1 Main is 4:2:0 only; High adds 4:4:4.
        config.g_profile = 1;
        config.g_usage = aom::Usage::Realtime;
        config.g_timebase = aom::AomRational { num: 1, den: 1 };
        config.g_threads = Some(self.threads);
        config.g_lag_in_frames = Some(0);
        config.cpu_used = Some(10);
        config.rc_end_usage = aom::RateControlMode::Q;
        config.cq_level = Some(level);
        config.rc_min_quantizer = level;
        config.rc_max_quantizer = level;
        config.tune_content = Some(aom::ContentType::Screen);
        config.color_primaries = Some(STILL_SIGNAL.primaries.into());
        config.transfer_characteristics = Some(STILL_SIGNAL.transfer.into());
        config.matrix_coefficients = Some(STILL_SIGNAL.matrix.into());
        config.color_range = Some(u32::from(STILL_SIGNAL.full_range));
        let mut encoder =
            aom::Encoder::new(config).map_err(|_| Error::Unsupported("libaom 4:4:4 encoder"))?;
        encoder
            .encode(
                &aom::ImageData::I444 { y, u, v },
                &aom::EncodeOptions {
                    force_keyframe: true,
                },
            )
            .map_err(|_| Error::EncodeRejected("libaom still picture"))?;
        let mut picture = Vec::new();
        drain(&mut encoder, &mut picture)?;
        encoder
            .finish()
            .map_err(|_| Error::EncodeRejected("libaom still picture"))?;
        drain(&mut encoder, &mut picture)?;
        if picture.is_empty() {
            return Err(Error::EncodeRejected("libaom produced no still picture"));
        }
        Ok(picture)
    }
}

/// Makes still pictures on a thread of its own, so the stream never waits
/// for one: about 50 ms at 1080p and 100 ms at 4K a try on a first-generation
/// Apple silicon Mac (M1, 2026-10-22). Each is tagged with the change it was
/// asked for after; the caller throws away one made before the screen changed
/// again.
pub(crate) struct StillWorker {
    jobs: Option<mpsc::SyncSender<Job>>,
    made: mpsc::Receiver<Made>,
    thread: Option<JoinHandle<()>>,
}

struct Job {
    change: u64,
    frame: RawFrame,
    /// The stream's encoded size: the still matches its picture.
    size: (u16, u16),
    budget: usize,
}

/// A still picture made for `change`, or `None` when none fit its budget
/// or the encoder refused.
pub(crate) struct Made {
    pub(crate) change: u64,
    pub(crate) timestamp_us: u64,
    pub(crate) picture: Option<Vec<u8>>,
}

impl StillWorker {
    pub(crate) fn new() -> Result<Self, Error> {
        let (jobs, inbox) = mpsc::sync_channel::<Job>(1);
        let (outbox, made) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("vnilink-still".into())
            .spawn(move || {
                let encoder = StillEncoder::new();
                for job in inbox {
                    // The stream's own size, so the still overlays its
                    // picture exactly; averaged, so text stays clean.
                    let frame = if (job.frame.width(), job.frame.height()) == job.size {
                        Ok(job.frame.clone())
                    } else {
                        area_scale(&job.frame, job.size)
                    };
                    let picture = frame
                        .and_then(|frame| encoder.encode(&frame, job.budget))
                        .ok()
                        .flatten();
                    let made = Made {
                        change: job.change,
                        timestamp_us: job.frame.timestamp_us(),
                        picture,
                    };
                    if outbox.send(made).is_err() {
                        return;
                    }
                }
            })
            .map_err(|_| Error::Unsupported("still picture thread"))?;
        Ok(Self {
            jobs: Some(jobs),
            made,
            thread: Some(thread),
        })
    }

    /// Asks for a still of `frame` within `budget` bytes; false while the
    /// last one asked for is still being made.
    pub(crate) fn make(
        &self,
        change: u64,
        frame: RawFrame,
        size: (u16, u16),
        budget: usize,
    ) -> bool {
        self.jobs.as_ref().is_some_and(|jobs| {
            jobs.try_send(Job {
                change,
                frame,
                size,
                budget,
            })
            .is_ok()
        })
    }

    /// A still made since last asked, if any.
    pub(crate) fn take(&self) -> Option<Made> {
        self.made.try_recv().ok()
    }
}

impl Drop for StillWorker {
    /// Waits for a still being made, at most one encoder's tries.
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            if thread.join().is_err() {
                tracing::warn!(target: vnilink::trace::STILL, "the still picture worker panicked");
            }
        }
    }
}

fn drain(encoder: &mut aom::Encoder, picture: &mut Vec<u8>) -> Result<(), Error> {
    while let Some(packet) = encoder.next_frame() {
        picture.extend_from_slice(
            packet
                .data()
                .map_err(|_| Error::EncodeRejected("libaom packet"))?,
        );
    }
    Ok(())
}

/// One AV1 4:4:4 picture back to BGRA in the stream's colour, as a viewer
/// shows it.
fn decode_i444(picture: &[u8]) -> Result<RawFrame, Error> {
    let rejected = |_| Error::EncodeRejected("libaom could not decode its still picture");
    let mut decoder = aom::Decoder::new(aom::DecoderConfig::new()).map_err(rejected)?;
    decoder.decode(picture).map_err(rejected)?;
    let decoded = decoder
        .next_frame()
        .ok_or(Error::EncodeRejected("libaom decoded no still picture"))?;
    let (width, height) = (decoded.width(), decoded.height());
    let planes = (
        decoded.y_plane().map_err(rejected)?,
        decoded.u_plane().map_err(rejected)?,
        decoded.v_plane().map_err(rejected)?,
    );
    let strides = (
        decoded.y_stride().map_err(rejected)?,
        decoded.u_stride().map_err(rejected)?,
        decoded.v_stride().map_err(rejected)?,
    );
    let to_rgb = ToRgb::of(STILL_SIGNAL);
    let clamp = |value: i32| (value >> 8).clamp(0, 255) as u8;
    let mut bgra = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for col in 0..width {
            let c = to_rgb.luma * (i32::from(planes.0[row * strides.0 + col]) - to_rgb.luma_offset);
            let d = i32::from(planes.1[row * strides.1 + col]) - 128;
            let e = i32::from(planes.2[row * strides.2 + col]) - 128;
            bgra.extend_from_slice(&[
                clamp(c + to_rgb.cb_blue * d + 128),
                clamp(c + to_rgb.cb_green * d + to_rgb.cr_green * e + 128),
                clamp(c + to_rgb.cr_red * e + 128),
                255,
            ]);
        }
    }
    RawFrame::bgra(width as u16, height as u16, 0, bgra)
}

/// `frame` scaled to `(width, height)` by averaging the source pixels each
/// destination pixel covers: a box filter, exact for whole ratios such as a
/// 4K screen at 1080p, which keeps text clean where nearest-neighbour would
/// drop strokes.
pub(crate) fn area_scale(frame: &RawFrame, (width, height): (u16, u16)) -> Result<RawFrame, Error> {
    if width == 0 || height == 0 || width > frame.width() || height > frame.height() {
        return Err(Error::InvalidFrame("a still only scales down"));
    }
    // The source columns and rows each destination pixel covers.
    let spans = |from: u16, to: u16| -> Vec<(usize, usize)> {
        (0..usize::from(to))
            .map(|at| {
                let start = at * usize::from(from) / usize::from(to);
                let end = ((at + 1) * usize::from(from) / usize::from(to)).max(start + 1);
                (start, end)
            })
            .collect()
    };
    let (columns, rows) = (spans(frame.width(), width), spans(frame.height(), height));
    let mut bgra = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
    let mut sums = vec![[0u32; 4]; usize::from(width)];
    for &(top, bottom) in &rows {
        sums.iter_mut().for_each(|sum| *sum = [0; 4]);
        for row in top..bottom {
            let source = frame.row(row as u16);
            for (sum, &(left, right)) in sums.iter_mut().zip(&columns) {
                for pixel in source[left * 4..right * 4].chunks_exact(4) {
                    for channel in 0..4 {
                        sum[channel] += u32::from(pixel[channel]);
                    }
                }
            }
        }
        for (sum, &(left, right)) in sums.iter().zip(&columns) {
            let count = ((right - left) * (bottom - top)) as u32;
            bgra.extend(sum.iter().map(|total| ((total + count / 2) / count) as u8));
        }
    }
    RawFrame::bgra(width, height, frame.timestamp_us(), bgra)
}

/// BGRA to limited-range planar 4:4:4 in the still signal: one chroma sample per pixel.
fn bgra_to_i444(frame: &RawFrame) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pixels = usize::from(frame.width()) * usize::from(frame.height());
    let (mut y, mut u, mut v) = (
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
    );
    for row in 0..frame.height() {
        for pixel in frame.row(row).chunks_exact(4) {
            let (luma, blue, red) =
                crate::nv12::yuv(STILL_SIGNAL.yuv_matrix(), pixel[2], pixel[1], pixel[0]);
            y.push(luma);
            u.push(blue);
            v.push(red);
        }
    }
    (y, u, v)
}
