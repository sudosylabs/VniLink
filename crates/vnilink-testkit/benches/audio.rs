// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How the audio services sound on this computer: each packet's time from
//! the host's source to the controller, and how many are lost, at the
//! real pace of 20 ms packets.

use std::time::{Duration, Instant};

use vnilink::audio::AudioKind;
use vnilink::ServiceKind;
use vnilink_testkit::audio::{encoder, packet, AudioHost};
use vnilink_testkit::bench::{median, percentile, Better, Report};

const PACKETS: u64 = 250;
const PACE: Duration = Duration::from_millis(20);

fn main() {
    let mut report = Report::new("audio");
    let host = AudioHost::start(&[ServiceKind::SystemAudio]);
    let system = host.open(AudioKind::System).unwrap();
    let source = host.audio.latest(AudioKind::System);
    let mut encoding = encoder();
    let mut latency = Vec::new();
    let mut received = Vec::new();
    for _ in 0..PACKETS {
        let sent = Instant::now();
        source.publish(packet(&mut encoding)).unwrap();
        while sent.elapsed() < Duration::from_millis(500) {
            if let Some(arrived) = system.try_recv() {
                latency.push(sent.elapsed());
                received.push(arrived.sequence);
                break;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
        std::thread::sleep(PACE.saturating_sub(sent.elapsed()));
    }
    let lost = PACKETS - received.len() as u64;
    report.record(
        "packet latency p50",
        median(&mut latency).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "packet latency p95",
        percentile(&mut latency, 95).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "packets lost",
        lost as f64 / PACKETS as f64 * 100.0,
        "%",
        Better::Lower,
    );
    drop(system);
    drop(host);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
