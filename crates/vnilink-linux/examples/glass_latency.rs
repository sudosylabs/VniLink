//! Read the latency strips on a monitor through the ScreenCast portal
//! (`vnilink_media::measure::glass`). A picture is timed by when PipeWire
//! handed it over: the compositor's presentation time is not read, so a
//! picture counts as shown a little late. That delay is shared by both
//! strips and cancels out of the latency. The portal asks someone at this
//! computer which monitor to share; the pointer is left out.
//!
//! `cargo run -p vnilink-linux --release --example glass_latency -- <seconds> <out.jsonl>`

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let mut args = std::env::args().skip(1);
    let seconds: u64 = args.next().ok_or("seconds required")?.parse()?;
    let out = std::path::PathBuf::from(args.next().ok_or("output file required")?);
    // The portal session lives as long as this process's bus connection,
    // which the runtime owns: keep it until the reading is done.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut session = vnilink_linux::PortalSession::new();
    let remote = runtime.block_on(vnilink_linux::open_monitor(&mut session))?;
    let node = session
        .pipewire_node()
        .ok_or("the portal started no stream")?;
    let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
    let capture = vnilink_linux::PortalCapture::open(remote, node, startup)?;
    let summary =
        vnilink_media::measure::glass::read_latency(seconds, &out, || capture.poll_frame(0))?;
    println!("{summary}");
    drop(capture);
    drop(runtime);
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("glass_latency reads a Linux display");
    std::process::exit(2);
}
