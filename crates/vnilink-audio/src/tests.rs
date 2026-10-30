// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use super::*;
use std::time::Instant;

fn tone(offset: usize, channels: usize) -> Vec<f32> {
    (0..FRAME_SAMPLES)
        .flat_map(|n| {
            let v = ((offset + n) as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32)
                .sin()
                * 0.25;
            (0..channels).map(move |c| if c == 0 { v } else { -v })
        })
        .collect()
}
fn packets(count: usize, format: Format) -> Vec<AudioPacket> {
    let mut encoder = AudioEncoder::new(format, false).unwrap();
    (0..count)
        .map(|i| {
            encoder
                .encode(&tone(i * FRAME_SAMPLES, format.channels()))
                .unwrap()
        })
        .collect()
}

#[test]
fn actual_opus_round_trip_preserves_tone_and_stereo_with_bounded_packets() {
    for format in [Format::Mono, Format::Stereo] {
        let frames = packets(15, format);
        let mut decoder = opus::Decoder::new(SAMPLE_RATE, format.opus()).unwrap();
        let mut output = Vec::new();
        for (i, packet) in frames.into_iter().enumerate() {
            assert_eq!(packet.sequence, i as u64);
            assert_eq!(packet.timestamp, (i * FRAME_SAMPLES) as u64);
            let wire = packet.encode().unwrap();
            assert!(wire.len() <= 1217);
            let decoded_packet = AudioPacket::decode(&wire).unwrap();
            assert_eq!(packet, decoded_packet);
            let mut samples = vec![0.0; FRAME_SAMPLES * format.channels()];
            assert_eq!(
                decoder
                    .decode_float(&packet.payload, &mut samples, false)
                    .unwrap(),
                FRAME_SAMPLES
            );
            assert!(samples.iter().all(|v| v.is_finite()));
            output.extend(samples);
        }
        // Codec has lookahead. Compare after warmup at the known Audio mode
        // delay reported by libopus, instead of demanding lossless PCM.
        let encoder =
            opus::Encoder::new(SAMPLE_RATE, format.opus(), opus::Application::Audio).unwrap();
        let mut encoder = encoder;
        let delay = encoder.get_lookahead().unwrap() as usize;
        let mut error = 0.0;
        let mut power = 0.0;
        for n in 3 * FRAME_SAMPLES..14 * FRAME_SAMPLES {
            let expected =
                (n as f32 * 440.0 * std::f32::consts::TAU / SAMPLE_RATE as f32).sin() * 0.25;
            for c in 0..format.channels() {
                let target = if c == 0 { expected } else { -expected };
                let actual = output[(n + delay) * format.channels() + c];
                error += (actual - target).powi(2);
                power += target.powi(2);
            }
        }
        assert!(
            error / power < 0.025,
            "tone normalized error {}",
            error / power
        );
    }
}

#[test]
fn arriving_packets_decode_oldest_first_and_repeats_or_late_ones_are_ignored() {
    let frames = packets(4, Format::Mono);
    let mut player = AudioPlayback::new(Format::Mono).unwrap();
    assert!(player.poll().unwrap().is_none(), "nothing waits");
    player.push(frames[1].clone()).unwrap();
    player.push(frames[0].clone()).unwrap();
    player.push(frames[1].clone()).unwrap();
    assert_eq!(player.poll().unwrap().unwrap().timestamp, 0, "oldest first");
    player.set_muted(true);
    let muted = player.poll().unwrap().unwrap();
    assert_eq!(muted.timestamp, 960);
    assert!(
        muted.samples.iter().all(|&v| v == 0.0),
        "muted sound decodes silent"
    );
    player.set_muted(false);
    player.push(frames[0].clone()).unwrap();
    player.push(frames[3].clone()).unwrap();
    let after_gap = player.poll().unwrap().unwrap();
    assert_eq!(
        after_gap.timestamp, 2880,
        "a skipped packet is not waited for"
    );
    assert!(after_gap.samples.iter().any(|v| v.abs() > 0.01));
    assert_eq!(
        player.stats(),
        PlaybackStats {
            received: 3,
            duplicate: 1,
            late: 1,
            discontinuities: 1,
            ..Default::default()
        }
    );
}

#[test]
fn a_backlog_keeps_the_newest_sixty_milliseconds_and_a_restarted_count_plays() {
    // Two seconds of packets: more than a sender's count can fall behind.
    let frames = packets(100, Format::Stereo);
    let mut player = AudioPlayback::new(Format::Stereo).unwrap();
    for frame in &frames {
        player.push(frame.clone()).unwrap();
    }
    assert_eq!(player.stats().queued, 3, "only the newest wait");
    assert_eq!(player.stats().overflow, 97);
    assert_eq!(player.poll().unwrap().unwrap().timestamp, 97 * 960);
    let mut restarted = AudioEncoder::new(Format::Stereo, false).unwrap();
    player.push(restarted.encode(&tone(0, 2)).unwrap()).unwrap();
    assert_eq!(
        player.poll().unwrap().unwrap().timestamp,
        0,
        "a sender that counts from zero again is heard, not called late"
    );
    assert_eq!(player.stats().late, 0);
}

#[test]
fn packet_and_sample_formats_are_checked() {
    let packet = packets(1, Format::Mono).remove(0);
    let wire = packet.encode().unwrap();
    for len in 0..18 {
        assert!(AudioPacket::decode(&wire[..len]).is_err());
    }
    let mut invalid = packet.clone();
    invalid.timestamp = 1;
    assert!(invalid.validate().is_err());
    invalid = packet;
    invalid.payload = vec![0; 1201];
    assert!(invalid.validate().is_err());
    let mut encoder = AudioEncoder::new(Format::Mono, true).unwrap();
    assert!(encoder.encode(&[0.0; 959]).is_err());
    assert!(encoder.encode(&[f32::NAN; 960]).is_err());
    assert!(encoder.encode(&[1.1; 960]).is_err());
    assert_eq!(encoder.encode(&[0.0; 960]).unwrap().sequence, 0);
}

#[test]
fn sinc_resampling_preserves_frequency_and_stereo_at_44100_and_48000() {
    let mut resampler = AudioResampler::new(44100, 48000, Format::Stereo, 441).unwrap();
    let mut output = Vec::new();
    for i in 0..100 {
        let samples: Vec<_> = (0..441)
            .flat_map(|n| {
                let value =
                    (((i * 441 + n) as f32) * 440.0 * std::f32::consts::TAU / 44100.0).sin() * 0.25;
                [value, -value]
            })
            .collect();
        output.extend(resampler.process(&samples).unwrap());
    }
    let frames = output.len() / 2;
    assert!(
        (47800..=48010).contains(&frames),
        "{} output frames",
        frames
    );
    // Rubato's output_delay is buffering latency, not leading PCM silence.
    // The sinc window begins at the first source sample; phase error at this
    // frequency must remain below one output sample after streaming warmup.
    let mut error = 0.0;
    let mut power = 0.0;
    for n in 2000..frames - 1 {
        let target = (n as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.25;
        let actual = output[n * 2];
        error += (target - actual).powi(2);
        power += target.powi(2);
        assert!((output[n * 2 + 1] + actual).abs() < 0.00001);
    }
    assert!(error / power < 0.01, "normalized error {}", error / power);
    assert!(resampler.set_drift_ppm(5000.1).is_err());
    assert!(resampler.set_drift_ppm(f64::NAN).is_err());
    assert!(resampler.process(&[0.0; 2]).is_err());
}

#[test]
#[cfg(feature = "native")]
#[ignore = "opens only explicitly selected native audio devices"]
fn explicit_native_device_loopback_transports_and_decodes_tone() {
    use std::{thread, time::Duration};
    let input = std::env::var("VNILINK_AUDIO_CAPTURE_ID").expect("explicit capture ID required");
    let output = std::env::var("VNILINK_AUDIO_OUTPUT_ID").expect("explicit output ID required");
    #[cfg(target_os = "linux")]
    {
        let devices = audio_devices().unwrap();
        assert!(devices
            .iter()
            .any(|d| d.id == input && d.direction == DeviceDirection::Loopback));
        assert!(!devices
            .iter()
            .any(|d| d.id == input && d.direction == DeviceDirection::Input));
        assert!(
            NativeCapture::open(&input, DeviceDirection::Input, Format::Stereo, false).is_err()
        );
    }
    // Native playback and capture run on independent workers, as production
    // does. A slow capture/decode iteration must not burst stale playback.
    let (ready, opened) = std::sync::mpsc::sync_channel(1);
    let (begin, start_playback) = std::sync::mpsc::sync_channel(1);
    let output_worker = thread::spawn(move || {
        let mut playback = NativeOutput::open(&output, Format::Stereo).unwrap();
        ready.send(()).unwrap();
        start_playback.recv().unwrap();
        let start = Instant::now();
        let mut sequence = 0;
        while start.elapsed() < Duration::from_secs(3) {
            playback.push(&tone(sequence * 960, 2)).unwrap();
            sequence += 1;
            let deadline = start + Duration::from_millis(sequence as u64 * 20);
            if let Some(wait) = deadline.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            } else {
                sequence = (start.elapsed().as_millis() / 20) as usize + 1;
            }
        }
        assert!(playback.queued_samples().unwrap() <= 19200);
        let closing = Instant::now();
        drop(playback);
        assert!(
            closing.elapsed() < Duration::from_secs(2),
            "output cleanup took {:?}",
            closing.elapsed()
        );
    });
    opened.recv_timeout(Duration::from_secs(8)).unwrap();
    let direction = if cfg!(target_os = "linux") {
        DeviceDirection::Loopback
    } else {
        DeviceDirection::Input
    };
    let mut capture = NativeCapture::open(&input, direction, Format::Stereo, false).unwrap();
    begin.send(()).unwrap();
    let start = Instant::now();
    let mut decoder = opus::Decoder::new(SAMPLE_RATE, opus::Channels::Stereo).unwrap();
    let mut count = 0;
    let mut power = 0.0f64;
    while start.elapsed() < Duration::from_secs(3) {
        if let Some(packet) = capture.next_packet().unwrap() {
            let wire = packet.encode().unwrap();
            let packet = AudioPacket::decode(&wire).unwrap();
            let mut samples = vec![0.0; 1920];
            assert_eq!(
                decoder
                    .decode_float(&packet.payload, &mut samples, false)
                    .unwrap(),
                960
            );
            power += samples.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            count += 1;
        } else {
            thread::sleep(Duration::from_millis(2));
        }
    }
    assert!(start.elapsed() < Duration::from_secs(10));
    assert!(count >= 100, "only {count} captured packets");
    assert!(
        power / (count * 1920) as f64 > 0.001,
        "silence instead of test tone"
    );
    let closing = Instant::now();
    drop(capture);
    assert!(
        closing.elapsed() < Duration::from_secs(2),
        "capture cleanup took {:?}",
        closing.elapsed()
    );
    output_worker.join().unwrap();
}

#[test]
fn bursty_system_audio_plays_every_packet_once_and_resumes_after_silence() {
    let frames = packets(10, Format::Stereo);
    let mut player = AudioPlayback::new(Format::Stereo).unwrap();
    // A native 50 ms callback completes two 20 ms packets, the next three;
    // the playback worker polls between arrivals. Nothing is invented or lost.
    let mut decoded = 0;
    let mut sent = frames.iter();
    for burst in [2, 3, 2, 3] {
        for frame in sent.by_ref().take(burst) {
            player.push(frame.clone()).unwrap();
        }
        while player.poll().unwrap().is_some() {
            decoded += 1;
        }
    }
    assert_eq!(decoded, 10, "200 ms of captured sound remains 200 ms");
    assert_eq!(player.stats().late, 0);
    assert!(player.poll().unwrap().is_none(), "silence invents nothing");
    let mut encoder = AudioEncoder::new(Format::Stereo, false).unwrap();
    let resumed = (0..=10)
        .map(|_| encoder.encode(&tone(0, 2)).unwrap())
        .last()
        .unwrap();
    player.push(resumed).unwrap();
    assert!(
        player.poll().unwrap().is_some(),
        "sound after silence plays"
    );
}
