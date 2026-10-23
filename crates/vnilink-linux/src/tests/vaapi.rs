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

    if !crate::vaapi::offers(VaCodec::H265) {
        return;
    }
    let session = Session::open(VaCodec::H265, PROBE_SIDE, PROBE_SIDE, 30, 1_000_000).unwrap();
    assert!(decodes(vnilink::VideoCodec::H265, |picture, key| {
        let (y, uv) = vnilink_media::bgra_to_nv12(picture)?.into_planes();
        session.encode(Input::Nv12 { y, uv }, key)
    }));
    assert!(crate::video_codec_caps().additional.iter().any(|codec| {
        codec.codec == vnilink::VideoCodec::H265 && codec.encode && codec.hardware
    }));
}

/// The GPU converts to the stream's colour as the CPU does
/// ([`vnilink_media::bgra_to_nv12`]): flat pictures, black, white and the
/// primaries included, decode to the same pixels either way.
#[test]
fn gpu_conversion_matches_the_cpu_conversion() {
    use vnilink_media::{RawFrame, SoftwareDecoder, VideoDecoder};

    use crate::codecs::PROBE_SIDE;
    use crate::vaapi::{Input, Session, VaCodec};

    if !crate::vaapi::offers(VaCodec::H265) {
        return;
    }
    let session = Session::open(VaCodec::H265, PROBE_SIDE, PROBE_SIDE, 30, 8_000_000).unwrap();
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
        let (y, uv) = vnilink_media::bgra_to_nv12(&picture).unwrap().into_planes();
        let cpu = shown(Input::Nv12 { y, uv });
        assert!(
            gpu.iter()
                .zip(&cpu)
                .all(|(gpu, cpu)| gpu.abs_diff(*cpu) <= 2),
            "{bgra:?} decoded as {gpu:?} from the GPU, {cpu:?} from the CPU"
        );
    }
}
