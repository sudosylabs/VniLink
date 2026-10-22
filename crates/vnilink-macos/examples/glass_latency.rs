//! Read the latency strips on the main display: the stimulus and its copy in
//! the viewer, from the same captured picture, so the difference between
//! their values is glass-to-glass latency with no clock to share. Copies of
//! copies (the viewer shows the display it is on) are older still and are
//! ignored. See `vnilink_media::measure::timecode` for the strip.
//!
//! `cargo run -p vnilink-macos --release --example glass_latency -- <seconds> <out.jsonl>`
//!
//! Each line of the output is one new picture: when ScreenCaptureKit showed
//! it, every strip found, and the latency in ms when both strips were read.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::time::{Duration, Instant};

    use vnilink_macos::{list_displays, ScreenCapture, ScreenPlan};
    use vnilink_media::measure::timecode::{age, find};
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

    let mut file = std::io::BufWriter::new(std::fs::File::create(&out)?);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let (mut last, mut pictures, mut latencies) = (None, 0u32, Vec::new());
    while Instant::now() < deadline {
        let Some(frame) = capture.poll_frame(0)? else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        if frame.captured_at().is_none() || frame.captured_at() == last {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        last = frame.captured_at();
        pictures += 1;
        let codes = find(&frame, 3);
        // The newest strip is the stimulus; its copy in the viewer is the
        // next older one. Equal values are the same strip read twice.
        let newest = codes
            .iter()
            .map(|code| code.value)
            .find(|&value| codes.iter().all(|other| age(value, other.value) < 1 << 19));
        let latency = newest.and_then(|newest| {
            codes
                .iter()
                .map(|code| age(newest, code.value))
                .filter(|&ms| ms > 0 && ms < 2_000)
                .min()
        });
        latencies.extend(latency);
        let at_ms = last
            .map(|at| at.saturating_duration_since(started).as_secs_f64() * 1000.0)
            .unwrap_or_default();
        let found: Vec<String> = codes
            .iter()
            .map(|code| {
                format!(
                    r#"{{"value":{},"x":{},"y":{},"cell":{:.2},"rows":{}}}"#,
                    code.value, code.x, code.y, code.cell, code.rows
                )
            })
            .collect();
        writeln!(
            file,
            r#"{{"shownMs":{at_ms:.3},"codes":[{}],"latencyMs":{}}}"#,
            found.join(","),
            latency.map_or("null".to_owned(), |ms| ms.to_string()),
        )?;
    }
    file.flush()?;
    latencies.sort_unstable();
    let percentile = |p: f64| -> String {
        if latencies.is_empty() {
            return "-".into();
        }
        let at = ((latencies.len() - 1) as f64 * p).round() as usize;
        latencies[at].to_string()
    };
    println!(
        "pictures {pictures}, read {}, latency ms p50 {} p95 {} p99 {} max {}",
        latencies.len(),
        percentile(0.5),
        percentile(0.95),
        percentile(0.99),
        percentile(1.0),
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("glass_latency reads a macOS display");
    std::process::exit(2);
}
