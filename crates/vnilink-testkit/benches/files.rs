//! How fast the Files service moves trees between two engines on this
//! computer: many small files, one large file, and a mix, uploaded with both
//! job drivers running as in the application.

use std::time::{Duration, Instant};

use vnilink::JobState;
use vnilink_testkit::bench::{Better, Report};
use vnilink_testkit::files::{drive, finished, received, write_files};
use vnilink_testkit::Pair;

/// Uploads `files` and returns how long the receiver took to finish.
fn upload(files: &[(String, usize)]) -> Duration {
    let pair = Pair::for_files();
    drive(&pair);
    let named: Vec<(&str, usize)> = files
        .iter()
        .map(|(name, size)| (name.as_str(), *size))
        .collect();
    write_files(&pair.controller.root().join("set"), &named);
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
    let receiver = received(&pair.host.core);
    let done = finished(&pair.host.core, receiver, Duration::from_secs(600));
    let took = started.elapsed();
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    took
}

fn many(count: usize, size: usize, prefix: &str) -> Vec<(String, usize)> {
    (0..count)
        .map(|index| (format!("{prefix}/f{index:05}.bin"), size))
        .collect()
}

fn main() {
    let mut report = Report::new("files");
    let mut measure = |name: &str, files: Vec<(String, usize)>| {
        let bytes: usize = files.iter().map(|(_, size)| size).sum();
        let took = upload(&files).as_secs_f64();
        report.record(
            &format!("{name} throughput"),
            bytes as f64 / took / 1e6,
            "MB/s",
            Better::Higher,
        );
        report.record(
            &format!("{name} files"),
            files.len() as f64 / took,
            "files/s",
            Better::Higher,
        );
    };
    measure("small", many(1_000, 4 * 1024, "small"));
    measure("large", many(1, 256 * 1024 * 1024, "large"));
    let mut mixed = many(500, 4 * 1024, "small");
    mixed.extend(many(40, 1024 * 1024, "medium"));
    mixed.extend(many(2, 32 * 1024 * 1024, "large"));
    measure("mixed", mixed);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
