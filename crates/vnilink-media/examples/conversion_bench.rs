//! Synthetic capture pixels only; no screen permission or desktop content.
//! Run with WIDTH HEIGHT ITERATIONS [P95_BUDGET_MS]. Compare debug/release
//! on the actual host. This measures conversion, not end-to-end latency.

use std::{hint::black_box, time::Instant};
use vnilink_media::{bgra_to_nv12, RawFrame};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let width: u16 = args.first().map_or("2560", String::as_str).parse()?;
    let height: u16 = args.get(1).map_or("1440", String::as_str).parse()?;
    let iterations: usize = args.get(2).map_or("60", String::as_str).parse()?;
    if iterations == 0 {
        return Err("iterations must be positive".into());
    }
    let budget: Option<f64> = args.get(3).map(|s| s.parse()).transpose()?;
    if budget.is_some_and(|ms| !ms.is_finite() || ms <= 0.0) {
        return Err("budget must be finite and positive".into());
    }
    let mut pixels = vec![0; usize::from(width) * usize::from(height) * 4];
    for (i, pixel) in pixels.chunks_exact_mut(4).enumerate() {
        let x = i % usize::from(width.max(1));
        let y = i / usize::from(width.max(1));
        pixel.copy_from_slice(&[x as u8, y as u8, (x ^ y) as u8, 255]);
    }
    let frame = RawFrame::bgra(width, height, 0, pixels)?;
    for _ in 0..3 {
        black_box(bgra_to_nv12(black_box(&frame))?);
    }
    let mut timings = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let started = Instant::now();
        black_box(bgra_to_nv12(black_box(&frame))?);
        timings.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    let p95 = timings[(iterations * 95).div_ceil(100) - 1];
    let median = timings[iterations / 2];
    println!(
        "BGRA -> NV12 {width}x{height}: median={median:.2}ms p95={p95:.2}ms; 60fps frame budget=16.67ms; optimized={}",
        !cfg!(debug_assertions)
    );
    if budget.is_some_and(|ms| p95 > ms) {
        return Err(format!("conversion exceeds p95 budget: {p95:.2}ms").into());
    }
    Ok(())
}
