//! Diagnostic: whether VideoToolbox's encoder here is hardware, as the host
//! reports it to viewers, and how long a picture takes to encode into a
//! 1080p stream. A picture larger than 1080p is scaled first, as the host
//! scales a 4K screen, so its time less a 1080p picture's is the scaling.
//! A viewer's "can't encode fast enough" means more than three quarters of
//! its frame budget: 12.5 ms at 60 pictures a second.
//!
//! `cargo run -p vnilink-macos --release --example encode_probe -- [h264|h265] [WIDTHxHEIGHT] [FPS]`

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use vnilink_media::{EncodeParams, VideoEncoder};
    let mut args = std::env::args().skip(1);
    let codec = match args.next().as_deref() {
        None | Some("h264") => vnilink::VideoCodec::H264,
        Some("h265") => vnilink::VideoCodec::H265,
        Some(other) => return Err(format!("unknown codec {other}").into()),
    };
    let input = args.next().unwrap_or_else(|| "1920x1080".into());
    let (input_width, input_height) = input
        .split_once('x')
        .ok_or("size is WIDTHxHEIGHT")
        .and_then(|(w, h)| {
            Ok((
                w.parse::<u16>().map_err(|_| "width")?,
                h.parse::<u16>().map_err(|_| "height")?,
            ))
        })?;
    let fps: u8 = args.next().map_or(Ok(60), |fps| fps.parse())?;
    let (width, height) = (1920u16, 1080u16);
    if (input_width, input_height) != (width, height) {
        return scale(input_width, input_height, width, height, fps);
    }
    let mut encoder = vnilink_macos::MacEncoder::open_codec(
        width,
        height,
        fps,
        12_000_000,
        codec,
        true,
        vnilink::YuvMatrix::Bt709,
    )?;
    println!("hardware: {}", encoder.hardware());
    // Pictures that change everywhere, the encoder's hardest case, made
    // before timing starts.
    let pictures: Vec<Vec<u8>> = (0..4u32)
        .map(|index| {
            (0..usize::from(input_width) * usize::from(input_height))
                .flat_map(|pixel| {
                    let value = ((pixel as u32 + index * 7) % 255) as u8;
                    [value, value.wrapping_mul(3), value ^ 0x5a, 255]
                })
                .collect()
        })
        .collect();
    let period = std::time::Duration::from_secs(1) / u32::from(fps.max(1));
    let mut times = Vec::new();
    for index in 0..240u32 {
        let frame = vnilink_macos::raw_frame_from_bgra(
            input_width,
            input_height,
            usize::from(input_width) * 4,
            u64::from(index) * 1_000_000 / u64::from(fps.max(1)),
            &pictures[index as usize % pictures.len()],
        )?;
        let started = std::time::Instant::now();
        encoder.encode(
            &frame,
            &EncodeParams {
                force_keyframe: index == 0,
                recovery: false,
                bitrate_bps: 12_000_000,
                frame_rate: fps,
            },
        )?;
        let took = started.elapsed();
        times.push(took.as_micros());
        // Paced as the host feeds it, one picture a frame period.
        std::thread::sleep(period.saturating_sub(took));
    }
    times.sort_unstable();
    println!(
        "{codec:?} 1080p at {fps} fps, copy into the encoder's surface included: median {} us, p95 {} us",
        times[times.len() / 2],
        times[times.len() * 95 / 100]
    );
    Ok(())
}

/// A captured picture of another size is scaled on the GPU before encoding,
/// as `MacEncoder` does: a pixel transfer between IOSurfaces, each output
/// pixel the average of those it covers. Timed alone.
#[cfg(target_os = "macos")]
fn scale(
    input_width: u16,
    input_height: u16,
    width: u16,
    height: u16,
    fps: u8,
) -> Result<(), Box<dyn std::error::Error>> {
    use apple_cf::cv::CVPixelBuffer;
    use apple_cf::iosurface::IOSurface;
    use videotoolbox::transfer::{DownsamplingMode, PixelTransferSession};
    const BGRA: u32 = 0x4247_5241;
    let surface = |w: u16, h: u16| {
        IOSurface::create(usize::from(w), usize::from(h), BGRA, 4).ok_or("an IOSurface")
    };
    let sources = (0..4u8)
        .map(|index| {
            let source = surface(input_width, input_height)?;
            {
                let mut guard = source
                    .lock_read_write()
                    .map_err(|_| "a writable IOSurface")?;
                // Only this lock maps the surface's bytes.
                let bytes = unsafe { guard.as_slice_mut() }.ok_or("the IOSurface's bytes")?;
                for (offset, byte) in bytes.iter_mut().enumerate() {
                    *byte = (offset as u8).wrapping_mul(31).wrapping_add(index * 17);
                }
            }
            Ok::<_, Box<dyn std::error::Error>>(
                CVPixelBuffer::create_with_io_surface(&source).map_err(|_| "a pixel buffer")?,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let target = CVPixelBuffer::create_with_io_surface(&surface(width, height)?)
        .map_err(|_| "a pixel buffer")?;
    let session = PixelTransferSession::new()?;
    session.set_downsampling_mode(DownsamplingMode::Average)?;
    session.set_real_time(true)?;
    let period = std::time::Duration::from_secs(1) / u32::from(fps.max(1));
    let mut times = Vec::new();
    for index in 0..240usize {
        let started = std::time::Instant::now();
        session.transfer(&sources[index % sources.len()], &target)?;
        let took = started.elapsed();
        times.push(took.as_micros());
        std::thread::sleep(period.saturating_sub(took));
    }
    times.sort_unstable();
    println!(
        "GPU scale {input_width}x{input_height} to {width}x{height} at {fps} fps: median {} us, p95 {} us",
        times[times.len() / 2],
        times[times.len() * 95 / 100]
    );
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("encode_probe measures VideoToolbox");
    std::process::exit(2);
}
