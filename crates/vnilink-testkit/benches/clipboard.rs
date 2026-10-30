// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How quickly copied text reaches the other computer: a short copy's round
//! trip until the other side applied it, and the largest text's.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use vnilink::MAX_CLIPBOARD_TEXT;
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::desktop::{DesktopSession, RecordingClipboard, Script, ScriptedScreen};

const COPIES: usize = 100;

fn main() {
    let mut report = Report::new("clipboard");
    let session = DesktopSession::prepared(ScriptedScreen::new(Script::Still), false, |pair| {
        pair.host
            .core
            .set_clipboard(Box::new(RecordingClipboard {
                texts: Arc::new(Mutex::new(Vec::new())),
            }))
            .unwrap();
    });
    session.share_clipboard(true);
    let mut short = Vec::with_capacity(COPIES);
    for index in 0..COPIES {
        let started = Instant::now();
        session
            .service
            .submit_clipboard(&format!("copy {index}"))
            .unwrap();
        short.push(started.elapsed());
    }
    report.record(
        "short copy p50",
        median(&mut short).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "short copy p95",
        percentile(&mut short, 95).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    let largest = "a".repeat(MAX_CLIPBOARD_TEXT);
    let mut large = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        session.service.submit_clipboard(&largest).unwrap();
        large.push(started.elapsed());
    }
    report.record(
        "largest copy p50",
        median(&mut large).as_secs_f64() * 1e3,
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
