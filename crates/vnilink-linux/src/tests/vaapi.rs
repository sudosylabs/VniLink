use libva::VAEntrypoint;

use crate::vaapi::{choose_bitrate_control, choose_h264_entrypoint, is_render_node_name};

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
fn constrained_baseline_prefers_low_power_then_slice() {
    assert_eq!(
        choose_h264_entrypoint(&[
            VAEntrypoint::VAEntrypointEncSlice,
            VAEntrypoint::VAEntrypointVLD,
        ]),
        Some(VAEntrypoint::VAEntrypointEncSlice)
    );
    assert_eq!(
        choose_h264_entrypoint(&[
            VAEntrypoint::VAEntrypointEncSlice,
            VAEntrypoint::VAEntrypointEncSliceLP,
        ]),
        Some(VAEntrypoint::VAEntrypointEncSliceLP)
    );
    assert_eq!(
        choose_h264_entrypoint(&[VAEntrypoint::VAEntrypointVLD]),
        None
    );
}

#[test]
fn availability_probes_do_not_fail_without_hardware() {
    // Either answer is valid; the point is the probe reports a decision
    // instead of the session failing silently later.
    let _encode = crate::h264_encode_available();
    let _decode = crate::h264_decode_available();
}
