// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How long pairing and connecting take on this computer: a pairing file
//! from export to saved device, and a services connection from asking to
//! admitted, again and again between the same two computers.

use std::time::{Duration, Instant};

use vnilink::SessionPhase;
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::{wait_for, Engine, Pair};

const TIMES: usize = 30;

fn main() {
    let mut report = Report::new("pairing");
    let mut pairing = Vec::with_capacity(TIMES);
    for _ in 0..TIMES {
        let host = Engine::start("host");
        let controller = Engine::start("controller");
        let started = Instant::now();
        let file = host
            .core
            .export_pairing_file(Duration::from_secs(60), false)
            .unwrap();
        let preview = controller.core.open_pairing_file(&file.bytes).unwrap();
        controller
            .core
            .confirm_pairing(&preview, &file.check)
            .unwrap();
        pairing.push(started.elapsed());
    }
    report.record(
        "pairing file p50",
        median(&mut pairing).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );

    let pair = Pair::for_desktop();
    let mut connecting = Vec::with_capacity(TIMES);
    for _ in 0..TIMES {
        let started = Instant::now();
        let session = pair.connect_services();
        connecting.push(started.elapsed());
        pair.controller.core.disconnect_session(session.id).unwrap();
        // A new connection is refused while the last one is still closing.
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
        "connect p50",
        median(&mut connecting).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "connect p95",
        percentile(&mut connecting, 95).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    drop(pair);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
