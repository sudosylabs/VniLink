use libva::VAEntrypoint;

use crate::vaapi::{choose_h264_entrypoint, is_render_node_name};

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
