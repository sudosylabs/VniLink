//! How fast the Files service moves trees between two engines: many small
//! files, one large file, and a mix, uploaded with both job drivers
//! running as in the application. Both engines run on this computer, or,
//! with `--peer`, the receiving one is the test agent on another computer.

use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink::JobState;
use vnilink_testkit::bench::{Better, Report};
use vnilink_testkit::files::{drive, finished, received, write_files};
use vnilink_testkit::peer::Remote;
use vnilink_testkit::{Pair, FILES};

/// How long one upload may take.
const UPLOAD_WAIT: Duration = Duration::from_secs(600);

/// Uploads `files` between two engines on this computer and returns how
/// long the receiver took to finish.
fn upload(files: &[(&str, usize)]) -> Duration {
    let pair = Pair::for_files();
    drive(&pair);
    write_files(&pair.controller.root().join("set"), files);
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller
        .core
        .connect_files(&pair.host.endpoint())
        .unwrap();
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
    let done = finished(&pair.host.core, receiver, UPLOAD_WAIT);
    let took = started.elapsed();
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    took
}

/// Uploads `files` to the agent and returns how long until the agent said
/// its receiver finished: the receiver's time plus the answer's way back.
fn upload_to(remote: &mut Remote, files: &[(&str, usize)]) -> Duration {
    let set = remote.suite.root().join("set");
    let _ = std::fs::remove_dir_all(&set);
    write_files(&set, files);
    let facts = remote
        .host
        .start("files", Value::Null)
        .expect("the files scenario");
    let (Some(root), Some(inbox)) = (facts["root"].as_u64(), facts["inbox"].as_str()) else {
        panic!("the files scenario said where uploads land: {facts}");
    };
    let endpoint = remote.host.endpoint().to_owned();
    let session = remote
        .suite
        .core
        .connect_files(&endpoint)
        .expect("a files session to the agent");
    let started = Instant::now();
    remote
        .suite
        .core
        .job_submit_upload(&endpoint, remote.suite.root_id(), "set", root, inbox)
        .expect("the upload");
    let done = remote.host.wait("received", UPLOAD_WAIT);
    let took = started.elapsed();
    remote
        .suite
        .core
        .disconnect_session(session.id)
        .expect("the session ends");
    let records = remote.host.stop().expect("the scenario ends");
    let done = done.unwrap_or_else(|error| {
        panic!(
            "the agent's upload did not end: {error}\nthe agent recorded:\n{}",
            records.join("\n")
        )
    });
    assert_eq!(done["state"], "Complete", "the agent's receiver: {done}");
    took
}

fn many(count: usize, size: usize, prefix: &str) -> Vec<(String, usize)> {
    (0..count)
        .map(|index| (format!("{prefix}/f{index:05}.bin"), size))
        .collect()
}

fn main() {
    let mut remote = Remote::from_env();
    let mut report = Report::new("files");
    if let Some(remote) = remote.as_mut() {
        let (_, controller) = FILES;
        remote
            .suite
            .core
            .set_desktop_sharing(controller)
            .expect("sharing");
        remote.suite.core.start_job_driver();
    }
    let mut measure = |name: &str, files: Vec<(String, usize)>| {
        let bytes: usize = files.iter().map(|(_, size)| size).sum();
        let named: Vec<(&str, usize)> = files
            .iter()
            .map(|(name, size)| (name.as_str(), *size))
            .collect();
        let took = match remote.as_mut() {
            Some(remote) => upload_to(remote, &named),
            None => upload(&named),
        }
        .as_secs_f64();
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
    // The path the connection settled on, once the runs gave it time to.
    if let Some(remote) = remote.as_ref() {
        report.against(remote.bench_peer());
    }
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
