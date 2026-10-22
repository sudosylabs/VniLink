//! Read the latency strips on the primary display with Desktop Duplication
//! (`vnilink_media::measure::glass`). A picture is timed by its
//! `LastPresentTime`, the QueryPerformanceCounter time the desktop was
//! presented. Needs the signed-in desktop: Desktop Duplication fails in
//! SSH's session 0.
//!
//! `cargo run -p vnilink-windows --release --example glass_latency -- <seconds> <out.jsonl>`

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().ok_or("seconds required")?.parse()?;
    let out = std::path::PathBuf::from(args.next().ok_or("output file required")?);
    let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
    let capture = vnilink_windows::WindowsCapture::open(&startup)?;
    // The reader reads the desktop as it is: the pointer is not drawn in.
    capture.set_embeds_cursor(false);
    let summary =
        vnilink_media::measure::glass::read_latency(seconds, &out, || capture.poll_frame(0))?;
    println!("{summary}");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("glass_latency reads a Windows display");
    std::process::exit(2);
}
