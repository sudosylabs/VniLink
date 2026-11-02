// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The Remote Desktop service alone: a host serving a scripted screen
//! through its real picture pipeline, and a viewer connected as the
//! application connects one.

#[path = "desktop/codecs.rs"]
mod codecs;
#[path = "desktop/control.rs"]
mod control;
#[path = "desktop/displays.rs"]
mod displays;
#[path = "desktop/input.rs"]
mod input;
#[path = "desktop/keyboard.rs"]
mod keyboard;
#[path = "desktop/lifecycle.rs"]
mod lifecycle;
#[path = "desktop/services.rs"]
mod services;
#[path = "desktop/snapshot.rs"]
mod snapshot;
#[path = "desktop/stills.rs"]
mod stills;
#[path = "desktop/streams.rs"]
mod streams;

use expect_test::expect;
use std::time::Duration;

use vnilink::FrameRole::{Changed, Still, Unchanged};
use vnilink::{usage, InputEvent, Region};
use vnilink_testkit::desktop::{next_picture, DesktopSession, Script, ScriptedScreen, DISPLAY};
use vnilink_testkit::{traced, wait_for, TestResult, DEFAULT_WAIT};

fn ticking(share: f32, regions: Option<Vec<Region>>) -> ScriptedScreen {
    ScriptedScreen::new(Script::Ticking {
        every: Duration::from_millis(100),
        share,
        regions,
    })
}

/// A screen that keeps changing reaches the viewer as a keyframe and then
/// pictures in the order they were taken, all of its display.
#[test]
fn a_changing_screen_arrives_in_order() -> TestResult {
    let _trace = traced();
    let session = DesktopSession::start(ticking(0.5, None), false);
    let frames = session.watch(Duration::from_millis(1_500));
    assert!(
        frames.len() >= 5,
        "a screen changing every 100 ms keeps pictures coming: only {} arrived",
        frames.len()
    );
    let (_, first) = frames.first().ok_or("no picture arrived")?;
    assert!(first.keyframe, "the first picture is a keyframe: {first:?}");
    assert_eq!(
        first.role, Changed,
        "the first picture shows the whole changed screen"
    );
    assert!(
        frames.iter().all(|(_, frame)| frame.display_id == DISPLAY),
        "every picture is of the display the host serves"
    );
    let timestamps: Vec<u64> = frames.iter().map(|(_, frame)| frame.timestamp_us).collect();
    assert!(
        timestamps.windows(2).all(|pair| pair[0] < pair[1]),
        "pictures arrive in the order they were taken: {timestamps:?}"
    );
    Ok(())
}

/// After the whole first picture, each changed picture says where the
/// screen changed, as the capture told the host.
#[test]
fn a_changed_picture_says_where_the_screen_changed() -> TestResult {
    let _trace = traced();
    let clock = Region {
        x: 40,
        y: 56,
        width: 20,
        height: 8,
    };
    let session = DesktopSession::start(ticking(0.04, Some(vec![clock])), false);
    let frames = session.watch(Duration::from_millis(1_200));
    assert_eq!(
        frames.first().map(|(_, frame)| frame.regions.clone()),
        Some(None),
        "the first picture is whole, naming no regions"
    );
    let changed: Vec<_> = frames
        .iter()
        .skip(1)
        .filter(|(_, frame)| frame.role == Changed)
        .collect();
    assert!(
        !changed.is_empty(),
        "a ticking screen brings changed pictures after the first"
    );
    for (_, frame) in changed {
        assert!(
            frame
                .regions
                .as_ref()
                .is_some_and(|regions| regions.contains(&clock)),
            "a changed picture names the region the capture changed: {frame:?}"
        );
    }
    Ok(())
}

/// An unchanged screen brings exactly one still picture to a window that
/// can show it, after the stream's pictures say it is unchanged; a window
/// that cannot show stills is sent none.
#[test]
fn an_unchanged_screen_brings_one_still() -> TestResult {
    let _trace = traced();
    let watch = vnilink_media::STILL_AFTER + Duration::from_millis(2_500);
    let roles = |stills| -> Vec<vnilink::FrameRole> {
        let session = DesktopSession::start(ScriptedScreen::new(Script::Still), stills);
        session
            .watch(watch)
            .into_iter()
            .map(|(_, frame)| frame.role)
            .collect()
    };
    let shown = roles(true);
    assert_eq!(
        shown.first(),
        Some(&Changed),
        "the first picture shows the screen: {shown:?}"
    );
    assert_eq!(
        shown.iter().filter(|role| **role == Still).count(),
        1,
        "an unchanged screen brings exactly one still: {shown:?}"
    );
    assert!(
        shown.iter().skip(1).all(|role| *role != Changed),
        "an unchanged screen brings no changed picture after the first: {shown:?}"
    );
    assert!(
        shown.contains(&Unchanged),
        "the stream says the screen is unchanged: {shown:?}"
    );
    let not_shown = roles(false);
    assert!(
        !not_shown.contains(&Still),
        "a window that cannot show stills is sent none: {not_shown:?}"
    );
    Ok(())
}

/// Keys pressed on the viewer reach the host's desktop, all of them, in
/// the order they were pressed.
#[test]
fn input_reaches_the_host_in_order() -> TestResult {
    let trace = traced();
    let session = DesktopSession::start(ScriptedScreen::new(Script::Still), false);
    next_picture(&session.frames, "the first picture");
    let mut pressed = Vec::new();
    for key in [usage::A, usage::A + 1, usage::A + 2, usage::A + 3] {
        pressed.push(InputEvent::key_down(key));
        pressed.push(InputEvent::key_up(key));
    }
    for event in &pressed {
        session.service.submit_input(*event)?;
    }
    let applied = wait_for("every key to be applied", DEFAULT_WAIT, || {
        let applied = session.applied();
        (applied.len() >= pressed.len()).then_some(applied)
    });
    assert_eq!(
        applied, pressed,
        "the host applies every key pressed, in order"
    );
    // The host records each key it applied by kind, never which key.
    trace.wait_for_records("input applied", pressed.len());
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          input: input applied kind=key down service=1
          input: input applied kind=key up service=1
          input: input applied kind=key down service=1
          input: input applied kind=key up service=1
          input: input applied kind=key down service=1
          input: input applied kind=key up service=1
          input: input applied kind=key down service=1
          input: input applied kind=key up service=1
    "#]],
        || trace.detailed_story(&["input"]),
    );
    Ok(())
}

/// Once the host stops sharing control, a viewer's key still leaves the
/// viewer, and the host discards it and records why.
#[test]
fn a_key_after_control_is_withdrawn_is_discarded_and_says_why() -> TestResult {
    let trace = traced();
    let session = DesktopSession::start(ScriptedScreen::new(Script::Still), false);
    next_picture(&session.frames, "the first picture");
    session
        .pair
        .host
        .core
        .set_desktop_sharing(vnilink::Grants {
            view: true,
            control: false,
            clipboard: false,
            transfer: false,
        })?;
    // Once the viewer knows, a key it sends reaches the host, which decides;
    // input queued before is the viewer's to drop.
    trace.wait_for_records_with(
        "control changed",
        &[("engine", "controller"), ("state", "Viewing")],
        1,
    );
    session
        .service
        .submit_input(InputEvent::key_down(usage::A))?;
    trace.wait_for_records("input refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          input: control changed epoch=2 service=1 state=Viewing
        host:
          input: input refused kind=key down reason=the peer may not control this computer service=1
    "#]],
        || trace.story(&["input"]),
    );
    assert!(
        session.applied().is_empty(),
        "a key was applied without control"
    );
    Ok(())
}
