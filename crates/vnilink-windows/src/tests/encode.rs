//! Which H.264 transform the desktop encoder opens. Windows only.

use vnilink::EncodedFrame;
use vnilink_media::measure::text_page;
use vnilink_media::{EncodeParams, VideoDecoder, VideoEncoder};

use crate::decode::WindowsDecoder;
use crate::encode::WindowsEncoder;
use crate::transform::h264_encoders;

/// A hardware transform that opens is preferred to the software one, which
/// is listed last so a machine without hardware still encodes, and the
/// probe says which kind a peer will get.
#[test]
fn prefers_a_hardware_transform_that_opens() {
    let encoders = unsafe { h264_encoders() };
    assert!(!encoders.last().unwrap().hardware());
    let hardware_opens = encoders
        .iter()
        .filter(|candidate| candidate.hardware())
        .any(|candidate| {
            WindowsEncoder::open_candidate(candidate, 1280, 720, 30, 4_000_000).is_ok()
        });
    let encoder = WindowsEncoder::open(1280, 720, 30, 4_000_000).unwrap();
    assert_eq!(encoder.hardware(), hardware_opens);
    assert_eq!(crate::probe_video_codecs().hardware, hardware_opens);
}

/// `profile_idc` of the SPS in an Annex-B keyframe.
fn sps_profile(annex_b: &[u8]) -> Option<u8> {
    annex_b
        .windows(5)
        .find(|bytes| bytes[..3] == [0, 0, 1] && bytes[3] & 0x1f == 7)
        .map(|bytes| bytes[4])
}

/// Every transform that opens, hardware or not, returns a picture for each
/// picture it is given, a forced keyframe as a keyframe; a hardware one in
/// Baseline. Microsoft's software transform writes Main on the desktop path
/// (measured 2026-10-22), which is left as it was.
#[test]
fn every_transform_encodes_each_picture() {
    for candidate in unsafe { h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(&candidate, 1280, 720, 30, 4_000_000)
        else {
            continue;
        };
        let mut decoder = WindowsDecoder::new();
        for index in 0..20u64 {
            let force = index == 0 || index == 10;
            let params = EncodeParams {
                force_keyframe: force,
                recovery: index == 10,
                bitrate_bps: 4_000_000,
                frame_rate: 30,
            };
            let sample = encoder
                .encode(&text_page(1280, 720, index), &params)
                .unwrap_or_else(|err| panic!("{}: picture {index}: {err}", candidate.name));
            assert!(
                !force || sample.keyframe(),
                "{}: picture {index}",
                candidate.name
            );
            if force && candidate.hardware() {
                assert_eq!(sps_profile(sample.bytes()), Some(66), "{}", candidate.name);
            }
            let frame =
                EncodedFrame::h264(sample.keyframe(), index * 33_333, sample.bytes().to_vec())
                    .unwrap();
            // Media Foundation's decoder may hold the first picture back.
            let _ = decoder.decode(&frame);
        }
    }
}

/// Every transform's stream says BT.709 limited range, and pictures
/// converted with that matrix come back through Media Foundation's decoder
/// in the colours they went in with.
#[test]
fn every_transform_keeps_the_stream_colour() {
    // (b, g, r)
    let colours = [
        [0u8, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 255, 255],
        [255, 0, 255],
        [0, 128, 255],
        [128, 128, 128],
    ];
    // 1080 rows are coded as 1088: the top rows show whether chroma was
    // read from where the decoder put it.
    for (width, height) in [(640u16, 480u16), (1920, 1080)] {
        for candidate in unsafe { h264_encoders() } {
            for colour in colours {
                let Ok(mut encoder) =
                    WindowsEncoder::open_candidate(&candidate, width, height, 30, 8_000_000)
                else {
                    break;
                };
                let mut decoder = WindowsDecoder::new();
                let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * 4);
                for _ in 0..usize::from(width) * usize::from(height) {
                    pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
                let mut shown = None;
                // Media Foundation's decoder may hold a picture back: give it a few.
                for index in 0..6u64 {
                    let picture = vnilink_media::RawFrame::bgra(
                        width,
                        height,
                        index * 33_333,
                        pixels.clone(),
                    )
                    .unwrap();
                    let params = EncodeParams {
                        force_keyframe: index == 0,
                        recovery: false,
                        bitrate_bps: 8_000_000,
                        frame_rate: 30,
                    };
                    let sample = encoder.encode(&picture, &params).unwrap();
                    if index == 0 {
                        let parsed = vnilink_media::parse_annex_b(sample.bytes()).unwrap();
                        let sps = parsed
                            .units()
                            .iter()
                            .find(|unit| unit.nal_type == 7)
                            .unwrap();
                        assert_eq!(
                            vnilink_media::h264_video_signal(sps.bytes()).unwrap(),
                            Some(vnilink_media::STREAM_SIGNAL),
                            "{}",
                            candidate.name
                        );
                    }
                    let frame = EncodedFrame::h264(
                        sample.keyframe(),
                        index * 33_333,
                        sample.bytes().to_vec(),
                    )
                    .unwrap();
                    if let Ok(decoded) = decoder.decode(&frame) {
                        shown = Some(decoded);
                    }
                }
                let decoded = shown.unwrap_or_else(|| panic!("{}: no picture", candidate.name));
                for row in [4, height / 2] {
                    let x = usize::from(width / 2) * 4;
                    let pixel = &decoded.row(row)[x..x + 3];
                    for channel in 0..3 {
                        let drift = (i16::from(pixel[channel]) - i16::from(colour[channel])).abs();
                        assert!(
                            drift <= 6,
                            "{} {width}x{height} row {row}: {colour:?} came back as {pixel:?}",
                            candidate.name
                        );
                    }
                }
            }
        }
    }
}

/// Coloured text through each H.264 transform (4:2:0) against its source,
/// as `vnilink-macos`'s `chroma_quality` does for VideoToolbox.
/// `cargo test -p vnilink-windows --release --lib chroma_quality -- --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "measurement"]
fn chroma_quality() {
    use vnilink_media::measure::{chroma_psnr, colour_text_page, edge_psnr, psnr};
    println!(
        "| Transform | Size | Target Mb/s | Sent kB/s | Last PSNR | Last chroma | Last edge |"
    );
    println!("|---|---|---|---|---|---|---|");
    for candidate in unsafe { h264_encoders() } {
        for (width, height) in [(1280u16, 720u16), (1920, 1080)] {
            for megabits in [2u32, 8] {
                let bitrate = megabits * 1_000_000;
                let Ok(mut encoder) =
                    WindowsEncoder::open_candidate(&candidate, width, height, 30, bitrate)
                else {
                    continue;
                };
                let mut decoder = WindowsDecoder::new();
                let (mut bytes, mut last) = (0usize, None);
                for index in 0..60u64 {
                    let params = EncodeParams {
                        force_keyframe: index == 0,
                        recovery: false,
                        bitrate_bps: bitrate,
                        frame_rate: 30,
                    };
                    let Ok(sample) =
                        encoder.encode(&colour_text_page(width, height, index), &params)
                    else {
                        continue;
                    };
                    bytes += sample.bytes().len();
                    let frame = EncodedFrame::h264(
                        sample.keyframe(),
                        index * 33_333,
                        sample.bytes().to_vec(),
                    )
                    .unwrap();
                    if let Ok(picture) = decoder.decode(&frame) {
                        last = Some(picture);
                    }
                }
                let Some(last) = last else {
                    println!(
                        "| {} | {width}x{height} | {megabits} | no picture |",
                        candidate.name
                    );
                    continue;
                };
                // The decoder may hold one picture back: compare with the
                // source of the picture it returned.
                let source = (0..60u64)
                    .rev()
                    .map(|index| colour_text_page(width, height, index))
                    .max_by(|a, b| psnr(a, &last).total_cmp(&psnr(b, &last)))
                    .unwrap();
                println!(
                    "| {} | {width}x{height} | {megabits} | {:.0} | {:.1} | {:.1} | {:.1} |",
                    candidate.name,
                    bytes as f64 / 1000.0 / 2.0,
                    psnr(&source, &last),
                    chroma_psnr(&source, &last),
                    edge_psnr(&source, &last),
                );
            }
        }
    }
}

/// Which adapter drives which display, and which hardware encoders exist:
/// a GPU-resident path wants the encoder on the adapter that captures.
/// `cargo test -p vnilink-windows --lib adapters_and_outputs -- --ignored --nocapture`.
#[test]
#[ignore = "probe"]
fn adapters_and_outputs() {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().unwrap();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            let desc = adapter.GetDesc1().unwrap();
            let name = String::from_utf16_lossy(&desc.Description)
                .trim_end_matches('\0')
                .to_owned();
            println!(
                "adapter {index}: {name} vendor {:04x} luid {:08x}:{:08x} flags {}",
                desc.VendorId, desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart, desc.Flags
            );
            let mut output = 0;
            while let Ok(found) = adapter.EnumOutputs(output) {
                let out = found.GetDesc().unwrap();
                let device = String::from_utf16_lossy(&out.DeviceName)
                    .trim_end_matches('\0')
                    .to_owned();
                let r = out.DesktopCoordinates;
                println!(
                    "  output {output}: {device} attached {} {}x{} at {},{}",
                    out.AttachedToDesktop.as_bool(),
                    r.right - r.left,
                    r.bottom - r.top,
                    r.left,
                    r.top
                );
                output += 1;
            }
            index += 1;
        }
    }
    for candidate in unsafe { h264_encoders() } {
        println!("encoder: {} ({:?})", candidate.name, candidate.vendor);
    }
}

/// `frame` in a texture on the first adapter, as Desktop Duplication's
/// pictures arrive, through the capture's pool.
fn on_gpu(frame: &vnilink_media::RawFrame) -> vnilink_media::RawFrame {
    on_device(
        frame,
        windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
    )
}

/// `frame` in a texture on a device of `driver`'s kind.
fn on_device(
    frame: &vnilink_media::RawFrame,
    driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
) -> vnilink_media::RawFrame {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    unsafe {
        // As the capture does: video support where the driver has it.
        let (mut device, mut context) = (None, None);
        for flags in [
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        ] {
            let made = D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                flags,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
            if made.is_ok() {
                break;
            }
        }
        let (device, context) = (device.unwrap(), context.unwrap());
        crate::gpu::protect(&device).unwrap();
        let (width, height) = (u32::from(frame.width()), u32::from(frame.height()));
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let data = D3D11_SUBRESOURCE_DATA {
            pSysMem: frame.pixels().as_ptr().cast(),
            SysMemPitch: frame.stride() as u32,
            SysMemSlicePitch: 0,
        };
        let mut source = None;
        device
            .CreateTexture2D(&desc, Some(&data), Some(&mut source))
            .unwrap();
        let pool = crate::gpu::TexturePool::new(device, context, (width, height));
        let picture = pool.picture(&source.unwrap()).unwrap();
        vnilink_media::RawFrame::native(frame.timestamp_us(), Box::new(picture)).unwrap()
    }
}

/// `frame` averaged over `block` x `block` squares.
fn blocks(frame: &vnilink_media::RawFrame, block: u16) -> vnilink_media::RawFrame {
    let (width, height) = (frame.width() / block, frame.height() / block);
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            for channel in 0..4 {
                let mut sum = 0u32;
                for dy in 0..block {
                    for dx in 0..block {
                        sum += u32::from(
                            frame.row(y * block + dy)[usize::from(x * block + dx) * 4 + channel],
                        );
                    }
                }
                out.push((sum / u32::from(block * block)) as u8);
            }
        }
    }
    vnilink_media::RawFrame::bgra(width, height, 0, out).unwrap()
}

/// A picture on the GPU reads back as it was, and each transform encodes it
/// after the video processor converts it there, at its own size and at
/// half, with the stream's colour.
#[test]
fn every_transform_encodes_a_picture_converted_on_the_gpu() {
    use vnilink_media::measure::{psnr, text_page};
    use vnilink_media::{HostVideo, PumpOutput, Quality};
    let page = text_page(1280, 720, 0);
    assert_eq!(on_gpu(&page).pixels(), page.pixels());
    let colours = [
        [0u8, 0, 255],
        [0, 255, 0],
        [255, 0, 0],
        [0, 128, 255],
        [128, 128, 128],
    ];
    for candidate in unsafe { h264_encoders() } {
        for (width, height) in [(1280u16, 720u16), (640, 360)] {
            let encode = |frame: &vnilink_media::RawFrame| {
                let Ok(encoder) =
                    WindowsEncoder::open_candidate(&candidate, width, height, 30, 8_000_000)
                else {
                    return None;
                };
                let mut host =
                    HostVideo::new(Quality::new(8_000, 30, width, height).unwrap(), encoder);
                let mut decoder = WindowsDecoder::new();
                let mut shown = None;
                // The decoder may hold a picture back: give it a few.
                for index in 0..4u64 {
                    let frame = frame.clone().with_timestamp(index * 33_333);
                    if let PumpOutput::Encoded(encoded) = host.push(&frame).unwrap() {
                        if let Ok(decoded) = decoder.decode(&encoded) {
                            shown = Some(decoded);
                        }
                    }
                }
                Some(shown.unwrap_or_else(|| panic!("{}: no picture", candidate.name)))
            };
            let Some(decoded) = encode(&on_gpu(&page)) else {
                break;
            };
            assert_eq!((decoded.width(), decoded.height()), (width, height));
            let layout = psnr(&blocks(&page, 8 * (1280 / width)), &blocks(&decoded, 8));
            assert!(
                layout > 30.0,
                "{} {width}x{height}: {layout:.1} dB",
                candidate.name
            );
            for colour in colours {
                let mut pixels = Vec::with_capacity(1280 * 720 * 4);
                for _ in 0..1280 * 720 {
                    pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
                let solid = vnilink_media::RawFrame::bgra(1280, 720, 0, pixels).unwrap();
                let decoded = encode(&on_gpu(&solid)).unwrap();
                let pixel = &decoded.row(height / 2)
                    [usize::from(width / 2) * 4..usize::from(width / 2) * 4 + 3];
                for channel in 0..3 {
                    let drift = (i16::from(pixel[channel]) - i16::from(colour[channel])).abs();
                    assert!(
                        drift <= 6,
                        "{} {width}x{height}: {colour:?} came back as {pixel:?}",
                        candidate.name
                    );
                }
            }
        }
    }
}

/// The real desktop captured and encoded for twenty seconds a mode: on the
/// GPU, and as before (every picture read into memory, converted and scaled
/// on the CPU). Prints the time per picture and the process's CPU. Needs the
/// interactive session: Desktop Duplication fails in SSH's session 0.
/// `cargo test -p vnilink-windows --release --lib capture_to_encode_cost -- --ignored --nocapture`.
#[test]
#[ignore = "measurement"]
fn capture_to_encode_cost() {
    use std::time::{Duration, Instant};
    use vnilink_media::{HostVideo, PumpOutput, Quality};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let cpu = || unsafe {
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
        .unwrap();
        let ticks =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
    };
    let startup = vnilink::BackendStartup::new(Duration::from_secs(10));
    let capture = crate::WindowsCapture::open(&startup).unwrap();
    capture.set_embeds_cursor(false);
    let (width, height) = capture.size();
    println!("desktop {width}x{height}, encoder at 1280x720");
    for mode in ["gpu", "cpu", "gpu", "cpu"] {
        let encoder = WindowsEncoder::open(1280, 720, 30, 4_000_000).unwrap();
        let mut host = HostVideo::new(Quality::new(4_000, 30, 1280, 720).unwrap(), encoder);
        let (started, cpu_started) = (Instant::now(), cpu());
        let mut times = Vec::new();
        while started.elapsed() < Duration::from_secs(20) {
            let tick = Instant::now();
            let timestamp = started.elapsed().as_micros() as u64;
            if let Some(frame) = capture.poll_frame(timestamp).unwrap() {
                let frame = if mode == "cpu" {
                    vnilink_media::RawFrame::with_stride(
                        frame.width(),
                        frame.height(),
                        frame.stride(),
                        timestamp,
                        frame.pixels().to_vec(),
                    )
                    .unwrap()
                } else {
                    frame
                };
                if let PumpOutput::Encoded(_) = host.push(&frame).unwrap() {
                    times.push(tick.elapsed());
                }
            }
            std::thread::sleep(Duration::from_millis(33).saturating_sub(tick.elapsed()));
        }
        let used = cpu() - cpu_started;
        times.sort();
        let at = |p: f64| times[((times.len() - 1) as f64 * p) as usize].as_secs_f64() * 1000.0;
        println!(
            "| {mode} | {} | {:.1} | {:.1} | {:.2} |",
            times.len(),
            at(0.5),
            at(0.95),
            used.as_secs_f64() / started.elapsed().as_secs_f64()
        );
    }
}

/// When the GPU cannot convert (no video processor, as on Microsoft's basic
/// render driver), a captured picture is read into memory, scaled and
/// converted on the CPU: the stream goes on, as before the GPU path.
#[test]
fn a_captured_picture_is_converted_on_the_cpu_when_the_gpu_cannot() {
    use vnilink_media::measure::{psnr, text_page};
    use vnilink_media::{HostVideo, PumpOutput, Quality};
    let page = text_page(1280, 720, 0);
    for candidate in unsafe { h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(&candidate, 640, 360, 30, 8_000_000)
        else {
            continue;
        };
        assert_eq!(encoder.converts_on_gpu(), None);
        encoder.refuse_gpu_conversion();
        // One transform of a hardware encoder is live at a time: this one
        // goes before the next opens.
        {
            let mut host = HostVideo::new(Quality::new(8_000, 30, 640, 360).unwrap(), encoder);
            let mut decoder = WindowsDecoder::new();
            let mut shown = None;
            for index in 0..4u64 {
                let frame = on_gpu(&page).with_timestamp(index * 33_333);
                if let PumpOutput::Encoded(encoded) = host.push(&frame).unwrap() {
                    if let Ok(decoded) = decoder.decode(&encoded) {
                        shown = Some(decoded);
                    }
                }
            }
            let decoded = shown.unwrap_or_else(|| panic!("{}: no picture", candidate.name));
            // Nearest-neighbour, as the CPU path scales.
            let reference = vnilink_media::scale_bgra(&page, 640, 360).unwrap();
            let quality = psnr(&reference, &decoded);
            assert!(quality > 25.0, "{}: {quality:.1} dB", candidate.name);
        }
        // An encoder that has not been refused converts on the GPU.
        let mut encoder =
            WindowsEncoder::open_candidate(&candidate, 640, 360, 30, 8_000_000).unwrap();
        let _ = encoder.encode(
            &on_gpu(&page),
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
                bitrate_bps: 8_000_000,
                frame_rate: 30,
            },
        );
        assert_eq!(encoder.converts_on_gpu(), Some(true), "{}", candidate.name);
    }
}

/// Microsoft's software renderer (WARP), as in a virtual machine without a
/// GPU: its device has no video support, so the picture is converted on the
/// CPU, and encoded.
#[test]
fn a_picture_on_the_software_renderer_is_encoded() {
    let page = vnilink_media::measure::text_page(1280, 720, 0);
    let frame = on_device(
        &page,
        windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_WARP,
    );
    let mut encoder = WindowsEncoder::open(640, 360, 30, 8_000_000).unwrap();
    let sample = encoder
        .encode(
            &frame,
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
                bitrate_bps: 8_000_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    assert!(sample.keyframe());
    assert_eq!(encoder.converts_on_gpu(), Some(false));
}
