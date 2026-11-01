// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How soon the host applies what a viewer does: a pointer moved every 8 ms
//! (a 125 Hz mouse), and a key pressed, from the viewer's submit to the
//! host's apply, over a loopback link with a screen being watched.

use std::time::{Duration, Instant};

use vnilink::{usage, InputEvent};
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::desktop::{DesktopSession, Script, ScriptedScreen, DISPLAY, SCREEN};
use vnilink_testkit::wait_for;

/// Positions are unique per move: the pointer's x and y, each under the
/// screen's width, name its index.
const MOVES: i32 = 64 * 32;
const KEYS: usize = 60;
const MOUSE_PERIOD: Duration = Duration::from_millis(8);

/// The pointer position the `index`th move takes.
fn position(index: i32) -> InputEvent {
    InputEvent::pointer(
        index % i32::from(SCREEN.0),
        index / i32::from(SCREEN.0),
        DISPLAY,
        1,
    )
}

fn index_of(x: i32, y: i32) -> i32 {
    y * i32::from(SCREEN.0) + x
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e3
}

fn main() {
    let mut report = Report::new("input");
    let session = DesktopSession::prepared(ScriptedScreen::new(Script::Still), false, |_| {});
    session.service.select_displays(&[DISPLAY]).unwrap();

    let mut sent = Vec::new();
    let started = Instant::now();
    for index in 0..MOVES {
        let due = started + MOUSE_PERIOD * u32::try_from(index).unwrap_or(u32::MAX);
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        sent.push(Instant::now());
        session.service.submit_input(position(index)).unwrap();
    }
    let last = position(MOVES - 1);
    let applied = wait_for("the last pointer position", Duration::from_secs(10), || {
        let applied = session.applied.lock().unwrap().clone();
        applied
            .iter()
            .any(|(_, event)| *event == last)
            .then_some(applied)
    });
    let mut delays = Vec::new();
    let mut taken = 0;
    for (at, event) in &applied {
        if let InputEvent::Pointer { x, y, .. } = event {
            taken += 1;
            if let Some(sent) = usize::try_from(index_of(*x, *y))
                .ok()
                .and_then(|at| sent.get(at))
            {
                delays.push(at.saturating_duration_since(*sent));
            }
        }
    }
    report.record(
        "pointer to applied p50",
        milliseconds(median(&mut delays)),
        "ms",
        Better::Lower,
    );
    report.record(
        "pointer to applied p95",
        milliseconds(percentile(&mut delays, 95)),
        "ms",
        Better::Lower,
    );
    report.record(
        "pointer positions applied",
        f64::from(taken) / f64::from(MOVES),
        "of sent",
        Better::Lower,
    );

    let mut key_delays = Vec::new();
    for _ in 0..KEYS {
        let before = session.applied.lock().unwrap().len();
        let sent = Instant::now();
        session
            .service
            .submit_input(InputEvent::key_down(usage::A))
            .unwrap();
        let applied = wait_for("the key", Duration::from_secs(10), || {
            let applied = session.applied.lock().unwrap().clone();
            (applied.len() > before).then_some(applied)
        });
        key_delays.push(applied[before].0.saturating_duration_since(sent));
        session
            .service
            .submit_input(InputEvent::key_up(usage::A))
            .unwrap();
        wait_for("the key up", Duration::from_secs(10), || {
            (session.applied.lock().unwrap().len() > before + 1).then_some(())
        });
    }
    report.record(
        "key to applied p50",
        milliseconds(median(&mut key_delays)),
        "ms",
        Better::Lower,
    );
    report.record(
        "key to applied p95",
        milliseconds(percentile(&mut key_delays, 95)),
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
