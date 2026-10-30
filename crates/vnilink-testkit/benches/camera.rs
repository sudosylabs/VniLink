// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How quickly the Camera service brings the host's pictures to the
//! viewer on this computer: each picture's time from the camera to the
//! viewer at 30 a second, at a typical and a large size, and how many
//! arrive.
//!
//! With `--peer` the camera is the test agent's on another computer. Each
//! picture carries the agent's clock when it was sent, converted to this
//! computer's clock with the offset estimated just before each run
//! (`peer::ClockOffset`); a picture's time across is then known within half
//! the estimate's fastest round trip, which the run records with its peer.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use vnilink::{EncodedFrame, ServiceKind};
use vnilink_testkit::bench::{Better, Report};
use vnilink_testkit::camera::{config, CameraHost};
use vnilink_testkit::peer::{clock, ClockOffset, Remote};

const PICTURES: u64 = 150;
/// Questions of the agent's time behind each offset estimate.
const PINGS: usize = 20;
/// How long the agent may take to send its pictures.
const SEND_WAIT: Duration = Duration::from_secs(60);

/// Sends `PICTURES` of `bytes` each at 30 a second; their latencies and
/// how many arrived.
fn stream(host: &CameraHost, bytes: usize) -> (Vec<Duration>, usize) {
    let camera = host.open(config()).unwrap();
    let source = host.cameras.latest();
    let mut latency = Vec::new();
    for index in 0..PICTURES {
        let sent = Instant::now();
        // A keyframe every second, and whenever the host asks for one, as
        // the cameras' own encoders do.
        let keyframe = index % 30 == 0 || source.take_keyframe_request();
        source
            .publish(EncodedFrame::h264(keyframe, index, vec![7; bytes]).unwrap())
            .unwrap();
        if camera.recv_timeout(Duration::from_millis(500)).is_some() {
            latency.push(sent.elapsed());
        }
        std::thread::sleep(Duration::from_millis(33).saturating_sub(sent.elapsed()));
    }
    camera.close();
    // The camera is one viewer's at a time; the host lets it go after the close.
    vnilink_testkit::wait_for(
        "the host to release the camera",
        Duration::from_secs(10),
        || {
            host.pair
                .host
                .core
                .hosted_services()
                .ok()?
                .is_empty()
                .then_some(())
        },
    );
    let arrived = latency.len();
    (latency, arrived)
}

/// Opens the agent's camera, which sends `PICTURES` of `bytes` each at 30
/// a second stamped with its clock; their latencies on this computer's
/// clock and how many arrived.
fn stream_from(remote: &mut Remote, offset: &ClockOffset, bytes: usize) -> (Vec<f64>, usize) {
    remote
        .host
        .start(
            "camera",
            json!({ "pictures": PICTURES, "bytes": bytes, "fps": 30 }),
        )
        .expect("the camera scenario");
    let endpoint = remote.host.endpoint().to_owned();
    let connection = remote
        .suite
        .connect_services_to(&endpoint)
        .expect("a services connection to the agent");
    let owner = remote
        .suite
        .core
        .attach_controller_service(connection.id, connection.generation, ServiceKind::Camera)
        .expect("a camera service");
    let camera = remote
        .suite
        .core
        .open_camera_service(owner.id, owner.generation, config())
        .expect("the agent's camera");
    let camera = Arc::new(camera);
    let done = Arc::new(AtomicBool::new(false));
    let receiving = {
        let (camera, done, offset) = (Arc::clone(&camera), Arc::clone(&done), *offset);
        std::thread::spawn(move || {
            let mut latency = Vec::new();
            while !done.load(Ordering::Acquire) {
                if let Some(frame) = camera.recv_timeout(Duration::from_millis(20)) {
                    // The stamp is the agent's clock when it sent the picture.
                    // Signed: within the clock's error a time may come out
                    // below zero, and is kept as it came out.
                    let arrived = clock::now_us() as f64;
                    let sent = offset.suite_us(frame.timestamp_us) as f64;
                    latency.push((arrived - sent) / 1e3);
                }
            }
            latency
        })
    };
    let sent = remote.host.wait("sent", SEND_WAIT);
    // The last pictures are on their way.
    std::thread::sleep(Duration::from_millis(500));
    done.store(true, Ordering::Release);
    let latency = receiving.join().expect("the receiving thread");
    camera.close();
    remote
        .suite
        .core
        .disconnect_session(connection.id)
        .expect("the connection ends");
    let records = remote.host.stop().expect("the scenario ends");
    let sent = sent.unwrap_or_else(|error| {
        panic!(
            "the agent did not send its pictures: {error}\nthe agent recorded:\n{}",
            records.join("\n")
        )
    });
    assert_eq!(
        sent["sent"], PICTURES,
        "the agent sent every picture: {sent}"
    );
    let arrived = latency.len();
    (latency, arrived)
}

/// The `percent` percentile of `samples`, in ms, which may be below zero
/// within a clock's error; zero for none.
fn percentile_ms(samples: &mut [f64], percent: usize) -> f64 {
    samples.sort_by(f64::total_cmp);
    if samples.is_empty() {
        return 0.0;
    }
    samples[(samples.len() * percent / 100).min(samples.len() - 1)]
}

fn main() {
    let mut remote = Remote::from_env();
    let mut report = Report::new("camera");
    let host = remote.is_none().then(CameraHost::start);
    let mut bound_us: Option<u64> = None;
    for (name, bytes) in [("picture", 30 * 1024), ("large picture", 400 * 1024)] {
        let (mut latency, arrived) = match (remote.as_mut(), host.as_ref()) {
            (Some(remote), _) => {
                let offset = remote.host.clock_offset(PINGS).expect("the agent's clock");
                let streamed = stream_from(remote, &offset, bytes);
                // Estimated again after the run: the clocks drift apart meanwhile.
                let after = remote.host.clock_offset(PINGS).expect("the agent's clock");
                let bound = offset.bound_us(&after);
                bound_us = Some(bound_us.map_or(bound, |worst| worst.max(bound)));
                streamed
            }
            (None, Some(host)) => {
                let (latency, arrived) = stream(host, bytes);
                (
                    latency
                        .iter()
                        .map(|sample| sample.as_secs_f64() * 1e3)
                        .collect(),
                    arrived,
                )
            }
            (None, None) => unreachable!("a camera here when no peer is named"),
        };
        report.record(
            &format!("{name} latency p50"),
            percentile_ms(&mut latency, 50),
            "ms",
            Better::Lower,
        );
        report.record(
            &format!("{name} latency p95"),
            percentile_ms(&mut latency, 95),
            "ms",
            Better::Lower,
        );
        report.record(
            &format!("{name} delivered"),
            arrived as f64 / PICTURES as f64 * 100.0,
            "%",
            Better::Higher,
        );
    }
    drop(host);
    if let Some(remote) = remote.as_ref() {
        report.against(match bound_us {
            Some(bound) => remote.bench_peer_timed(bound),
            None => remote.bench_peer(),
        });
    }
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
