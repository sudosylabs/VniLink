//! Glass-to-glass latency read from a display: the stimulus strip and its
//! copy in the viewer come from the same captured picture, so the
//! difference between their values is the latency, with no clock to share.
//! Copies of copies (the viewer shows the display it is on) are older still
//! and are ignored. Each platform's `glass_latency` example captures the
//! display and hands its pictures here. See [`super::timecode`] for the strip.

use std::io::Write;
use std::time::{Duration, Instant};

use super::timecode::{age, find};
use crate::{Error, RawFrame};

/// Reads every new picture `next` gives for `seconds` and writes one JSON
/// line per picture to `out`: when the display showed it, every strip
/// found, and the latency in ms when both strips were read. A picture is
/// new when its [`RawFrame::captured_at`] is; `next` returns `None` when it
/// has none. Returns the line the example prints.
pub fn read_latency(
    seconds: u64,
    out: &std::path::Path,
    mut next: impl FnMut() -> Result<Option<RawFrame>, Error>,
) -> std::io::Result<String> {
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let started = Instant::now();
    let deadline = started + Duration::from_secs(seconds);
    let (mut last, mut pictures, mut latencies) = (None, 0u32, Vec::new());
    while Instant::now() < deadline {
        let frame = next().map_err(|error| std::io::Error::other(error.to_string()))?;
        let Some(frame) =
            frame.filter(|frame| frame.captured_at().is_some() && frame.captured_at() != last)
        else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
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
    Ok(format!(
        "pictures {pictures}, read {}, latency ms p50 {} p95 {} p99 {} max {}",
        latencies.len(),
        percentile(0.5),
        percentile(0.95),
        percentile(0.99),
        percentile(1.0),
    ))
}
