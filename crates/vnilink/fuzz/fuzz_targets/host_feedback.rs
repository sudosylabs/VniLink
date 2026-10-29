//! `HostFeedback::decode` on the host's feedback, and the viewer's
//! adaptation loop on what decodes, beside its own measurements.
#![no_main]

use arbitrary::Arbitrary;

#[derive(Debug, Arbitrary)]
struct Input {
    quality: (u32, u8, u16, u16),
    ceiling: (u32, u8, u16, u16),
    intervals: Vec<Interval>,
}

#[derive(Debug, Arbitrary)]
struct Interval {
    host: Vec<u8>,
    received_frames: u64,
    received_bytes: u64,
    queued: u16,
    dropped: u64,
    oldest_ms: u64,
    frames_lost: u64,
    rtt_ms: u64,
    path_changed: bool,
    decode_us: Option<u64>,
    elapsed_ms: u64,
}

libfuzzer_sys::fuzz_target!(|input: Input| {
    let (bitrate, frame_rate, width, height) = input.quality;
    let Ok(quality) = vnilink::Quality::new(bitrate, frame_rate, width, height) else {
        return;
    };
    let (bitrate, frame_rate, width, height) = input.ceiling;
    let Ok(ceiling) = vnilink::Quality::new(bitrate, frame_rate, width, height) else {
        return;
    };
    let intervals: Vec<_> = input
        .intervals
        .iter()
        .map(|interval| vnilink::fuzzing::Interval {
            host: &interval.host,
            received_frames: interval.received_frames,
            received_bytes: interval.received_bytes,
            queued: interval.queued,
            dropped: interval.dropped,
            oldest_ms: interval.oldest_ms,
            frames_lost: interval.frames_lost,
            rtt_ms: interval.rtt_ms,
            path_changed: interval.path_changed,
            decode_us: interval.decode_us,
            elapsed_ms: interval.elapsed_ms,
        })
        .collect();
    vnilink::fuzzing::host_feedback(quality, ceiling, &intervals);
});
