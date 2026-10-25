//! How fast the Remote Desktop service brings the host's screen to a viewer
//! on this computer, through the host's real picture pipeline with a
//! stand-in encoder: how long a change takes to arrive, the host's part of
//! it, how many pictures a busy screen brings, and how soon a still
//! screen's still picture comes.

use std::time::Duration;

use vnilink::FrameRole;
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::desktop::{DesktopSession, Script, ScriptedScreen};

const WATCH: Duration = Duration::from_secs(5);

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn main() {
    let mut report = Report::new("desktop");

    // From a change to the picture's arrival. The interval is no multiple
    // of the pipeline's reads (33 ms at 30 frames a second), so changes fall
    // at every point between two reads, as a person's do.
    let every = Duration::from_millis(107);
    let session = DesktopSession::start(
        ScriptedScreen::new(Script::Ticking {
            every,
            share: 0.3,
            regions: None,
        }),
        false,
    );
    let frames = session.watch(WATCH);
    let mut arrival = Vec::new();
    let mut host = Vec::new();
    let mut last_tick = None;
    for (at, frame) in &frames {
        if frame.role != FrameRole::Changed {
            continue;
        }
        host.push(Duration::from_micros(u64::from(frame.timing.host_us)));
        let since = at.duration_since(session.screen_started);
        let tick = since.as_millis() / every.as_millis();
        // The first picture after each change carries it.
        if last_tick != Some(tick) && tick > 0 {
            last_tick = Some(tick);
            arrival.push(since - every * u32::try_from(tick).unwrap_or(u32::MAX));
        }
    }
    drop(session);
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
        milliseconds(median(&mut host)),
        "ms",
        Better::Lower,
    );

    // A screen changing at 60 Hz: the pictures it brings each second.
    let session = DesktopSession::start(
        ScriptedScreen::new(Script::Ticking {
            every: Duration::from_millis(16),
            share: 0.3,
            regions: None,
        }),
        false,
    );
    let busy = session.watch(WATCH);
    drop(session);
    report.record(
        "busy screen pictures",
        busy.len() as f64 / WATCH.as_secs_f64(),
        "pictures/s",
        Better::Higher,
    );

    // A still screen: when its still picture arrives.
    let session = DesktopSession::start(ScriptedScreen::new(Script::Still), true);
    let still = session
        .watch(vnilink_media::STILL_AFTER + Duration::from_secs(3))
        .into_iter()
        .find(|(_, frame)| frame.role == FrameRole::Still)
        .map(|(at, _)| at.duration_since(session.screen_started));
    drop(session);
    match still {
        Some(at) => report.record("still picture after", milliseconds(at), "ms", Better::Lower),
        None => println!("still picture after: none arrived"),
    }

    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
