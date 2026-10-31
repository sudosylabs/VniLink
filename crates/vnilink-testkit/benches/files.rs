// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How fast the Files service moves trees between two engines: many small
//! files, one large file, and a mix, uploaded with both job drivers
//! running as in the application. Both engines run on this computer, or,
//! with `--peer`, the receiving one is the test agent on another computer.
//! The `rtt` cases cross a relay that holds each byte, as a distant peer
//! does; `compressible` moves text that a compressing transfer would shrink.

use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink::{Grants, JobState};
use vnilink_testkit::bench::{Better, Report};
use vnilink_testkit::files::{self, drive, finished, received, write_files};
use vnilink_testkit::peer::Remote;
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{Pair, FILES};

/// How long one upload may take.
const UPLOAD_WAIT: Duration = Duration::from_secs(600);

/// The round trip the `rtt` cases inject.
const RTT: Duration = Duration::from_millis(40);

/// What a case's files hold.
#[derive(Clone, Copy)]
enum Content {
    /// Bytes that do not compress, as most files.
    Random,
    /// Repeating text.
    Text,
}

/// Writes `files` under `folder` with `content`.
fn write(folder: &std::path::Path, files: &[(&str, usize)], content: Content) {
    match content {
        Content::Random => write_files(folder, files),
        Content::Text => {
            let line = b"the quick brown fox jumps over the lazy dog, again and again\n";
            for (name, size) in files {
                let path = folder.join(name);
                std::fs::create_dir_all(path.parent().expect("a folder")).expect("a folder");
                let text: Vec<u8> = line.iter().cycle().take(*size).copied().collect();
                std::fs::write(path, text).expect("a file");
            }
        }
    }
}

/// Uploads `files` between two engines on this computer, across a relay
/// with `rtt` round trip time when given, and returns how long the
/// receiver took to finish.
fn upload(files: &[(&str, usize)], content: Content, rtt: Option<Duration>) -> Duration {
    let relay = rtt.map(|_| LocalRelay::start());
    let delayed = rtt
        .zip(relay.as_ref())
        .map(|(rtt, relay)| relay.delayed(rtt / 4));
    let pair = match &delayed {
        Some(delayed) => {
            let (host, controller) = FILES;
            Pair::relayed(delayed.url(), host, controller)
        }
        None => Pair::for_files(),
    };
    drive(&pair);
    write(&pair.controller.folder().join("set"), files, content);
    std::fs::create_dir_all(pair.host.folder().join("inbox")).unwrap();
    let service = pair.open_files();
    let started = Instant::now();
    files::upload(&pair, &service, "set", "inbox").unwrap();
    let receiver = received(&pair.host.core);
    let done = finished(&pair.host.core, receiver, UPLOAD_WAIT);
    let took = started.elapsed();
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    took
}

/// Uploads `files` to the agent and returns how long until the agent said
/// its receiver finished: the receiver's time plus the answer's way back.
fn upload_to(remote: &mut Remote, files: &[(&str, usize)]) -> Duration {
    let set = remote.suite.folder().join("set");
    let _ = std::fs::remove_dir_all(&set);
    write_files(&set, files);
    let facts = remote
        .host
        .start("files", Value::Null)
        .expect("the files scenario");
    let (Some(root), Some(inbox)) = (facts["root"].as_u64(), facts["inbox"].as_str()) else {
        panic!("the files scenario said where uploads land: {facts}");
    };
    // The agent fetches what it receives, so this computer shares Files
    // with it, its whole disk, only while the upload runs.
    let (_, controller) = FILES;
    remote
        .suite
        .core
        .set_desktop_sharing(controller)
        .expect("sharing Files with the agent");
    let service = remote
        .suite
        .files_at(remote.host.endpoint())
        .expect("the agent's Files");
    let here = remote.suite.location();
    let (folder, names) = files::source(&here.path("set"));
    let started = Instant::now();
    remote
        .suite
        .core
        .files_service_upload(
            service.service.id,
            service.service.generation,
            here.root,
            &folder,
            &names,
            root,
            inbox,
        )
        .expect("the upload");
    let done = remote.host.wait("received", UPLOAD_WAIT);
    let took = started.elapsed();
    remote
        .suite
        .core
        .disconnect_session(service.connection.id)
        .expect("the connection ends");
    remote
        .suite
        .core
        .set_desktop_sharing(Grants::default())
        .expect("sharing nothing with the agent");
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
        remote.suite.core.start_job_driver();
    }
    // Cases whose name contains the first argument that is no flag, as
    // `cargo bench --bench files -- small` runs only the small ones.
    let only = std::env::args()
        .skip(1)
        .find(|argument| !argument.starts_with('-'));
    let mut measure =
        |name: &str, files: Vec<(String, usize)>, content: Content, rtt: Option<Duration>| {
            if only
                .as_ref()
                .is_some_and(|only| !name.contains(only.as_str()))
            {
                return;
            }
            if remote.is_some() && (rtt.is_some() || matches!(content, Content::Text)) {
                return;
            }
            let bytes: usize = files.iter().map(|(_, size)| size).sum();
            let named: Vec<(&str, usize)> = files
                .iter()
                .map(|(name, size)| (name.as_str(), *size))
                .collect();
            let took = match remote.as_mut() {
                Some(remote) => upload_to(remote, &named),
                None => upload(&named, content, rtt),
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
    measure(
        "small",
        many(1_000, 4 * 1024, "small"),
        Content::Random,
        None,
    );
    measure(
        "large",
        many(1, 256 * 1024 * 1024, "large"),
        Content::Random,
        None,
    );
    let mut mixed = many(500, 4 * 1024, "small");
    mixed.extend(many(40, 1024 * 1024, "medium"));
    mixed.extend(many(2, 32 * 1024 * 1024, "large"));
    measure("mixed", mixed, Content::Random, None);
    measure("many", many(10_000, 1024, "many"), Content::Random, None);
    measure(
        "compressible",
        many(1, 128 * 1024 * 1024, "text"),
        Content::Text,
        None,
    );
    measure(
        "small rtt40",
        many(200, 4 * 1024, "small"),
        Content::Random,
        Some(RTT),
    );
    measure(
        "large rtt40",
        many(1, 64 * 1024 * 1024, "large"),
        Content::Random,
        Some(RTT),
    );
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
