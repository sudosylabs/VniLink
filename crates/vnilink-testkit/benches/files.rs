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
//! does, and the `slow` cases also pass at most 5 MB/s; `compressible` and
//! `slow text` move text that a compressing transfer shrinks.

use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink::probes::PrestageHooks;
use vnilink::{Core, Grants, JobState};
use vnilink_testkit::bench::{Better, Report};
use vnilink_testkit::files::{self, drive, finished, received, write_files};
use vnilink_testkit::peer::Remote;
use vnilink_testkit::relay::LocalRelay;
use vnilink_testkit::{Pair, FILES};

/// How long one upload may take.
const UPLOAD_WAIT: Duration = Duration::from_secs(600);

/// The round trip the `rtt` cases inject.
const RTT: Duration = Duration::from_millis(40);

/// Where a case's receiver stages what it receives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Staging {
    /// Where the application keeps it, on the destination's volume here.
    Here,
    /// On another volume than the destination's, so publishing copies.
    Elsewhere,
}

/// What a case's files hold.
#[derive(Clone, Copy)]
enum Content {
    /// Bytes that do not compress, as most files.
    Random,
    /// Repeating text.
    Text,
}

/// `len` bytes of words chosen at random from a small vocabulary, which
/// compress about as well as logs and source do, not as repeated lines do.
fn prose(len: usize, seed: u64) -> Vec<u8> {
    const WORDS: &str = "the of and to in is that for it as with was on be by this have from \
        or one had not but what all were when we there can an your which their said if do \
        will each about how up out them then she many some so these would other into has \
        more her two like him see time could no make than first been its who now people my \
        made over did down only way find use may water long little very after words called \
        just where most know get through back much before go good new write our me man too \
        any day same right look think also around another came come work three word must \
        because does part even place well such here take why help put different away again \
        off went old number great tell men say small every found still between name should";
    let words: Vec<&str> = WORDS.split_whitespace().collect();
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut text = Vec::with_capacity(len + 16);
    while text.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        text.extend_from_slice(words[(state >> 20) as usize % words.len()].as_bytes());
        text.push(if state.is_multiple_of(11) {
            b'\n'
        } else {
            b' '
        });
    }
    text.truncate(len);
    text
}

/// Writes `files` under `folder` with `content`.
fn write(folder: &std::path::Path, files: &[(&str, usize)], content: Content) {
    match content {
        Content::Random => write_files(folder, files),
        Content::Text => {
            for (index, (name, size)) in files.iter().enumerate() {
                let path = folder.join(name);
                std::fs::create_dir_all(path.parent().expect("a folder")).expect("a folder");
                std::fs::write(path, prose(*size, index as u64 + 1)).expect("a file");
            }
        }
    }
}

/// How a case differs from the plain one: moving bytes on this computer's
/// disk, between engines in one process.
#[derive(Clone, Copy)]
struct Setup {
    content: Content,
    /// The round trip across a relay, when the case has one.
    rtt: Option<Duration>,
    /// Bytes a second the relay passes each way, when the link is slow.
    rate: Option<u64>,
    staging: Staging,
}

impl Setup {
    const PLAIN: Self = Self {
        content: Content::Random,
        rtt: None,
        rate: None,
        staging: Staging::Here,
    };
}

/// Uploads `files` between two engines on this computer as `setup` says and
/// returns how long the receiver took to finish.
fn upload(files: &[(&str, usize)], setup: Setup) -> Duration {
    let Setup {
        content,
        rtt,
        rate,
        staging,
    } = setup;
    let relay = rtt.map(|_| LocalRelay::start());
    let delayed = rtt
        .zip(relay.as_ref())
        .map(|(rtt, relay)| relay.limited(rtt / 4, rate));
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
    if staging == Staging::Elsewhere {
        // The job exists before its first file is whole, so the hook is in
        // place when that one publishes.
        Core::set_prestage_hooks(hooks(pair.host.core.prestage_key(), Some(receiver), true));
    }
    let done = finished(&pair.host.core, receiver, UPLOAD_WAIT);
    let took = started.elapsed();
    assert_eq!(done.state, JobState::Complete, "{done:?}");
    if staging == Staging::Elsewhere {
        Core::set_prestage_hooks(hooks(0, None, false));
    }
    took
}

/// The publication hooks that force a job's files across volumes, or none.
fn hooks(engine: usize, job: Option<u64>, cross_volume: bool) -> PrestageHooks {
    PrestageHooks {
        engine,
        job_id: job,
        force_cross_volume: cross_volume,
        probe: None,
        mkdir_probe: None,
        sync_probe: None,
    }
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
    let mut measure = |name: &str, files: Vec<(String, usize)>, setup: Setup| {
        if only
            .as_ref()
            .is_some_and(|only| !name.contains(only.as_str()))
        {
            return;
        }
        // The agent's receiver runs only the plain case.
        if remote.is_some()
            && (setup.rtt.is_some()
                || matches!(setup.content, Content::Text)
                || setup.staging == Staging::Elsewhere)
        {
            return;
        }
        let bytes: usize = files.iter().map(|(_, size)| size).sum();
        let named: Vec<(&str, usize)> = files
            .iter()
            .map(|(name, size)| (name.as_str(), *size))
            .collect();
        let took = match remote.as_mut() {
            Some(remote) => upload_to(remote, &named),
            None => upload(&named, setup),
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
    let mib = 1024 * 1024;
    measure("small", many(1_000, 4 * 1024, "small"), Setup::PLAIN);
    measure("large", many(1, 256 * mib, "large"), Setup::PLAIN);
    let mut mixed = many(500, 4 * 1024, "small");
    mixed.extend(many(40, mib, "medium"));
    mixed.extend(many(2, 32 * mib, "large"));
    measure("mixed", mixed, Setup::PLAIN);
    measure("many", many(10_000, 1024, "many"), Setup::PLAIN);
    let text = Setup {
        content: Content::Text,
        ..Setup::PLAIN
    };
    measure("compressible", many(1, 128 * mib, "text"), text);
    let far = Setup {
        staging: Staging::Elsewhere,
        ..Setup::PLAIN
    };
    measure("cross-volume", many(1, 128 * mib, "far"), far);
    let distant = Setup {
        rtt: Some(RTT),
        ..Setup::PLAIN
    };
    measure("small rtt40", many(200, 4 * 1024, "small"), distant);
    measure("large rtt40", many(1, 64 * mib, "large"), distant);
    // A slow link, 5 MB/s (40 Mbit/s) each way, where the bytes sent decide.
    let slow = Setup {
        rate: Some(5_000_000),
        ..distant
    };
    measure(
        "slow text",
        many(1, 32 * mib, "text"),
        Setup {
            content: Content::Text,
            ..slow
        },
    );
    measure("slow random", many(1, 16 * mib, "large"), slow);
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
