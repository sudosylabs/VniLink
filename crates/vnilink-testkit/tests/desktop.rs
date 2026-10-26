//! The Remote Desktop service alone: a host serving a scripted screen
//! through its real picture pipeline, and a viewer connected as the
//! application connects one.

#[path = "desktop/control.rs"]
mod control;
#[path = "desktop/displays.rs"]
mod displays;
#[path = "desktop/lifecycle.rs"]
mod lifecycle;
#[path = "desktop/streams.rs"]
mod streams;

use expect_test::expect;
use std::time::Duration;

use vnilink::FrameRole::{Changed, Still, Unchanged};
use vnilink::{usage, InputEvent, Region};
use vnilink_testkit::desktop::{DesktopSession, Script, ScriptedScreen, DISPLAY};
use vnilink_testkit::{traced, wait_for, DEFAULT_WAIT};

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
fn a_changing_screen_arrives_in_order() {
    let _trace = traced();
    let session = DesktopSession::start(ticking(0.5, None), false);
    let frames = session.watch(Duration::from_millis(1_500));
    assert!(frames.len() >= 5, "only {} pictures arrived", frames.len());
    let (_, first) = &frames[0];
    assert!(first.keyframe, "the first picture is a keyframe: {first:?}");
    assert_eq!(first.role, Changed);
    assert!(frames.iter().all(|(_, frame)| frame.display_id == DISPLAY));
    let timestamps: Vec<u64> = frames.iter().map(|(_, frame)| frame.timestamp_us).collect();
    assert!(
        timestamps.windows(2).all(|pair| pair[0] < pair[1]),
        "pictures arrived out of order: {timestamps:?}"
    );
}

/// After the whole first picture, each changed picture says where the
/// screen changed, as the capture told the host.
#[test]
fn a_changed_picture_says_where_the_screen_changed() {
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
        Some(None)
    );
    let changed: Vec<_> = frames
        .iter()
        .skip(1)
        .filter(|(_, frame)| frame.role == Changed)
        .collect();
    assert!(!changed.is_empty(), "no changed picture after the first");
    for (_, frame) in changed {
        assert!(
            frame
                .regions
                .as_ref()
                .is_some_and(|regions| regions.contains(&clock)),
            "a changed picture lost its regions: {frame:?}"
        );
    }
}

/// An unchanged screen brings exactly one still picture to a window that
/// can show it, after the stream's pictures say it is unchanged; a window
/// that cannot show stills is sent none.
#[test]
fn an_unchanged_screen_brings_one_still() {
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
    assert_eq!(shown.first(), Some(&Changed), "{shown:?}");
    assert_eq!(
        shown.iter().filter(|role| **role == Still).count(),
        1,
        "{shown:?}"
    );
    assert!(shown[1..].iter().all(|role| *role != Changed), "{shown:?}");
    assert!(shown.contains(&Unchanged), "{shown:?}");
    let not_shown = roles(false);
    assert!(!not_shown.contains(&Still), "{not_shown:?}");
}

/// Keys pressed on the viewer reach the host's desktop, all of them, in
/// the order they were pressed.
#[test]
fn input_reaches_the_host_in_order() {
    let trace = traced();
    let session = DesktopSession::start(ScriptedScreen::new(Script::Still), false);
    wait_for("the first picture", DEFAULT_WAIT, || {
        session.frames.recv_timeout(Duration::from_millis(20))
    });
    let mut pressed = Vec::new();
    for key in [usage::A, usage::A + 1, usage::A + 2, usage::A + 3] {
        pressed.push(InputEvent::key_down(key));
        pressed.push(InputEvent::key_up(key));
    }
    for event in &pressed {
        session.service.submit_input(*event).unwrap();
    }
    let applied = wait_for("every key to be applied", DEFAULT_WAIT, || {
        let applied = session.applied();
        (applied.len() >= pressed.len()).then_some(applied)
    });
    assert_eq!(applied, pressed);
    // The host records each key it applied by kind, never which key.
    trace.wait_for_records("input applied", pressed.len());
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
    "#]]
    .assert_eq(&trace.detailed_story(&["input"]));
}

/// Once the host stops sharing control, a viewer's key still leaves the
/// viewer, and the host discards it and records why.
#[test]
fn a_key_after_control_is_withdrawn_is_discarded_and_says_why() {
    let trace = traced();
    let session = DesktopSession::start(ScriptedScreen::new(Script::Still), false);
    wait_for("the first picture", DEFAULT_WAIT, || {
        session.frames.recv_timeout(Duration::from_millis(20))
    });
    session
        .pair
        .host
        .core
        .set_desktop_sharing(vnilink::Grants {
            view: true,
            control: false,
            clipboard: false,
            transfer: false,
        })
        .unwrap();
    // Once the viewer knows, a key it sends reaches the host, which decides;
    // input queued before is the viewer's to drop.
    wait_for("the viewer to learn it only views", DEFAULT_WAIT, || {
        trace
            .named("control changed")
            .iter()
            .any(|event| event["engine"] == "controller" && event["state"] == "Viewing")
            .then_some(())
    });
    session
        .service
        .submit_input(InputEvent::key_down(usage::A))
        .expect("the viewer sends the key; the host decides");
    wait_for("the host to discard the key", DEFAULT_WAIT, || {
        (!trace.named("input discarded").is_empty()).then_some(())
    });
    expect![[r#"
        controller:
          input: control changed epoch=2 service=1 state=Viewing
        host:
          input: input discarded kind=key down service=1 why=the peer may not control this computer
    "#]]
    .assert_eq(&trace.story(&["input"]));
    assert!(
        session.applied().is_empty(),
        "a key was applied without control"
    );
}
