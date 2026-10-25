//! How quickly the host carries out a system action the controller asks
//! for: a lock's round trip, and turning privacy on and off.

use std::sync::Arc;
use std::time::Instant;

use vnilink::{SystemAction, SystemPolicy};
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::system::{RecordingSystem, SystemSession};

const TIMES: usize = 100;

fn main() {
    let mut report = Report::new("system");
    let session = SystemSession::open(Some(Arc::new(RecordingSystem::default())));
    session
        .pair
        .host
        .core
        .set_system_policy(SystemPolicy {
            allow_privacy: true,
            ..SystemPolicy::default()
        })
        .unwrap();
    let mut locks = Vec::with_capacity(TIMES);
    for _ in 0..TIMES {
        let started = Instant::now();
        session.service.execute(SystemAction::Lock).unwrap();
        locks.push(started.elapsed());
    }
    report.record(
        "lock p50",
        median(&mut locks).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "lock p95",
        percentile(&mut locks, 95).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    let mut privacy = Vec::with_capacity(TIMES);
    for index in 0..TIMES {
        let on = index % 2 == 0;
        let started = Instant::now();
        session.service.set_privacy(on, on).unwrap();
        privacy.push(started.elapsed());
    }
    report.record(
        "privacy change p50",
        median(&mut privacy).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    drop(session);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
