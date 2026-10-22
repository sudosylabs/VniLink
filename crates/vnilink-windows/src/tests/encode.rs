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
    let captured_on = capture.device();
    let capture_adapter = unsafe { captured_on.adapter().unwrap() };
    println!("desktop {width}x{height}, encoder at 1280x720");
    // Path 2: the picture copied into a texture shared with another
    // adapter's device, converted and encoded there.
    let shared = unsafe {
        SharedAcross::new(
            &captured_on,
            capture_adapter,
            (u32::from(width), u32::from(height)),
        )
    };
    if let Err(refused) = &shared {
        println!("shared: {refused}");
    }
    println!("| mode | encoder | pictures | p50 ms | p95 ms | cores |");
    // Read-back into an encoder on the other adapter, the baseline path 2
    // has to beat.
    let other = adapter_devices().into_iter().find_map(|(_, device, _)| {
        let device = crate::gpu::GpuDevice::new(device);
        let adapter = unsafe { device.adapter().unwrap() };
        ((adapter.LowPart, adapter.HighPart) != (capture_adapter.LowPart, capture_adapter.HighPart))
            .then_some(device)
    });
    let modes = [
        "zero copy",
        "read-back",
        "read-back across",
        "shared",
        "cpu",
    ];
    for mode in modes.iter().chain(modes.iter()).copied() {
        let encoder = match mode {
            "zero copy" => WindowsEncoder::open_on(1280, 720, 30, 4_000_000, &captured_on).unwrap(),
            "shared" => match &shared {
                Ok(shared) => {
                    WindowsEncoder::open_on(1280, 720, 30, 4_000_000, &shared.device()).unwrap()
                }
                Err(_) => continue,
            },
            "read-back across" => match &other {
                Some(other) => WindowsEncoder::open_on(1280, 720, 30, 4_000_000, other).unwrap(),
                None => continue,
            },
            _ => WindowsEncoder::open(1280, 720, 30, 4_000_000).unwrap(),
        };
        let vendor = encoder.vendor();
        let reads_textures = encoder.reads_textures();
        let mut host = HostVideo::new(Quality::new(4_000, 30, 1280, 720).unwrap(), encoder);
        let (started, cpu_started) = (Instant::now(), cpu());
        let mut times = Vec::new();
        while started.elapsed() < Duration::from_secs(20) {
            let tick = Instant::now();
            let timestamp = started.elapsed().as_micros() as u64;
            if let Some(frame) = capture.poll_frame(timestamp).unwrap() {
                let frame = match mode {
                    "cpu" => vnilink_media::RawFrame::with_stride(
                        frame.width(),
                        frame.height(),
                        frame.stride(),
                        timestamp,
                        frame.pixels().to_vec(),
                    )
                    .unwrap(),
                    "shared" => unsafe { shared.as_ref().unwrap().carry(&frame).unwrap() },
                    _ => frame,
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
            "| {mode} | {vendor:?}{} | {} | {:.1} | {:.1} | {:.2} |",
            if reads_textures { " (textures)" } else { "" },
            times.len(),
            at(0.5),
            at(0.95),
            used.as_secs_f64() / started.elapsed().as_secs_f64()
        );
    }
}

/// A BGRA texture shared between the capture's device and a device on
/// another adapter, through an NT handle and a keyed mutex.
struct SharedAcross {
    source: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    source_context: windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    source_mutex: windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex,
    target: windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
    target_mutex: windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex,
    pool: crate::gpu::TexturePool,
    name: String,
}

impl SharedAcross {
    unsafe fn new(
        captured_on: &crate::gpu::GpuDevice,
        capture_adapter: windows::Win32::Foundation::LUID,
        size: (u32, u32),
    ) -> Result<Self, String> {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Device, ID3D11Device1, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
            D3D11_BIND_SHADER_RESOURCE, D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX,
            D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
        };
        use windows::Win32::Graphics::Dxgi::{
            IDXGIResource, IDXGIResource1, DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE,
        };
        let (name, device, context) = adapter_devices()
            .into_iter()
            .find(|(_, device, _)| {
                let adapter = crate::gpu::GpuDevice::new(device.clone())
                    .adapter()
                    .unwrap();
                (adapter.LowPart, adapter.HighPart)
                    != (capture_adapter.LowPart, capture_adapter.HighPart)
            })
            .ok_or("no other adapter")?;
        // Each way a texture can be shared: created on either side, with an
        // NT handle or a legacy one, always with a keyed mutex.
        let share = |creator: &ID3D11Device, opener: &ID3D11Device, legacy: bool| {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: size.0,
                Height: size.1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: if legacy {
                    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0 as u32
                } else {
                    (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                        | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32
                },
            };
            let mut created = None;
            creator
                .CreateTexture2D(&desc, None, Some(&mut created))
                .map_err(|error| format!("create: {error}"))?;
            let created: ID3D11Texture2D = created.unwrap();
            let opened: ID3D11Texture2D = if legacy {
                let handle = created
                    .cast::<IDXGIResource>()
                    .and_then(|resource| resource.GetSharedHandle())
                    .map_err(|error| format!("handle: {error}"))?;
                let mut opened: Option<ID3D11Texture2D> = None;
                opener
                    .OpenSharedResource(handle, &mut opened)
                    .map_err(|error| format!("open: {error}"))?;
                opened.unwrap()
            } else {
                let handle = created
                    .cast::<IDXGIResource1>()
                    .and_then(|resource| {
                        resource.CreateSharedHandle(
                            None,
                            (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0,
                            None,
                        )
                    })
                    .map_err(|error| format!("handle: {error}"))?;
                let opened = opener
                    .cast::<ID3D11Device1>()
                    .and_then(|device| device.OpenSharedResource1(handle));
                let _ = windows::Win32::Foundation::CloseHandle(handle);
                opened.map_err(|error| format!("open: {error}"))?
            };
            Ok::<_, String>((created, opened))
        };
        let mut refusals = Vec::new();
        let mut found = None;
        for (on_capture, legacy) in [(true, false), (true, true), (false, false), (false, true)] {
            let attempt = if on_capture {
                share(captured_on.device(), &device, legacy)
            } else {
                share(&device, captured_on.device(), legacy)
                    .map(|(created, opened)| (opened, created))
            };
            let how = format!(
                "created on the {} adapter, {} handle",
                if on_capture { "capture" } else { "encoder" },
                if legacy { "legacy" } else { "NT" }
            );
            match attempt {
                Ok(pair) => {
                    println!("shared: {how}");
                    found = Some(pair);
                    break;
                }
                Err(refused) => refusals.push(format!("{how}: {refused}")),
            }
        }
        let (source, target) = found.ok_or_else(|| {
            format!(
                "{name} shares no texture with the capture adapter ({})",
                refusals.join("; ")
            )
        })?;
        Ok(Self {
            source_context: captured_on.device().GetImmediateContext().unwrap(),
            source_mutex: source.cast().unwrap(),
            source,
            target_mutex: target.cast().unwrap(),
            target,
            pool: crate::gpu::TexturePool::new(device, context, size),
            name,
        })
    }

    fn device(&self) -> crate::gpu::GpuDevice {
        self.pool.device()
    }

    /// `frame` copied to the other adapter, as a picture there.
    unsafe fn carry(
        &self,
        frame: &vnilink_media::RawFrame,
    ) -> Result<vnilink_media::RawFrame, String> {
        let picture = frame
            .native_picture()
            .and_then(|picture| {
                picture
                    .as_any()
                    .downcast_ref::<crate::gpu::WindowsPicture>()
            })
            .ok_or("not a captured texture")?;
        self.source_mutex
            .AcquireSync(0, u32::MAX)
            .map_err(|error| error.to_string())?;
        self.source_context
            .CopyResource(&self.source, picture.texture());
        self.source_mutex
            .ReleaseSync(1)
            .map_err(|error| error.to_string())?;
        self.target_mutex
            .AcquireSync(1, u32::MAX)
            .map_err(|error| error.to_string())?;
        let carried = self.pool.picture(&self.target);
        self.target_mutex
            .ReleaseSync(0)
            .map_err(|error| error.to_string())?;
        let carried = carried.map_err(|error| format!("{}: {error}", self.name))?;
        vnilink_media::RawFrame::native(frame.timestamp_us(), Box::new(carried))
            .map_err(|error| error.to_string())
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

/// A device on each hardware adapter, with its description.
fn adapter_devices() -> Vec<(
    String,
    windows::Win32::Graphics::Direct3D11::ID3D11Device,
    windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
)> {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
        D3D11_SDK_VERSION,
    };
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut devices = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().unwrap();
        let mut index = 0;
        while let Ok(adapter) = factory.EnumAdapters1(index) {
            index += 1;
            let desc = adapter.GetDesc1().unwrap();
            // Microsoft's software renderer is not a GPU.
            if desc.VendorId == 0x1414 {
                continue;
            }
            let (mut device, mut context) = (None, None);
            if D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .is_ok()
            {
                let (device, context) = (device.unwrap(), context.unwrap());
                crate::gpu::protect(&device).unwrap();
                let name = String::from_utf16_lossy(&desc.Description)
                    .trim_end_matches('\0')
                    .to_owned();
                devices.push((name, device, context));
            }
        }
    }
    devices
}

/// `frame` in a texture on `device`, through a capture pool there.
fn on_this_device(
    frame: &vnilink_media::RawFrame,
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    context: &windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
) -> vnilink_media::RawFrame {
    use windows::Win32::Graphics::Direct3D11::{
        D3D11_BIND_SHADER_RESOURCE, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
        D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    unsafe {
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
        let pool = crate::gpu::TexturePool::new(device.clone(), context.clone(), (width, height));
        let picture = pool.picture(&source.unwrap()).unwrap();
        vnilink_media::RawFrame::native(frame.timestamp_us(), Box::new(picture)).unwrap()
    }
}

/// On each adapter, an encoder opened on its device reads converted
/// pictures as textures (path 1 of the selection rule), and they decode to
/// the page in the stream's colours.
#[test]
fn an_encoder_opened_on_the_capture_device_reads_its_textures() {
    use vnilink_media::measure::{psnr, text_page};
    use vnilink_media::{HostVideo, PumpOutput, Quality};
    let page = text_page(1280, 720, 0);
    let devices = adapter_devices();
    assert!(!devices.is_empty());
    for (name, device, context) in devices {
        let gpu = crate::gpu::GpuDevice::new(device.clone());
        for (width, height) in [(1280u16, 720u16), (640, 360)] {
            let encode = |frame: &vnilink_media::RawFrame| {
                let encoder = WindowsEncoder::open_on(width, height, 30, 8_000_000, &gpu).unwrap();
                assert!(
                    encoder.reads_textures(),
                    "{name}: no transform reads textures there"
                );
                let mut host =
                    HostVideo::new(Quality::new(8_000, 30, width, height).unwrap(), encoder);
                let mut decoder = WindowsDecoder::new();
                let mut shown = None;
                for index in 0..4u64 {
                    let frame = frame.clone().with_timestamp(index * 33_333);
                    if let PumpOutput::Encoded(encoded) = host.push(&frame).unwrap() {
                        if let Ok(decoded) = decoder.decode(&encoded) {
                            shown = Some(decoded);
                        }
                    }
                }
                shown.unwrap_or_else(|| panic!("{name}: no picture"))
            };
            let decoded = encode(&on_this_device(&page, &device, &context));
            let layout = psnr(&blocks(&page, 8 * (1280 / width)), &blocks(&decoded, 8));
            assert!(layout > 30.0, "{name} {width}x{height}: {layout:.1} dB");
            for colour in [[0u8, 0, 255], [0, 255, 0], [255, 0, 0], [128, 128, 128]] {
                let mut pixels = Vec::with_capacity(1280 * 720 * 4);
                for _ in 0..1280 * 720 {
                    pixels.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
                }
                let solid = vnilink_media::RawFrame::bgra(1280, 720, 0, pixels).unwrap();
                let decoded = encode(&on_this_device(&solid, &device, &context));
                let pixel = &decoded.row(height / 2)
                    [usize::from(width / 2) * 4..usize::from(width / 2) * 4 + 3];
                for channel in 0..3 {
                    let drift = (i16::from(pixel[channel]) - i16::from(colour[channel])).abs();
                    assert!(
                        drift <= 6,
                        "{name} {width}x{height}: {colour:?} came back as {pixel:?}"
                    );
                }
            }
        }
    }
}
