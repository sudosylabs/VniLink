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
    use crate::vaapi::{Session, VaCodec};

    if !crate::vaapi::offers(VaCodec::H265) {
        return;
    }
    let session = Session::open(VaCodec::H265, PROBE_SIDE, PROBE_SIDE, 30, 1_000_000).unwrap();
    assert!(decodes(vnilink::VideoCodec::H265, |picture, key| {
        let (y, uv) = vnilink_media::bgra_to_nv12(picture)?.into_planes();
        session.encode(y, uv, key)
    }));
    assert!(crate::video_codec_caps().additional.iter().any(|codec| {
        codec.codec == vnilink::VideoCodec::H265 && codec.encode && codec.hardware
    }));
}
