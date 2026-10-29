use crate::outputs::{primary, Output};

fn output(adapter: u32, output: u32, origin: (i32, i32)) -> Output {
    Output {
        adapter,
        output,
        attached: true,
        primary: origin == (0, 0),
        origin,
    }
}

#[test]
fn a_single_gpu_with_one_display_captures_it() {
    let panel = output(0, 0, (0, 0));
    assert_eq!(primary(&[panel]), Some(panel));
}

#[test]
fn a_hybrid_laptop_listing_its_discrete_gpu_first_still_captures_the_panel() {
    // The discrete GPU has no outputs, so only the integrated GPU, listed
    // second, contributes one; the encoder then prefers that adapter too.
    let panel = output(1, 0, (0, 0));
    assert_eq!(primary(&[panel]), Some(panel));
}

#[test]
fn a_hybrid_laptop_with_its_primary_display_on_the_discrete_gpu_captures_it_there() {
    let panel = output(0, 0, (-1920, 0));
    let monitor = output(1, 0, (0, 0));
    assert_eq!(primary(&[panel, monitor]), Some(monitor));
}

#[test]
fn two_discrete_gpus_capture_the_primary_display_on_either() {
    let left = output(0, 0, (0, 0));
    let right = output(1, 0, (2560, 0));
    assert_eq!(primary(&[left, right]), Some(left));
    let left = output(0, 0, (-2560, 0));
    let right = output(1, 0, (0, 0));
    assert_eq!(primary(&[left, right]), Some(right));
}

#[test]
fn windows_primary_mark_wins_over_the_origin() {
    let marked = Output {
        primary: true,
        ..output(0, 1, (1920, 0))
    };
    let at_origin = Output {
        primary: false,
        ..output(0, 0, (0, 0))
    };
    assert_eq!(primary(&[at_origin, marked]), Some(marked));
}

#[test]
fn an_output_off_the_desktop_is_never_captured() {
    let detached = Output {
        attached: false,
        ..output(0, 0, (0, 0))
    };
    let other = Output {
        primary: false,
        ..output(1, 0, (1920, 0))
    };
    assert_eq!(primary(&[detached, other]), Some(other));
    assert_eq!(primary(&[detached]), None);
}

#[test]
fn no_output_leaves_nothing_to_capture() {
    // Microsoft's basic render adapter, or a session without a display.
    assert_eq!(primary(&[]), None);
}
