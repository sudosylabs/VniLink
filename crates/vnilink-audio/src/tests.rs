use super::*;
use std::time::{Duration, Instant};

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
fn jitter_reorders_duplicates_conceals_loss_and_consumes_while_muted() {
    let now = Instant::now();
    let frames = packets(8, Format::Mono);
    let mut player = AudioPlayback::new(Format::Mono).unwrap();
    player.push(frames[1].clone(), now).unwrap();
    player
        .push(frames[0].clone(), now + Duration::from_millis(5))
        .unwrap();
    player.push(frames[1].clone(), now).unwrap();
    player.push(frames[3].clone(), now).unwrap();
    assert!(player
        .poll(now + Duration::from_millis(59))
        .unwrap()
        .is_none());
    assert_eq!(
        player
            .poll(now + Duration::from_millis(60))
            .unwrap()
            .unwrap()
            .timestamp,
        0
    );
    player.set_muted(true);
    let muted = player
        .poll(now + Duration::from_millis(80))
        .unwrap()
        .unwrap();
    assert_eq!(muted.timestamp, 960);
    assert!(muted.samples.iter().all(|&v| v == 0.0));
    player.set_muted(false);
    let lost = player
        .poll(now + Duration::from_millis(100))
        .unwrap()
        .unwrap();
    assert!(lost.concealed);
    assert!(lost.samples.iter().all(|v| v.is_finite()));
    let recovered = player
        .poll(now + Duration::from_millis(120))
        .unwrap()
        .unwrap();
    assert!(!recovered.concealed);
    assert_eq!(recovered.timestamp, 2880);
    assert!(recovered.samples.iter().any(|v| v.abs() > 0.01));
    player.push(frames[0].clone(), now).unwrap();
    assert_eq!(
        player.stats(),
        PlaybackStats {
            received: 3,
            duplicate: 1,
            concealed: 1,
            late: 1,
            ..Default::default()
        }
    );
}

#[test]
fn playback_bounds_far_future_and_discards_stalled_backlog() {
    let frames = packets(40, Format::Stereo);
    let now = Instant::now();
    let mut player = AudioPlayback::new(Format::Stereo).unwrap();
    for frame in &frames {
        player.push(frame.clone(), now).unwrap();
    }
    assert_eq!(player.stats().queued, 16);
    assert_eq!(player.stats().overflow, 24);
    assert!(player.poll(now + Duration::from_secs(1)).unwrap().is_none());
    assert_eq!(player.stats().queued, 0);
    assert_eq!(player.stats().discontinuities, 1);
    player
        .push(frames[39].clone(), now + Duration::from_secs(1))
        .unwrap();
    let block = player
        .poll(now + Duration::from_millis(1060))
        .unwrap()
        .unwrap();
    assert_eq!(block.timestamp, 39 * 960);
    assert!(!block.concealed);
}

#[test]
fn long_loss_becomes_silence_and_packet_or_sample_format_is_checked() {
    let packet = packets(1, Format::Mono).remove(0);
    let now = Instant::now();
    let mut player = AudioPlayback::new(Format::Mono).unwrap();
    player.push(packet.clone(), now).unwrap();
    for i in 0..8 {
        let block = player
            .poll(now + Duration::from_millis(60 + i * 20))
            .unwrap()
            .unwrap();
        assert_eq!(block.concealed, i != 0);
        if i >= 4 {
            assert!(block.samples.iter().all(|&v| v == 0.0));
        }
    }
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
    assert!(resampler.set_drift_ppm(200.1).is_err());
    assert!(resampler.set_drift_ppm(f64::NAN).is_err());
    assert!(resampler.process(&[0.0; 2]).is_err());
}

#[test]
fn bounded_drift_servo_stabilizes_faster_and_slower_device_clocks() {
    for device_ppm in [-100.0, 100.0] {
        let target = 2880.0;
        let mut queued = target;
        let mut correction = 0.0;
        for _ in 0..30_000 {
            // Ten minutes of 20 ms blocks, no wall-clock wait.
            correction = crate::resample::drift_step(correction, (target - queued) / target);
            queued += 960.0 * (correction - device_ppm) / 1_000_000.0;
            assert!((1200.0..=4560.0).contains(&queued));
            assert!(correction.abs() <= 200.0);
        }
        assert!(
            (correction - device_ppm).abs() < 15.0,
            "device {device_ppm}, correction {correction}"
        );
    }
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
fn bursty_system_audio_and_silent_pause_resume_keep_one_capture_clock() {
    let now = Instant::now();
    let frames = packets(10, Format::Stereo);
    let mut player = AudioPlayback::new(Format::Stereo).unwrap();
    player.allow_silent_gaps();
    // A native 50 ms callback completes two 20 ms packets and retains 10 ms.
    // The following callback completes three; no frame is invented in between.
    let mut received = 0;
    let mut decoded = 0;
    for ms in (0..=200).step_by(10) {
        let count = match ms {
            0 => 2,
            50 => 3,
            100 => 2,
            150 => 3,
            _ => 0,
        };
        for _ in 0..count {
            player
                .push(frames[received].clone(), now + Duration::from_millis(ms))
                .unwrap();
            received += 1;
        }
        if let Some(block) = player.poll(now + Duration::from_millis(ms)).unwrap() {
            if !block.concealed {
                decoded += 1;
            }
        }
    }
    for ms in (220..=500).step_by(20) {
        if let Some(block) = player.poll(now + Duration::from_millis(ms)).unwrap() {
            if !block.concealed {
                decoded += 1;
            }
        }
    }
    assert_eq!(decoded, 10, "200 ms of captured PCM must remain 200 ms");
    assert_eq!(player.stats().received, 10);
    assert_eq!(player.stats().late, 0);
    assert!(player.poll(now + Duration::from_secs(5)).unwrap().is_none());
    let mut resumed_encoder = AudioEncoder::new(Format::Stereo, false).unwrap();
    let resumed = (0..=10)
        .map(|_| resumed_encoder.encode(&tone(0, 2)).unwrap())
        .last()
        .unwrap();
    let resumed_at = now + Duration::from_secs(5);
    player.set_muted(true);
    player.push(resumed, resumed_at).unwrap();
    let block = player
        .poll(resumed_at + Duration::from_millis(60))
        .unwrap()
        .unwrap();
    assert!(
        !block.concealed,
        "resumed packets must not be classified as stale"
    );
    assert!(block.samples.iter().all(|sample| *sample == 0.0));
    assert_eq!(player.stats().late, 0);
}
