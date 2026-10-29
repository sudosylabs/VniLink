//! Input on a desktop: what the host's desktop applies, what it lifts when
//! the desktop's connection ends, what it reports when its operating system
//! refuses an event or is slow to take one, and what it holds when input is
//! lost on the way.

use expect_test::expect;

use vnilink::{
    usage, DisplayInfo, Error, InputEvent, PointerButton, RemoteDesktopService, ServiceKind,
    ServicePhase, SessionInfo,
};
use vnilink_testkit::desktop::{input_failed, releases_settled, InputLog, Scripted, Step, Typed};
use vnilink_testkit::drops::{DropPoint, SeededDrops};
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

use super::keyboard::records_nothing_typed;
use super::streams::SCREEN;

/// The display a desktop's pointer moves on: the host's one monitor.
const DISPLAY: u64 = 1;

/// What the operating system says when it refuses an event.
const REFUSED: &str = "the operating system rejected the event";

/// The host publishes its one display before the viewer opens its desktop,
/// which then selects it: the selection goes before any pointer on the
/// desktop's stream, so pointer input has a display to land on.
fn publish_display(pair: &Pair) -> Result<(), Error> {
    pair.host.core.set_topology(
        1,
        &[DisplayInfo {
            id: DISPLAY,
            origin_x: 0,
            origin_y: 0,
            width: SCREEN.0,
            height: SCREEN.1,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 1,
        }],
    )
}

/// The controller's services connection to the host and a Desktop on it,
/// as the application opens them.
fn open_desktop(pair: &Pair) -> Result<(SessionInfo, RemoteDesktopService), Error> {
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    Ok((connection, desktop))
}

/// The id the host knows the controller's connection by, which its input
/// failures name.
fn hosted_connection(pair: &Pair) -> u64 {
    pair.host.hosted_id(&pair.controller)
}

/// A host with a desktop offers keyboard, pointer and its one monitor; the
/// keys, pointer moves, button and wheel a viewer sends apply in order,
/// pointer moves still queued together coalescing to the last, and ending
/// the connection lifts the held button and key, last pressed first.
#[test]
fn keyboard_and_pointer_events_are_applied_and_released() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let capabilities = host.capabilities();
    assert!(
        capabilities.keyboard && capabilities.pointer,
        "a host with a desktop offers keyboard and pointer"
    );
    assert_eq!(capabilities.monitors, 1, "the host offers its one monitor");
    publish_display(&pair)?;
    let (_, desktop) = open_desktop(&pair)?;
    let info = desktop.desktop_info()?;
    assert_eq!(
        (info.monitor_width, info.monitor_height),
        SCREEN,
        "the viewer learns the host's monitor"
    );
    desktop.select_displays(&[DISPLAY])?;
    let key = InputEvent::key_down(usage::A);
    let moves = [
        InputEvent::pointer(1, 1, 1, 1),
        InputEvent::pointer(4, 5, 1, 1),
    ];
    let tail = [
        InputEvent::pointer(9, 8, 1, 1),
        InputEvent::button_down(PointerButton::Left),
        InputEvent::wheel(0, -1),
    ];
    for event in [key].iter().chain(&moves).chain(&tail) {
        desktop.submit_input(*event)?;
    }
    // Coalescing drops moves still queued together; a running sender may
    // already have sent either earlier move.
    let valid: Vec<Vec<InputEvent>> = (0..4)
        .map(|mask| {
            let mut events = vec![key];
            events.extend(
                moves
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| mask & (1 << index) != 0)
                    .map(|(_, event)| *event),
            );
            events.extend(tail);
            events
        })
        .collect();
    let ends_with = |last: InputEvent| {
        wait_for(&format!("the host to apply {last:?}"), DEFAULT_WAIT, || {
            let events = applied.events();
            (events.last() == Some(&last)).then_some(events)
        })
    };
    let got = ends_with(InputEvent::wheel(0, -1));
    assert!(
        valid.contains(&got),
        "every event applies in order, only moves coalescing: {got:?}"
    );
    host.stop_hosted_session()?;
    let mut expected = got;
    expected.extend([
        InputEvent::button_up(PointerButton::Left),
        InputEvent::key_up(usage::A),
    ]);
    assert_eq!(
        ends_with(InputEvent::key_up(usage::A)),
        expected,
        "ending the connection lifts the held button, then the held key"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// A key and buttons pressed and released apply as sent, and ending the
/// connection lifts nothing more: none is still held.
#[test]
fn key_up_and_each_button_are_applied_and_not_released_again() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let (_, desktop) = open_desktop(&pair)?;
    let expected = [
        InputEvent::key_down(usage::A),
        InputEvent::key_up(usage::A),
        InputEvent::button_down(PointerButton::Right),
        InputEvent::button_up(PointerButton::Right),
        InputEvent::button_down(PointerButton::Middle),
        InputEvent::button_up(PointerButton::Middle),
        InputEvent::wheel(2, -3),
    ];
    for event in expected {
        desktop.submit_input(event)?;
    }
    assert_eq!(
        applied.exactly(expected.len()),
        expected,
        "each press and release applies as sent"
    );
    pair.host.core.stop_hosted_session()?;
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert_eq!(
        applied.events(),
        expected,
        "ending the connection releases nothing already released"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// A key the host's operating system refuses is reported on the host, on
/// the desktop's connection, and recorded with why; the desktop stays open
/// and the keys and pointer moves after it apply.
#[test]
fn an_apply_failure_is_reported_and_the_desktop_stays_open() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let scripted = Scripted::install(&pair.host, (640, 480), &[(1, Step::Fail(REFUSED))])?;
    let events = host.subscribe()?;
    publish_display(&pair)?;
    let (_, desktop) = open_desktop(&pair)?;
    assert_eq!(
        {
            let info = desktop.desktop_info()?;
            (info.monitor_width, info.monitor_height)
        },
        (640, 480),
        "the viewer learns the host's monitor"
    );
    desktop.submit_input(InputEvent::key_down(usage::B))?;
    assert_eq!(
        input_failed(&events),
        (hosted_connection(&pair), REFUSED),
        "the host reports the key its system refused, on the connection"
    );
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    desktop.select_displays(&[DISPLAY])?;
    desktop.submit_input(InputEvent::pointer(3, 4, 1, 1))?;
    scripted.wait(
        &[
            Typed::Event(InputEvent::key_down(usage::A)),
            Typed::Event(InputEvent::pointer(3, 4, 1, 1)),
        ],
        "the key and pointer after a refused key apply",
    );
    assert_eq!(
        pair.host
            .hosted_for(ServiceKind::Desktop, &pair.controller)
            .ok_or("the host's desktop")?
            .phase,
        ServicePhase::Active,
        "a refused key leaves the desktop open"
    );
    desktop.submit_input(InputEvent::key_up(usage::A))?;
    records_nothing_typed(&trace, &[]);
    trace.wait_for_records("input refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          warn input: input refused kind=key down reason=the operating system rejected the event service=1
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}

/// When the connection ends and the host's system refuses to lift what was
/// held, the host reports it on the connection, and the desktop still
/// closes.
#[test]
fn a_failed_release_is_reported_on_the_connection() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    // The two presses apply; both releases are refused.
    let scripted = Scripted::install(
        &pair.host,
        (800, 600),
        &[(3, Step::Fail(REFUSED)), (4, Step::Fail(REFUSED))],
    )?;
    let events = host.subscribe()?;
    let (_, desktop) = open_desktop(&pair)?;
    let held = [
        InputEvent::key_down(usage::A),
        InputEvent::button_down(PointerButton::Left),
    ];
    for event in held {
        desktop.submit_input(event)?;
    }
    scripted.wait(&held.map(Typed::Event), "the key and button apply");
    let connection = hosted_connection(&pair);
    host.stop_hosted_session()?;
    assert_eq!(
        input_failed(&events),
        (connection, "a held key or button could not be released"),
        "the host reports the failed release on the connection"
    );
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert_eq!(
        scripted.seen(),
        held.map(Typed::Event),
        "a refused release applies nothing more"
    );
    assert_eq!(
        scripted.calls(),
        4,
        "each held key and button was released once"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// A backend still applying a key when control is withdrawn finishes it,
/// and the host lifts that key at once: the viewer that pressed it can no
/// longer release it.
#[test]
fn a_key_the_backend_applies_as_control_is_withdrawn_is_released_at_once() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let scripted = Scripted::install(&pair.host, SCREEN, &[(1, Step::Park)])?;
    let (_, desktop) = open_desktop(&pair)?;
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    scripted.wait_parked();
    pair.host.core.set_desktop_sharing(vnilink::Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    })?;
    scripted.unpark();
    let released = [
        Typed::Event(InputEvent::key_down(usage::A)),
        Typed::Event(InputEvent::key_up(usage::A)),
    ];
    scripted.wait(&released, "the key applied as its lease ended is released");
    releases_settled(&pair.host);
    assert_eq!(
        scripted.seen(),
        released,
        "the key is released once, by the call that applied it or the withdrawal, not both"
    );
    records_nothing_typed(&trace, &[]);
    trace.wait_for_records("control changed", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          input: control changed epoch=2 service=1 state=Viewing
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}

/// A slow backend keeps the order the viewer sent: the keys behind a slow
/// one wait for it, and what it still holds is lifted, once, when the
/// connection ends.
#[test]
fn a_slow_backend_keeps_the_order_and_what_it_holds() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let scripted = Scripted::install(&pair.host, SCREEN, &[(1, Step::Park)])?;
    let (_, desktop) = open_desktop(&pair)?;
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    scripted.wait_parked();
    desktop.submit_input(InputEvent::key_down(usage::B))?;
    desktop.submit_input(InputEvent::key_up(usage::B))?;
    assert_eq!(
        scripted.calls(),
        1,
        "nothing reaches the backend while it is still applying a key"
    );
    scripted.unpark();
    let applied = [
        Typed::Event(InputEvent::key_down(usage::A)),
        Typed::Event(InputEvent::key_down(usage::B)),
        Typed::Event(InputEvent::key_up(usage::B)),
    ];
    scripted.wait(
        &applied,
        "the keys behind the slow one apply after it, in order",
    );
    pair.host.core.stop_hosted_session()?;
    pair.host.wait_released(Some(ServiceKind::Desktop));
    let mut released = applied.to_vec();
    released.push(Typed::Event(InputEvent::key_up(usage::A)));
    scripted.wait(&released, "ending the connection lifts the key still held");
    releases_settled(&pair.host);
    assert_eq!(
        scripted.seen(),
        released,
        "the key still held is lifted once"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// A backend that cannot apply a kind of input says so to the host's user
/// once, not for every input it refuses, and again only after an input
/// applied; each refusal is still recorded.
#[test]
fn a_fault_is_reported_once_until_an_input_applies() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let missing = Step::Unavailable("this desktop cannot press keys");
    let mut script: Vec<(usize, Step)> = (1..=8).map(|call| (call, missing)).collect();
    script.push((10, missing));
    let scripted = Scripted::install(&pair.host, SCREEN, &script)?;
    let events = pair.host.core.subscribe()?;
    let (_, desktop) = open_desktop(&pair)?;
    for _ in 0..4 {
        desktop.submit_input(InputEvent::key_down(usage::A))?;
        desktop.submit_input(InputEvent::key_up(usage::A))?;
    }
    desktop.submit_input(InputEvent::key_down(usage::B))?;
    desktop.submit_input(InputEvent::key_down(usage::C))?;
    scripted.wait(
        &[Typed::Event(InputEvent::key_down(usage::B))],
        "the key after the refused ones applies",
    );
    trace.wait_for_records("input refused", 9);
    let mut told = 0;
    wait_for("the host to tell its user twice", DEFAULT_WAIT, || {
        told += vnilink_testkit::events::drain(&events)
            .into_iter()
            .filter(|event| matches!(event, vnilink::CoreEvent::InputFailed { .. }))
            .count();
        (told >= 2).then_some(())
    });
    assert_eq!(
        told, 2,
        "eight refusals are told once, and the one after an applied key once more"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          warn input: input refused kind=key down reason=this desktop cannot press keys service=1
          input: input refused kind=key up reason=this desktop cannot press keys service=1
          input: input refused kind=key down reason=this desktop cannot press keys service=1
          input: input refused kind=key up reason=this desktop cannot press keys service=1
          input: input refused kind=key down reason=this desktop cannot press keys service=1
          input: input refused kind=key up reason=this desktop cannot press keys service=1
          input: input refused kind=key down reason=this desktop cannot press keys service=1
          input: input refused kind=key up reason=this desktop cannot press keys service=1
          warn input: input refused kind=key down reason=this desktop cannot press keys service=1
    "#]],
        || trace.tell(&["input"]).only(&["input refused"]).to_string(),
    );
    Ok(())
}

/// Input lost on the way, before it leaves the viewer, before the host
/// admits it or once admitted, never leaves the host holding a key the
/// viewer let go: releasing input lifts what the host applied and nothing
/// it did not. Each loss is recorded with where, never which key.
#[test]
fn input_lost_on_the_way_leaves_nothing_held_once_released() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let viewer_drops = SeededDrops::new()
        .nth(DropPoint::Outbox, &[2])
        .install(&pair.controller);
    let host_drops = SeededDrops::new()
        .nth(DropPoint::BeforeAdmission, &[2])
        .nth(DropPoint::AfterAdmission, &[2])
        .install(&pair.host);
    let (_, desktop) = open_desktop(&pair)?;
    // A's release is lost leaving the viewer, B before the host admits it,
    // C once admitted; D applies.
    for event in [
        InputEvent::key_down(usage::A),
        InputEvent::key_up(usage::A),
        InputEvent::key_down(usage::B),
        InputEvent::key_down(usage::C),
        InputEvent::key_down(usage::D),
    ] {
        desktop.submit_input(event)?;
    }
    assert_eq!(
        applied.wait_for(2),
        [
            InputEvent::key_down(usage::A),
            InputEvent::key_down(usage::D)
        ],
        "only what was not lost applies"
    );
    desktop.release_input()?;
    assert_eq!(
        applied.exactly(4)[2..],
        [InputEvent::key_up(usage::D), InputEvent::key_up(usage::A)],
        "releasing lifts the keys the host holds, A whose release was lost among them"
    );
    assert_eq!(
        (
            viewer_drops.tally(DropPoint::Outbox).lost,
            host_drops.tally(DropPoint::BeforeAdmission).lost,
            host_drops.tally(DropPoint::AfterAdmission).lost,
        ),
        (1, 1, 1),
        "one input was lost at each point"
    );
    records_nothing_typed(&trace, &[]);
    trace.wait_for_records("input dropped for a test", 3);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          input: input dropped for a test at=outbox kind=key up service=1
        host:
          input: input dropped for a test at=before admission kind=key down service=1
          input: input dropped for a test at=after admission kind=key down service=1
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}
