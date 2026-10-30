// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Read the latency strips on the main display with ScreenCaptureKit
//! (`vnilink_media::measure::glass`). A picture is timed by its
//! `displayTime`.
//!
//! `cargo run -p vnilink-macos --release --example glass_latency -- <seconds> <out.jsonl>`

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    use vnilink_macos::{list_displays, ScreenCapture, ScreenPlan};
    use vnilink_media::CapturePlan;

    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().ok_or("seconds required")?.parse()?;
    let out = std::path::PathBuf::from(args.next().ok_or("output file required")?);
    let display = list_displays(1)?
        .into_iter()
        .find(|display| display.origin_x == 0 && display.origin_y == 0)
        .ok_or("no main display")?;
    let quality = vnilink::Quality::new(50_000, 60, display.width, display.height)?;
    let plan = ScreenPlan::from_capture(&CapturePlan::for_display(display.id, quality)?)?;
    let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
    let capture = ScreenCapture::start_display(&plan, u32::try_from(display.id).ok(), &startup)?;
    let summary =
        vnilink_media::measure::glass::read_latency(seconds, &out, || capture.poll_frame(0))?;
    println!("{summary}");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("glass_latency reads a macOS display");
    std::process::exit(2);
}
