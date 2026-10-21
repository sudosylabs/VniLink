//! Entry points for the fuzz targets in `crates/vnilink/fuzz`.
//!
//! Each one feeds bytes a paired peer could send to one parser and checks
//! what that parser promises of them; a broken promise panics, which the
//! fuzzer records as a crash. Not an API: present only with the `fuzzing`
//! feature, which production builds leave off.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use crate::adaptation::{Adaptive, Feedback, HostFeedback};
use crate::protocol::ControlMessage;
use crate::transfer::FileRequest;
use crate::video::{
    decode_video_body, fragment, QueueTelemetry, Reassembler, VideoReport, ViewerLoss, REPORT_LEN,
};
use crate::Quality;

/// Most parity a frame carries, as a share of its data shards.
pub const MAX_PROTECTION: f64 = crate::video::MAX_PROTECTION;

/// A control frame decodes only to a message that encodes to a frame
/// decoding to the same message.
pub fn control_message(bytes: &[u8]) {
    let Ok(message) = ControlMessage::decode(bytes) else {
        return;
    };
    let again = ControlMessage::decode(&message.encode());
    assert_eq!(again.ok(), Some(message), "a decoded message re-encodes");
}

/// Exactly the request's 20 bytes decode, back to themselves.
pub fn file_request(bytes: &[u8]) {
    if let Ok(request) = FileRequest::decode(bytes) {
        assert_eq!(request.encode().as_slice(), bytes);
    }
}

/// What an honest sender sends for `file` in segments of `segment`, a
/// power of two of at least 1 KiB.
pub fn honest_stream(file: &[u8], segment: u64) -> Vec<u8> {
    crate::transfer::stream_of(file, segment)
}

/// Receives `stream` in chunks of `chunks` (then the rest at once) as the
/// file `file` in segments of `segment`, a power of two of at least 1 KiB.
/// Only the honest stream may land, and it lands as the file; it lands
/// however it is chunked.
pub fn receive_stream(file: &[u8], segment: u64, stream: &[u8], chunks: &[usize]) {
    assert!(
        segment.is_power_of_two() && segment >= 1024,
        "a segment length"
    );
    let dir = scratch();
    let data = dir.join("data");
    let mut rest = stream;
    let mut pieces = Vec::new();
    for &len in chunks {
        if rest.is_empty() {
            break;
        }
        let (piece, after) = rest.split_at(len.clamp(1, rest.len()));
        pieces.push(piece);
        rest = after;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    let landed = crate::transfer::land(
        &data,
        &dir.join("journal"),
        file.len() as u64,
        segment,
        pieces,
    );
    let honest = stream == honest_stream(file, segment).as_slice();
    match landed {
        Ok(true) => {
            assert!(honest, "only the honest stream lands");
            if !file.is_empty() {
                assert_eq!(std::fs::read(&data).unwrap(), file, "it lands as the file");
            }
        }
        _ => assert!(!honest, "the honest stream lands: {landed:?}"),
    }
}

/// Feeds datagrams to one reassembler in order. Whatever completes is a
/// body the video-body parser then reads.
pub fn reassemble<'a>(datagrams: impl IntoIterator<Item = &'a [u8]>) {
    let mut reassembler = Reassembler::new();
    for datagram in datagrams {
        if let Some(body) = reassembler.push(datagram) {
            let _ = decode_video_body(&body);
        }
        let _ = reassembler.take_loss();
        let _ = reassembler.take_fragments();
    }
}

/// The datagrams a sender sends for `body`, data shards then parity.
pub fn fragments(body: &[u8], seq: u32, max_datagram: usize, protection: f64) -> Vec<Vec<u8>> {
    fragment(body, seq, max_datagram, protection)
}

/// Fragments `body` as a sender would, then feeds a fresh reassembler the
/// fragments `kept` says arrived (all, past its end), ordered by `order`
/// (stable; missing keys sort first). The frame comes back whole exactly
/// when at least its data count arrived, rebuilt from parity if need be.
pub fn rebuild(
    body: &[u8],
    seq: u32,
    max_datagram: usize,
    protection: f64,
    kept: &[bool],
    order: &[u16],
) {
    let fragments = fragment(body, seq, max_datagram, protection);
    let data = usize::from(u16::from_be_bytes([fragments[0][6], fragments[0][7]]));
    let mut arriving: Vec<usize> = (0..fragments.len())
        .filter(|&index| kept.get(index).copied().unwrap_or(true))
        .collect();
    arriving.sort_by_key(|&index| order.get(index).copied().unwrap_or(0));
    let mut reassembler = Reassembler::new();
    let mut whole = Vec::new();
    for &index in &arriving {
        whole.extend(reassembler.push(&fragments[index]));
    }
    if arriving.len() >= data {
        assert_eq!(whole, [body.to_vec()], "enough shards rebuild the frame");
    } else {
        assert!(whole.is_empty(), "too few shards rebuild nothing");
    }
}

/// Loss reports decode back to their bytes, and whatever a viewer reports
/// keeps the host's parity a finite share within bounds.
pub fn video_reports(reports: &[[u8; REPORT_LEN]]) {
    let mut loss = ViewerLoss::default();
    let now = Instant::now();
    for bytes in reports {
        let report = VideoReport::decode(bytes);
        let mut encoded = Vec::new();
        report.encode(&mut encoded);
        assert_eq!(encoded.as_slice(), bytes.as_slice());
        loss.observe(&report, now);
        for (keyframe, congested) in [(false, false), (true, false), (false, true), (true, true)] {
            let share = loss.protection(keyframe, congested, now);
            assert!(
                (0.0..=MAX_PROTECTION).contains(&share),
                "parity share {share} out of bounds"
            );
        }
    }
}

/// One interval of what the adaptation loop sees: the host's feedback as
/// it arrived, beside this side's own measurements.
#[derive(Debug, Clone, Copy)]
pub struct Interval<'a> {
    pub host: &'a [u8],
    pub received_frames: u64,
    pub received_bytes: u64,
    pub queued: u16,
    pub dropped: u64,
    pub oldest_ms: u64,
    pub frames_lost: u64,
    pub rtt_ms: u64,
    pub path_changed: bool,
    pub decode_us: Option<u64>,
    pub elapsed_ms: u64,
}

/// Host feedback decodes back to its bytes, and no feedback leads the
/// adaptation loop to a quality outside the agreed limits.
pub fn host_feedback(mut quality: Quality, source: (u16, u16), intervals: &[Interval<'_>]) {
    let mut adaptive = Adaptive::default();
    for interval in intervals {
        let host = HostFeedback::decode(interval.host).ok();
        if let Some(feedback) = host {
            assert_eq!(feedback.encode().as_slice(), interval.host);
        }
        let next = Feedback {
            receiver: QueueTelemetry {
                received_frames: interval.received_frames,
                received_bytes: interval.received_bytes,
                queued: interval.queued,
                dropped: interval.dropped,
                oldest_ms: interval.oldest_ms,
            },
            host,
            frames_lost: interval.frames_lost,
            rtt_ms: interval.rtt_ms,
            path_changed: interval.path_changed,
            decode_us: interval.decode_us,
        };
        if let Some(changed) = adaptive.step(quality, source, interval.elapsed_ms, next) {
            assert!(
                Quality::new(
                    changed.max_bitrate_kbps,
                    changed.max_frame_rate,
                    changed.max_width,
                    changed.max_height,
                )
                .is_ok(),
                "adaptation chose {changed:?}"
            );
            quality = changed;
        }
    }
}

/// A directory of this process for the partials the transfer target
/// writes, emptied on first use.
fn scratch() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("vnilink-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    })
}
