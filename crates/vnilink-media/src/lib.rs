//! Frame pacing and H.264 Annex-B for VniLink platform adapters.
//!
//! This crate does not capture the screen and does not inject input. A
//! platform encoder turns BGRA pictures into Annex-B. [`FramePump`] reads
//! [`vnilink::Quality`] and drops pictures that arrive faster than the
//! configured frame rate, except when a keyframe is still owed. Keyframes
//! carry SPS, PPS, and an IDR so another operating system can start decoding.
//!
//! Pixel buffers and bitstreams are screen content. Do not log them.

#![forbid(unsafe_code)]

mod software;
pub use software::{
    probe_software_codecs, CodecEncoder, SoftwareDecoder, SoftwareDecoderSlot, SoftwareEncoder,
};
mod hevc;
pub use hevc::{hevc_annex_b, hevc_sample, hevc_units};
mod annexb;
mod channel;
mod controller;
mod cursor;
mod desktop;
mod error;
mod fanout;
mod frame;
mod host;
mod nv12;
mod pump;
mod rate;
mod recording;
mod recording_format;
mod recording_segments;
pub use recording_segments::{
    RecordingFailure, RecordingRetention, RecordingSnapshot, SegmentWriter,
};
/// Pictures and comparisons for the platform encoders' measurements.
#[doc(hidden)]
pub mod measure;
mod recovery;
pub use recovery::{RecoveryQp, RECOVERY_KEYFRAME_FRAMES};
mod recording_worker;
pub use recording_worker::{recover_desktop_recording, Recorder, RecordingPhase, RecordingStatus};
mod recording_recovery;
pub use recording_recovery::{scan_recording_prefix, RecordingRecovery};
mod sps;
pub use recording::{MatroskaRecorder, RecordingError, RecordingLimits};
pub use sps::{h264_low_delay_sps, h264_picture_size};

pub use annexb::{parse_annex_b, to_annex_b, AvccAccessUnit, NalUnit, ParsedAccessUnit};
pub use channel::{ChannelSlot, FrameChannel};
pub use controller::{serve_controller, ControllerOutput, ControllerVideo, DecodedFrame};
pub use desktop::{CapturePlan, CursorPolicy, DesktopKind, MonitorSize};
pub use error::{Error, PublishError, FRAME_DROPPED};
pub use fanout::serve_shared_capture;
pub use frame::{encoded_size, scale_bgra, RawFrame};
pub use host::{serve_capture, serve_desktop_capture, FrameSource, HostSession, OpenEncoder};
pub use nv12::{bgra_to_nv12, nv12_to_bgra, Nv12};
pub use pump::{
    CoreSink, EncodeParams, EncodedSample, FramePump, FrameSink, HostVideo, PumpOutput,
    VideoDecoder, VideoEncoder,
};
pub use rate::{EncoderSettings, RateChange, RateController};
pub use vnilink::Quality;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
