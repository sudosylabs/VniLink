//! Still pictures: one AV1 4:4:4 keyframe of a screen that stopped
//! changing, sharp where the stream's 4:2:0 blurs coloured text. The viewer
//! shows it until the screen changes ([`vnilink::FrameRole::Still`]).
//!
//! The picture is encoded at the capture's own size, in the stream's colour
//! (BT.709, limited range, [`crate::STREAM_SIGNAL`]), and is screen content:
//! do not log it.

use std::sync::mpsc;
use std::thread::JoinHandle;

use shiguredo_aom as aom;

use crate::colour::STREAM_SIGNAL;
use crate::error::Error;
use crate::frame::RawFrame;

/// How long the screen stays unchanged before its still picture is made:
/// the stream's own refinement of a still screen ends then.
pub const STILL_AFTER: std::time::Duration =
    std::time::Duration::from_micros(crate::pump::REFINE_FOR_US);

/// AV1 quantizer levels tried, finest first, until a picture fits. On a
/// 1080p page of coloured text they give about 40, 36 and 32 dB, against
/// 25 for the 4:2:0 stream; a coarser still is not worth its bytes.
const LEVELS: [u32; 3] = [20, 28, 36];

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

    /// One AV1 4:4:4 keyframe of `frame` of at most `budget` bytes, at the
    /// finest quality that fits; `None` when even the coarsest does not.
    pub fn encode(&self, frame: &RawFrame, budget: usize) -> Result<Option<Vec<u8>>, Error> {
        let (y, u, v) = bgra_to_i444(frame);
        for level in LEVELS {
            let picture = self.encode_at(frame.width(), frame.height(), &y, &u, &v, level)?;
            if picture.len() <= budget {
                return Ok(Some(picture));
            }
        }
        Ok(None)
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
        config.color_primaries = Some(STREAM_SIGNAL.primaries.into());
        config.transfer_characteristics = Some(STREAM_SIGNAL.transfer.into());
        config.matrix_coefficients = Some(STREAM_SIGNAL.matrix.into());
        config.color_range = Some(u32::from(STREAM_SIGNAL.full_range));
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
/// for one: about 50 ms at 1080p and 100 ms at 4K a try on an M1. Each is
/// tagged with the change it was asked for after; the caller throws away
/// one made before the screen changed again.
pub(crate) struct StillWorker {
    jobs: Option<mpsc::SyncSender<Job>>,
    made: mpsc::Receiver<Made>,
    thread: Option<JoinHandle<()>>,
}

struct Job {
    change: u64,
    frame: RawFrame,
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
                    let picture = encoder.encode(&job.frame, job.budget).ok().flatten();
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
    pub(crate) fn make(&self, change: u64, frame: RawFrame, budget: usize) -> bool {
        self.jobs.as_ref().is_some_and(|jobs| {
            jobs.try_send(Job {
                change,
                frame,
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
            let _ = thread.join();
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

/// BGRA to BT.709 limited-range planar 4:4:4: one chroma sample per pixel.
fn bgra_to_i444(frame: &RawFrame) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let pixels = usize::from(frame.width()) * usize::from(frame.height());
    let (mut y, mut u, mut v) = (
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
        Vec::with_capacity(pixels),
    );
    for row in 0..frame.height() {
        for pixel in frame.row(row).chunks_exact(4) {
            let (luma, blue, red) = crate::nv12::yuv(pixel[2], pixel[1], pixel[0]);
            y.push(luma);
            u.push(blue);
            v.push(red);
        }
    }
    (y, u, v)
}
