// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How fast the Remote Desktop service brings the host's screen to a viewer
//! on this computer, through the host's real picture pipeline with a
//! stand-in encoder: how long a change takes to arrive, the host's part of
//! it, how many pictures a busy screen brings, and how soon a still
//! screen's still picture comes.
//!
//! With `--peer` the host is the test agent on another computer and the
//! screen changes by its clock: the agent says when its screen started,
//! and that time is converted to this computer's clock with the offset
//! estimated just before each watch (`peer::ClockOffset`). A change's
//! arrival is then known within half the estimate's fastest round trip,
//! which the run records with its peer.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::{EncodedFrame, FrameRole};
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::desktop::{
    watch_pictures, window_decoders, DesktopSession, Script, ScriptedScreen,
};
use vnilink_testkit::peer::{ClockOffset, Remote};

const WATCH: Duration = Duration::from_secs(5);
/// Questions of the agent's time behind each offset estimate.
const PINGS: usize = 20;

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

/// The pictures one watch brought, each with when it arrived, and when
/// the screen started, all on this computer's clock.
struct Watched {
    frames: Vec<(Instant, EncodedFrame)>,
    screen_started: Instant,
}

/// Where the host runs: on this computer, or on the agent, whose times
/// each watch converts with its own estimate, the largest error kept.
enum Host {
    Here,
    Peer {
        remote: Box<Remote>,
        /// The largest bound of a watch's converted times
        /// ([`ClockOffset::bound_us`]).
        bound_us: Option<u64>,
    },
}

impl Host {
    /// Watches `script` for `watch`, the viewer's window showing stills
    /// when `stills`.
    fn watch(&mut self, script: Script, stills: bool, watch: Duration) -> Watched {
        match self {
            Self::Here => {
                let session = DesktopSession::start(ScriptedScreen::new(script), stills);
                let frames = session.watch(watch);
                let screen_started = session.screen_started;
                drop(session);
                Watched {
                    frames,
                    screen_started,
                }
            }
            Self::Peer { remote, bound_us } => {
                let offset = remote.host.clock_offset(PINGS).expect("the agent's clock");
                let watched = watch_peer(remote, &offset, params(&script), stills, watch);
                // Estimated again after the watch: the clocks drift apart meanwhile.
                let after = remote.host.clock_offset(PINGS).expect("the agent's clock");
                let bound = offset.bound_us(&after);
                *bound_us = Some(bound_us.map_or(bound, |worst| worst.max(bound)));
                watched
            }
        }
    }
}

/// What the agent's desktop scenario takes for `script`.
fn params(script: &Script) -> Value {
    match script {
        Script::Still => json!({ "script": "still" }),
        Script::Ticking { every, share, .. } => json!({
            "script": "ticking",
            "every_ms": every.as_millis(),
            "share_percent": (share * 100.0).round() as u64,
        }),
    }
}

/// Starts the agent's scripted screen and watches it from this computer's
/// engine, connected as the application connects.
fn watch_peer(
    remote: &mut Remote,
    offset: &ClockOffset,
    params: Value,
    stills: bool,
    watch: Duration,
) -> Watched {
    let facts = remote
        .host
        .start("desktop", params)
        .expect("the desktop scenario");
    let started_us = facts["started_us"]
        .as_u64()
        .unwrap_or_else(|| panic!("the desktop scenario said when its screen started: {facts}"));
    let endpoint = remote.host.endpoint().to_owned();
    remote
        .suite
        .core
        .set_window_decoders(window_decoders(stills))
        .expect("the window's decoders");
    let connection = remote
        .suite
        .connect_services_to(&endpoint)
        .expect("a services connection to the agent");
    let service = remote
        .suite
        .open_desktop(&connection)
        .expect("the agent's desktop");
    let pictures = service.subscribe_video().expect("its pictures");
    let frames = watch_pictures(&pictures, watch);
    drop(pictures);
    service.close();
    drop(service);
    remote
        .suite
        .core
        .disconnect_session(connection.id)
        .expect("the connection ends");
    if let Err(error) = remote.host.stop() {
        panic!("the agent's desktop scenario did not end: {error}");
    }
    Watched {
        frames,
        screen_started: offset.suite_instant(started_us),
    }
}

fn main() {
    let mut host = match Remote::from_env() {
        Some(remote) => Host::Peer {
            remote: Box::new(remote),
            bound_us: None,
        },
        None => Host::Here,
    };
    let mut report = Report::new("desktop");

    // From a change to the picture's arrival. The interval is no multiple
    // of the pipeline's reads (33 ms at 30 frames a second), so changes fall
    // at every point between two reads, as a person's do.
    let every = Duration::from_millis(107);
    let watched = host.watch(
        Script::Ticking {
            every,
            share: 0.3,
            regions: None,
        },
        false,
        WATCH,
    );
    let mut arrival = Vec::new();
    let mut on_host = Vec::new();
    let mut last_tick = None;
    for (at, frame) in &watched.frames {
        if frame.role != FrameRole::Changed {
            continue;
        }
        on_host.push(Duration::from_micros(u64::from(frame.timing.host_us)));
        let since = at.saturating_duration_since(watched.screen_started);
        let tick = since.as_millis() / every.as_millis();
        // The first picture after each change carries it.
        if last_tick != Some(tick) && tick > 0 {
            last_tick = Some(tick);
            arrival.push(since.saturating_sub(every * u32::try_from(tick).unwrap_or(u32::MAX)));
        }
    }
    report.record(
        "change to arrival p50",
        milliseconds(median(&mut arrival)),
        "ms",
        Better::Lower,
    );
    report.record(
        "change to arrival p95",
        milliseconds(percentile(&mut arrival, 95)),
        "ms",
        Better::Lower,
    );
    report.record(
        "host capture to send p50",
        milliseconds(median(&mut on_host)),
        "ms",
        Better::Lower,
    );

    // A screen changing at 60 Hz: the pictures it brings each second.
    let busy = host.watch(
        Script::Ticking {
            every: Duration::from_millis(16),
            share: 0.3,
            regions: None,
        },
        false,
        WATCH,
    );
    report.record(
        "busy screen pictures",
        busy.frames.len() as f64 / WATCH.as_secs_f64(),
        "pictures/s",
        Better::Higher,
    );

    // A still screen: when its still picture arrives.
    let watched = host.watch(
        Script::Still,
        true,
        vnilink_media::STILL_AFTER + Duration::from_secs(3),
    );
    let still = watched
        .frames
        .iter()
        .find(|(_, frame)| frame.role == FrameRole::Still)
        .map(|(at, _)| at.saturating_duration_since(watched.screen_started));
    match still {
        Some(at) => report.record("still picture after", milliseconds(at), "ms", Better::Lower),
        None => println!("still picture after: none arrived"),
    }

    if let Host::Peer { remote, bound_us } = &host {
        report.against(match bound_us {
            Some(bound) => remote.bench_peer_timed(*bound),
            None => remote.bench_peer(),
        });
    }
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
