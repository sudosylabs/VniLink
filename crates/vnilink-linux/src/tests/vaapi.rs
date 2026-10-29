use libva::VAEntrypoint;

use crate::vaapi::{choose_bitrate_control, choose_entrypoint, is_render_node_name};

#[test]
fn bitrate_control_selects_supported_modes_and_rejects_fixed_quantization() {
    assert_eq!(
        choose_bitrate_control(libva::VA_RC_VBR | libva::VA_RC_CBR),
        Some(libva::VA_RC_VBR)
    );
    assert_eq!(
        choose_bitrate_control(libva::VA_RC_CBR),
        Some(libva::VA_RC_CBR)
    );
    assert_eq!(choose_bitrate_control(libva::VA_RC_CQP), None);
    assert_eq!(choose_bitrate_control(libva::VA_ATTRIB_NOT_SUPPORTED), None);
}

#[test]
fn render_node_names_keep_numbered_nodes_in_order() {
    let mut names = ["card1", "renderD129", "controlD64", "renderD128", "renderD"]
        .into_iter()
        .filter(|name| is_render_node_name(name))
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["renderD128", "renderD129"]);
}

#[test]
fn encode_prefers_low_power_then_slice() {
    assert_eq!(
        choose_entrypoint(&[
            VAEntrypoint::VAEntrypointEncSlice,
            VAEntrypoint::VAEntrypointVLD,
        ]),
        Some(VAEntrypoint::VAEntrypointEncSlice)
    );
    assert_eq!(
        choose_entrypoint(&[
            VAEntrypoint::VAEntrypointEncSlice,
            VAEntrypoint::VAEntrypointEncSliceLP,
        ]),
        Some(VAEntrypoint::VAEntrypointEncSliceLP)
    );
    assert_eq!(choose_entrypoint(&[VAEntrypoint::VAEntrypointVLD]), None);
}

#[test]
fn availability_probes_do_not_fail_without_hardware() {
    // Either answer is valid; the point is the probe reports a decision
    // instead of the session failing silently later.
    let caps = crate::video_codec_caps();
    let _ = (caps.encode_baseline, caps.decode_baseline, caps.additional);
}

/// A node that offers H.265 must also produce H.265 that decodes: the
/// probe would otherwise keep it from sessions without saying why.
#[test]
fn h265_from_a_node_that_offers_it_decodes_in_software() {
    use crate::codecs::{decodes, PROBE_SIDE};
    use crate::vaapi::{Input, Session, VaCodec};

    let Some(node) = crate::vaapi::offering(VaCodec::H265) else {
        return;
    };
    let session = Session::open(
        &node,
        VaCodec::H265,
        PROBE_SIDE,
        PROBE_SIDE,
        30,
        1_000_000,
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    assert!(decodes(vnilink::VideoCodec::H265, |picture, key| {
        let (y, uv) =
            vnilink_media::bgra_to_nv12(picture, vnilink::YuvMatrix::Bt709)?.into_planes();
        session.encode(Input::Nv12 { y, uv }, key)
    }));
    assert!(crate::video_codec_caps().additional.iter().any(|codec| {
        codec.codec == vnilink::VideoCodec::H265 && codec.encode && codec.hardware
    }));
}

/// The GPU converts to the stream's colour as the CPU does
/// ([`vnilink_media::bgra_to_nv12`]), in either matrix: flat pictures,
/// black, white and the primaries included, decode to the same pixels either
/// way, the SPS saying the matrix.
#[test]
fn gpu_conversion_matches_the_cpu_conversion() {
    use vnilink::YuvMatrix;
    for matrix in [YuvMatrix::Bt709, YuvMatrix::Bt601] {
        gpu_conversion_matches_the_cpu_conversion_in(matrix);
    }
}

fn gpu_conversion_matches_the_cpu_conversion_in(matrix: vnilink::YuvMatrix) {
    use vnilink_media::{RawFrame, SoftwareDecoder, VideoDecoder};

    use crate::codecs::PROBE_SIDE;
    use crate::vaapi::{Input, Session, VaCodec};

    let Some(node) = crate::vaapi::offering(VaCodec::H265) else {
        return;
    };
    let session = Session::open(
        &node,
        VaCodec::H265,
        PROBE_SIDE,
        PROBE_SIDE,
        30,
        8_000_000,
        matrix,
    )
    .unwrap();
    if !session.converts() {
        return;
    }
    let side = usize::from(PROBE_SIDE);
    let shown = |input: Input| {
        let sample = session.encode(input, true).unwrap();
        let frame =
            vnilink::EncodedFrame::new(vnilink::VideoCodec::H265, true, 0, sample.bytes().to_vec())
                .unwrap();
        let decoded = SoftwareDecoder::open(vnilink::VideoCodec::H265)
            .unwrap()
            .decode(&frame)
            .unwrap();
        let middle = side / 2 * decoded.stride() + side / 2 * 4;
        decoded.pixels()[middle..middle + 3].to_vec()
    };
    for bgra in [
        [200, 120, 40, 255],
        [0, 0, 0, 255],
        [255, 255, 255, 255],
        [128, 128, 128, 255],
        [0, 0, 255, 255],
        [0, 255, 0, 255],
        [255, 0, 0, 255],
    ] {
        let picture = RawFrame::bgra(PROBE_SIDE, PROBE_SIDE, 0, bgra.repeat(side * side)).unwrap();
        let gpu = shown(Input::Bgra(picture.clone()));
        let (y, uv) = vnilink_media::bgra_to_nv12(&picture, matrix)
            .unwrap()
            .into_planes();
        let cpu = shown(Input::Nv12 { y, uv });
        assert!(
            gpu.iter()
                .zip(&cpu)
                .all(|(gpu, cpu)| gpu.abs_diff(*cpu) <= 2),
            "{matrix:?} {bgra:?} decoded as {gpu:?} from the GPU, {cpu:?} from the CPU"
        );
    }
}

/// A quarter second of the bitrate is the HRD buffer and the window rate
/// control holds; a change starts rate control over once, on the picture
/// that carries it, and the same rate again changes nothing.
#[test]
fn rate_control_holds_a_quarter_second_and_resets_once_per_change() {
    use crate::vaapi::RateControl;

    let mut rate = RateControl::new(12_000_000, 60);
    assert_eq!((rate.buffer_bits(), rate.window_ms()), (3_000_000, 250));
    assert!(!rate.reset);
    rate.retune(12_000_000, 60);
    assert!(!rate.reset, "the same rate is no change");
    rate.retune(6_000_000, 30);
    assert_eq!(
        (rate.bitrate_bps, rate.frame_rate, rate.reset),
        (6_000_000, 30, true)
    );
    assert_eq!((rate.buffer_bits(), rate.window_ms()), (1_500_000, 250));
}

/// Only the first picture is a keyframe however long the stream runs, and
/// a new bitrate taken between pictures makes no keyframe and changes what
/// each picture costs.
#[test]
fn keyframes_come_only_when_asked_and_a_retune_makes_none() {
    use crate::codecs::PROBE_SIDE;
    use crate::vaapi::{Input, Session, VaCodec};

    let codec = VaCodec::H264 { high: true };
    let Some(node) = crate::vaapi::offering(codec) else {
        return;
    };
    let session = Session::open(
        &node,
        codec,
        PROBE_SIDE,
        PROBE_SIDE,
        30,
        4_000_000,
        vnilink::YuvMatrix::Bt709,
    )
    .unwrap();
    let side = usize::from(PROBE_SIDE);
    let moving = |index: usize| {
        let pixels = (0..side * side)
            .flat_map(|pixel| {
                let (x, y) = (pixel % side, pixel / side);
                let value = (x + 2 * y + index * 3) as u8;
                [value, value.wrapping_mul(3), (x ^ y) as u8, 255]
            })
            .collect();
        let picture =
            vnilink_media::RawFrame::bgra(PROBE_SIDE, PROBE_SIDE, index as u64, pixels).unwrap();
        let (y, uv) = vnilink_media::bgra_to_nv12(&picture, vnilink::YuvMatrix::Bt709)
            .unwrap()
            .into_planes();
        Input::Nv12 { y, uv }
    };
    let mut sizes = Vec::new();
    for index in 0..300 {
        if index == 150 {
            session.retune(200_000, 30).unwrap();
        }
        let sample = session
            .encode(moving(index), false)
            .unwrap_or_else(|error| panic!("picture {index}: {error}"));
        assert_eq!(sample.keyframe(), index == 0, "picture {index}");
        sizes.push(sample.bytes().len());
    }
    let mean =
        |range: std::ops::Range<usize>| sizes[range.clone()].iter().sum::<usize>() / range.len();
    assert!(
        mean(260..300) * 4 < mean(100..140),
        "pictures after the retune average {} bytes against {} before",
        mean(260..300),
        mean(100..140)
    );
    assert!(session.encode(moving(300), true).unwrap().keyframe());
}
