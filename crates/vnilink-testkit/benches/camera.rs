//! How quickly the Camera service brings the host's pictures to the
//! viewer on this computer: each picture's time from the camera to the
//! viewer at 30 a second, at a typical and a large size, and how many
//! arrive.

use std::time::{Duration, Instant};

use vnilink::EncodedFrame;
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::camera::{config, CameraHost};

const PICTURES: u64 = 150;

/// Sends `PICTURES` of `bytes` each at 30 a second; their latencies and
/// how many arrived.
fn stream(host: &CameraHost, bytes: usize) -> (Vec<Duration>, usize) {
    let camera = host.open(config()).unwrap();
    let source = host.cameras.latest();
    let mut latency = Vec::new();
    for index in 0..PICTURES {
        let sent = Instant::now();
        source
            .publish(EncodedFrame::h264(index % 30 == 0, index, vec![7; bytes]).unwrap())
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

fn main() {
    let mut report = Report::new("camera");
    let host = CameraHost::start();
    for (name, bytes) in [("picture", 30 * 1024), ("large picture", 400 * 1024)] {
        let (mut latency, arrived) = stream(&host, bytes);
        report.record(
            &format!("{name} latency p50"),
            median(&mut latency).as_secs_f64() * 1e3,
            "ms",
            Better::Lower,
        );
        report.record(
            &format!("{name} latency p95"),
            percentile(&mut latency, 95).as_secs_f64() * 1e3,
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
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
