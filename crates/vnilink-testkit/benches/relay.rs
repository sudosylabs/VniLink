//! How a relayed connection performs on this computer: how long it takes
//! to connect through the relay, and how fast a file crosses it.

use std::time::{Duration, Instant};

use vnilink::{JobState, SessionPhase};
use vnilink_testkit::bench::{median, Better, Report};
use vnilink_testkit::files::{drive, finished, received, write_files};
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{wait_for, Pair, FILES};

const BYTES: usize = 64 * 1024 * 1024;

fn main() {
    let mut report = Report::new("relay");
    let relay = LocalRelay::start();
    let (host, controller) = FILES;
    let pair = Pair::relayed(relay.url(), host, controller);

    let mut connecting = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        let session = pair.connect_services();
        connecting.push(started.elapsed());
        pair.controller.core.disconnect_session(session.id).unwrap();
        wait_for("the connection to close", Duration::from_secs(10), || {
            let phase = pair
                .controller
                .core
                .controller_session(session.id)
                .ok()?
                .phase;
            (phase == SessionPhase::Closed).then_some(())
        });
    }
    report.record(
        "relayed connect p50",
        median(&mut connecting).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );

    drive(&pair);
    write_files(&pair.controller.root().join("set"), &[("large.bin", BYTES)]);
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let started = Instant::now();
    pair.controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
    let job = received(&pair.host.core);
    let done = finished(&pair.host.core, job, Duration::from_secs(600));
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    report.record(
        "relayed throughput",
        BYTES as f64 / started.elapsed().as_secs_f64() / 1e6,
        "MB/s",
        Better::Higher,
    );

    drop(pair);
    drop(relay);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
