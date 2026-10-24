//! Diagnostic: whether VideoToolbox's H.264 encoder here is hardware, as the
//! host reports it to viewers, and how long a 1080p picture takes to encode.
//! A viewer's "can't encode fast enough" means more than three quarters of
//! its frame budget: 12.5 ms at 60 pictures a second.
//!
//! `cargo run -p vnilink-macos --release --example encode_probe`

#[cfg(target_os = "macos")]
fn main() {
    use vnilink_media::{EncodeParams, VideoEncoder};
    let (width, height) = (1920u16, 1080u16);
    let mut encoder = vnilink_macos::MacEncoder::open(width, height, 60, 12_000_000, true)
        .expect("VideoToolbox H.264");
    println!("hardware: {}", encoder.hardware());
    // Pictures that change everywhere, the encoder's hardest case.
    let mut times = Vec::new();
    for index in 0..120u32 {
        let pixels: Vec<u8> = (0..usize::from(width) * usize::from(height))
            .flat_map(|pixel| {
                let value = ((pixel as u32 + index * 7) % 255) as u8;
                [value, value.wrapping_mul(3), value ^ 0x5a, 255]
            })
            .collect();
        let frame = vnilink_macos::raw_frame_from_bgra(
            width,
            height,
            usize::from(width) * 4,
            u64::from(index) * 16_666,
            &pixels,
        )
        .expect("picture");
        let started = std::time::Instant::now();
        encoder
            .encode(
                &frame,
                &EncodeParams {
                    force_keyframe: index == 0,
                    recovery: false,
                    bitrate_bps: 12_000_000,
                    frame_rate: 60,
                },
            )
            .expect("encode");
        times.push(started.elapsed().as_micros());
    }
    times.sort_unstable();
    println!(
        "1080p encode: median {} us, p95 {} us",
        times[times.len() / 2],
        times[times.len() * 95 / 100]
    );
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("encode_probe measures VideoToolbox");
    std::process::exit(2);
}
