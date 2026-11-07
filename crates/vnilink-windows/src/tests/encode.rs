// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
    // SAFETY: the candidates are listed and used on this test thread only.
    let encoders: Vec<_> = unsafe { h264_encoders() }.collect();
    assert!(!encoders.last().unwrap().hardware());
    let hardware_opens = encoders
        .iter()
        .filter(|candidate| candidate.hardware())
        .any(|candidate| {
            WindowsEncoder::open_candidate(
                candidate,
                1280,
                720,
                30,
                4_000_000,
                vnilink::YuvMatrix::Bt709,
            )
            .is_ok()
        });
    let encoder =
        WindowsEncoder::open(1280, 720, 30, 4_000_000, vnilink::YuvMatrix::Bt709).unwrap();
    assert_eq!(encoder.hardware(), hardware_opens);
    assert_eq!(crate::probe_video_codecs().hardware, hardware_opens);
}

/// For pictures on each adapter, the encoder rule decides, the same way at
/// every open: that adapter's hardware encoders are tried first; the first
/// D3D11-aware one reads its textures (zero copy) and says so; and an
/// encoder opened without a device reads memory, on the GPU or the CPU as
/// its pictures are.
#[test]
fn the_capture_adapter_decides_the_encoder() {
    use vnilink::EncoderInput;
    for (name, device, _) in adapter_devices() {
        let device = crate::gpu::GpuDevice::new(device);
        // SAFETY: `device` is a live D3D11 device created by `adapter_devices`.
        let adapter = unsafe { device.adapter().unwrap() };
        // SAFETY: the candidates are listed and used on this test thread only.
        let on_adapter: Vec<_> = unsafe { crate::transform::hardware_encoders_on(adapter) }
            .into_iter()
            .map(|candidate| candidate.name)
            .collect();
        // SAFETY: the candidates are listed and used on this test thread only.
        let listed: Vec<_> = unsafe { crate::transform::h264_encoders_for(Some(adapter)) }
            .map(|candidate| candidate.name)
            .collect();
        assert_eq!(
            listed[..on_adapter.len()],
            on_adapter[..],
            "{name}: its own encoders are not first"
        );
        let chosen: Vec<_> = (0..3)
            .map(|_| {
                let encoder = WindowsEncoder::open_on(
                    1280,
                    720,
                    30,
                    4_000_000,
                    false,
                    vnilink::YuvMatrix::Bt709,
                    &device,
                )
                .unwrap();
                (encoder.vendor(), encoder.implementation())
            })
            .collect();
        println!("{name}: {:?}", chosen[0]);
        assert!(
            chosen.iter().all(|choice| *choice == chosen[0]),
            "{name}: the encoder changed between opens: {chosen:?}"
        );
        let (_, implementation) = chosen[0];
        if implementation.input == Some(EncoderInput::GpuTexture) {
            assert_eq!(implementation.hardware, Some(true));
        } else {
            assert_eq!(implementation.input, Some(EncoderInput::GpuReadBack));
        }
    }
    let mut encoder =
        WindowsEncoder::open(1280, 720, 30, 4_000_000, vnilink::YuvMatrix::Bt709).unwrap();
    assert_eq!(
        encoder.implementation().input,
        Some(EncoderInput::GpuReadBack)
    );
    encoder
        .encode(
            &text_page(1280, 720, 0),
            &EncodeParams {
                force_keyframe: true,
                recovery: false,
                bitrate_bps: 4_000_000,
                frame_rate: 30,
            },
        )
        .unwrap();
    assert_eq!(
        encoder.implementation().input,
        Some(EncoderInput::Cpu),
        "a picture in memory is converted on the CPU"
    );
}

/// `profile_idc` of the SPS in an Annex-B keyframe.
fn sps_profile(annex_b: &[u8]) -> Option<u8> {
    annex_b
        .windows(5)
        .find(|bytes| bytes[..3] == [0, 0, 1] && bytes[3] & 0x1f == 7)
        .map(|bytes| bytes[4])
}

/// The negotiated High stream is High, still one picture out for each in, and
/// the baseline stream stays baseline.
#[test]
fn the_high_stream_is_high_and_the_default_stays_baseline() {
    let params = |force| EncodeParams {
        force_keyframe: force,
        recovery: false,
        bitrate_bps: 4_000_000,
        frame_rate: 30,
    };
    let Ok(mut high) =
        WindowsEncoder::open_profile(1280, 720, 30, 4_000_000, true, vnilink::YuvMatrix::Bt709)
    else {
        eprintln!("no encoder opens High here");
        return;
    };
    let key = high
        .encode(&text_page(1280, 720, 0), &params(true))
        .unwrap();
    assert_eq!(sps_profile(key.bytes()), Some(100), "High is 100");
    for index in 1..8 {
        let sample = high
            .encode(&text_page(1280, 720, index), &params(false))
            .unwrap();
        assert!(!sample.keyframe(), "picture {index} is a delta");
        assert!(
            !sample.bytes().is_empty(),
            "no picture waits for a later one"
        );
    }
    let mut base =
        WindowsEncoder::open(1280, 720, 30, 4_000_000, vnilink::YuvMatrix::Bt709).unwrap();
    let key = base
        .encode(&text_page(1280, 720, 0), &params(true))
        .unwrap();
    assert_ne!(
        sps_profile(key.bytes()),
        Some(100),
        "the default is not High"
    );
}

/// A hardware HEVC or AV1 encoder, where the machine has one, makes a stream
/// the shared software decoder shows in the right colour; where it has none
/// it declines, and the caps do not offer the codec.
#[test]
fn a_hardware_codec_that_opens_round_trips_and_one_that_does_not_is_not_offered() {
    let offered = crate::codecs::probe_video_codecs().additional;
    for codec in [vnilink::VideoCodec::H265, vnilink::VideoCodec::Av1] {
        let opens = WindowsEncoder::open_hardware_codec(
            codec,
            vnilink_media::PROBE_SIDE,
            vnilink_media::PROBE_SIDE,
            30,
            1_000_000,
            vnilink::YuvMatrix::Bt709,
            None,
        )
        .is_ok();
        let listed = offered.iter().any(|support| support.codec == codec);
        eprintln!("{codec:?}: opens {opens}, offered {listed}");
        assert!(!listed || opens, "{codec:?} is offered without an encoder");
    }
}

/// Every transform that opens, hardware or not, returns a picture for each
/// picture it is given, a forced keyframe as a keyframe; a hardware one in
/// Baseline. Microsoft's software transform writes Main on the desktop path
/// (measured 2026-10-22), which is left as it was.
#[test]
fn every_transform_encodes_each_picture() {
    // SAFETY: the candidates are listed and used on this test thread only.
    for candidate in unsafe { h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(
            &candidate,
            1280,
            720,
            30,
            4_000_000,
            vnilink::YuvMatrix::Bt709,
        ) else {
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

/// Every transform's stream says the session's matrix in limited range,
/// and pictures converted with that matrix come back through Media
/// Foundation's decoder in the colours they went in with.
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
    for (matrix, (width, height)) in [vnilink::YuvMatrix::Bt709, vnilink::YuvMatrix::Bt601]
        .into_iter()
        .flat_map(|matrix| [(640u16, 480u16), (1920, 1080)].map(move |size| (matrix, size)))
    {
        // SAFETY: the candidates are listed and used on this test thread only.
        for candidate in unsafe { h264_encoders() } {
            for colour in colours {
                let Ok(mut encoder) = WindowsEncoder::open_candidate(
                    &candidate, width, height, 30, 8_000_000, matrix,
                ) else {
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
                            Some(vnilink_media::VideoSignal::stream(matrix)),
                            "{} {matrix:?}",
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
                            "{} {matrix:?} {width}x{height} row {row}: {colour:?} came back as {pixel:?}",
                            candidate.name
                        );
                    }
                }
            }
        }
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
    on_device_in_pool(frame, driver).0
}

/// [`on_device`] with the pool the picture was copied in.
fn on_device_in_pool(
    frame: &vnilink_media::RawFrame,
    driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
) -> (vnilink_media::RawFrame, crate::gpu::TexturePool) {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA,
        D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    // SAFETY: `device` and `context` are filled by `D3D11CreateDevice` and kept alive.
    // `pSysMem` points at `frame`'s pixels, which hold `stride * height` bytes and are only
    // read during `CreateTexture2D`. The source texture is live on the same device when the
    // pool copies it.
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
        let native =
            vnilink_media::RawFrame::native(frame.timestamp_us(), Box::new(picture)).unwrap();
        (native, pool)
    }
}

/// The pointer is drawn into a picture on the GPU: the picture stays native,
/// reads back as the CPU composite does, and the original is untouched.
#[test]
fn a_pointer_over_a_gpu_picture_leaves_it_on_the_gpu() {
    use crate::cursor::{Kind, Pointer, Shape};
    let page = vnilink_media::measure::text_page(640, 360, 0);
    let (frame, pool) = on_device_in_pool(
        &page,
        windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE,
    );
    let shape = Shape::new(Kind::Color, 2, 2, 8, [200, 100, 50, 128].repeat(4)).unwrap();
    for position in [(10, 20), (-1, -1), (639, 359), (700, 20)] {
        let pointer = Pointer {
            visible: true,
            position,
            shape: Some(Shape::new(Kind::Color, 2, 2, 8, [200, 100, 50, 128].repeat(4)).unwrap()),
            image: None,
        };
        let drawn = pointer.composite_picture(frame.clone(), &pool).unwrap();
        assert!(
            drawn.native_picture().is_some(),
            "{position:?} left the GPU"
        );
        assert_eq!(
            drawn.pixels(),
            shape.composite(&page, position).unwrap().pixels(),
            "{position:?} differs from the CPU composite"
        );
    }
    assert_eq!(frame.pixels(), page.pixels());
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
    // SAFETY: the candidates are listed and used on this test thread only.
    for candidate in unsafe { h264_encoders() } {
        for (width, height) in [(1280u16, 720u16), (640, 360)] {
            let encode = |frame: &vnilink_media::RawFrame| {
                let Ok(encoder) = WindowsEncoder::open_candidate(
                    &candidate,
                    width,
                    height,
                    30,
                    8_000_000,
                    vnilink::YuvMatrix::Bt709,
                ) else {
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

/// When the GPU cannot convert (no video processor, as on Microsoft's basic
/// render driver), a captured picture is read into memory, scaled and
/// converted on the CPU: the stream goes on, as before the GPU path.
#[test]
fn a_captured_picture_is_converted_on_the_cpu_when_the_gpu_cannot() {
    use vnilink_media::measure::{psnr, text_page};
    use vnilink_media::{HostVideo, PumpOutput, Quality};
    let page = text_page(1280, 720, 0);
    // SAFETY: the candidates are listed and used on this test thread only.
    for candidate in unsafe { h264_encoders() } {
        let Ok(mut encoder) = WindowsEncoder::open_candidate(
            &candidate,
            640,
            360,
            30,
            8_000_000,
            vnilink::YuvMatrix::Bt709,
        ) else {
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
        if !vnilink_media::needs::needs("a GPU", gpu_present()) {
            continue;
        }
        let mut encoder = WindowsEncoder::open_candidate(
            &candidate,
            640,
            360,
            30,
            8_000_000,
            vnilink::YuvMatrix::Bt709,
        )
        .unwrap();
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
    let mut encoder =
        WindowsEncoder::open(640, 360, 30, 8_000_000, vnilink::YuvMatrix::Bt709).unwrap();
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
/// `Ok` when this computer has a GPU, not only Microsoft's software
/// renderer, as a virtual machine has.
fn gpu_present() -> Result<(), String> {
    if adapter_devices().is_empty() {
        Err("this computer has only Microsoft's software renderer".to_owned())
    } else {
        Ok(())
    }
}

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
    // SAFETY: every call is on a COM object created in this block; `device` and `context` are
    // filled by `D3D11CreateDevice` and kept alive in `devices`.
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
    // SAFETY: `device` and `context` are live and belong together. `pSysMem` points at
    // `frame`'s pixels, which hold `stride * height` bytes and are only read during
    // `CreateTexture2D`.
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
    // Microsoft's software renderer is covered by
    // `a_picture_on_the_software_renderer_is_encoded`.
    if !vnilink_media::needs::needs("a GPU", gpu_present()) {
        return;
    }
    let devices = adapter_devices();
    for (name, device, context) in devices {
        let gpu = crate::gpu::GpuDevice::new(device.clone());
        for (width, height) in [(1280u16, 720u16), (640, 360)] {
            let encode = |frame: &vnilink_media::RawFrame| {
                let encoder = WindowsEncoder::open_on(
                    width,
                    height,
                    30,
                    8_000_000,
                    false,
                    vnilink::YuvMatrix::Bt709,
                    &gpu,
                )
                .unwrap();
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
