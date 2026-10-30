// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What a viewer types into a desktop: the keyboard modes, the Control and
//! Command swap, finished text, and what the host does when its desktop
//! refuses a character or cannot type at all. Typed text and key identities
//! are the person's own: no engine records them.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    usage, Error, EventSubscription, InputEvent, KeyboardMode, LocalKey, PointerSettings,
    RemoteDesktopService, ServiceKind, ServicePhase, MAX_KEY_TEXT,
};
use vnilink_testkit::desktop::{input_failed, Scripted, Step, Typed, Typing};
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult, Traced, DEFAULT_WAIT};

use super::streams::SCREEN;

/// States that no engine recorded any of `typed`, and that no record names
/// a key or a character: what a person types is theirs.
pub(super) fn records_nothing_typed(trace: &Traced, typed: &[&str]) {
    trace.assert_unrecorded("the text typed", typed);
    for event in trace.events() {
        let event: serde_json::Value = serde_json::from_str(&event).expect("a JSON record");
        for field in ["usage", "key", "text", "character", "chord"] {
            assert!(
                event.get(field).is_none(),
                "an engine recorded which key or character was typed: {event}"
            );
        }
    }
}

/// States that `events`, the application's events from one engine, carry
/// none of `typed` for the 200 ms they are read.
pub(super) fn hides(events: &EventSubscription, typed: &str) {
    holds(
        &format!("the application's events hide {typed:?}"),
        Duration::from_millis(200),
        || {
            std::iter::from_fn(|| events.try_recv())
                .all(|event| !format!("{event:?}").contains(typed))
        },
    );
}

/// A local key as a viewer's keyboard reports it.
fn local(usage: u16, down: bool, text: &str) -> LocalKey<'_> {
    LocalKey::new(usage, down, text)
}

/// A Desktop on the controller's services connection to the host, as a
/// window opens one.
fn open_desktop(pair: &Pair) -> Result<RemoteDesktopService, Error> {
    pair.controller.open_desktop(&pair.connect_services())
}

/// Whether the host still serves the controller's Desktop.
fn desktop_active(pair: &Pair) -> bool {
    pair.host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .is_some_and(|service| service.phase == ServicePhase::Active)
}

/// The Control and Command swap waits for held keys, as the keyboard mode
/// does: refused while a key is down, taken once it is up.
#[test]
fn the_swap_waits_for_held_keys_like_the_mode_switch() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    Typing::install(&pair.host, SCREEN, false, false)?;
    let desktop = open_desktop(&pair)?;
    desktop.submit_local_key(local(usage::A, true, "a"))?;
    assert!(
        matches!(
            desktop.set_swap_control_and_command(true),
            Err(Error::InvalidSetting(_))
        ),
        "the swap does not change while a key is held"
    );
    desktop.submit_local_key(local(usage::A, false, "a"))?;
    desktop.set_swap_control_and_command(true)?;
    assert!(
        desktop.swap_control_and_command()?,
        "the swap changes once no key is held"
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// With Control and Command swapped, a Command pressed on the viewer reaches
/// the host as Control, and Control is what the host lifts when the
/// connection ends.
#[test]
fn a_swapped_command_is_the_key_the_host_releases() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typing::install(&pair.host, SCREEN, false, false)?;
    controller.set_swap_control_and_command(true)?;
    let desktop = open_desktop(&pair)?;
    desktop.submit_input(InputEvent::key_down(usage::LEFT_META))?;
    desktop.submit_input(InputEvent::key_down(usage::LEFT_SHIFT))?;
    let pressed = [
        Typed::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
        Typed::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
    ];
    typed.wait(&pressed, "a swapped Command reaches the host as Control");
    host.stop_hosted_session()?;
    let mut released = pressed.to_vec();
    released.extend([
        Typed::Event(InputEvent::key_up(usage::LEFT_SHIFT)),
        Typed::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
    ]);
    typed.wait(
        &released,
        "the host lifts the Control it pressed, last pressed first",
    );
    records_nothing_typed(&trace, &[]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// Finished text is checked before it leaves the viewer, and typed on the
/// host between the keys sent around it; neither engine records it, nor
/// tells the application it.
#[test]
fn finished_text_is_typed_in_order_and_is_not_logged() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typing::install(&pair.host, SCREEN, false, false)?;
    let desktop = open_desktop(&pair)?;
    let events = controller.subscribe()?;
    let host_events = host.subscribe()?;
    assert!(
        matches!(
            desktop.submit_text(""),
            Err(Error::InvalidSetting("text entry is empty"))
        ),
        "empty text is refused"
    );
    assert!(
        matches!(
            desktop.submit_text(&"a".repeat(MAX_KEY_TEXT + 1)),
            Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
        ),
        "text longer than the bound is refused"
    );
    assert!(
        matches!(
            desktop.submit_text("a\0b"),
            Err(Error::InvalidSetting("text entry contains a null byte"))
        ),
        "text with a null byte is refused"
    );
    desktop.submit_input(InputEvent::key_down(usage::LEFT_META))?;
    desktop.submit_text("café")?;
    desktop.submit_input(InputEvent::key_down(usage::ENTER))?;
    typed.wait(
        &[
            Typed::Event(InputEvent::key_down(usage::LEFT_META)),
            Typed::Text("café".to_owned()),
            Typed::Event(InputEvent::key_down(usage::ENTER)),
        ],
        "finished text is typed between the keys sent around it",
    );
    hides(&events, "café");
    hides(&host_events, "café");
    assert!(desktop_active(&pair), "typing text leaves the desktop open");
    records_nothing_typed(&trace, &["café", "caf"]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// Text the host's operating system refuses is reported on the host and
/// recorded with why, never what; the keys sent after it still apply.
#[test]
fn a_refused_text_leaves_the_keys_working() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let typed = Typing::install(&pair.host, SCREEN, true, false)?;
    let desktop = open_desktop(&pair)?;
    let events = host.subscribe()?;
    desktop.submit_text("nope")?;
    assert_eq!(
        input_failed(&events).1,
        "the operating system rejected the event",
        "the host reports the text its system refused"
    );
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    typed.wait(
        &[Typed::Event(InputEvent::key_down(usage::A))],
        "a key after a refused text applies",
    );
    assert!(
        desktop_active(&pair),
        "a refused text leaves the desktop open"
    );
    records_nothing_typed(&trace, &["nope"]);
    trace.wait_for_records("input refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          warn input: input refused kind=typed text reason=the operating system rejected the event service=1
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}

/// A host whose desktop cannot type text at all refuses it, says why, and
/// keeps the desktop: the keys sent after it apply.
#[test]
fn text_the_host_cannot_type_is_refused_and_the_desktop_stays() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let scripted = Scripted::install(
        &pair.host,
        SCREEN,
        &[(1, Step::Unavailable("typed text is not available"))],
    )?;
    let desktop = open_desktop(&pair)?;
    let events = host.subscribe()?;
    desktop.submit_text("unseen")?;
    assert_eq!(
        input_failed(&events).1,
        "typed text is not available",
        "the host reports it cannot type"
    );
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    scripted.wait(
        &[Typed::Event(InputEvent::key_down(usage::A))],
        "a key after text the host could not type applies",
    );
    assert!(
        desktop_active(&pair) && !desktop.is_cancelled(),
        "text the host cannot type leaves the desktop open"
    );
    records_nothing_typed(&trace, &["unseen"]);
    trace.wait_for_records("input refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          warn input: input refused kind=typed text reason=typed text is not available service=1
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}

/// Text typed before the link dropped is not typed again on the connection
/// dialled again, and the desktop of the lost generation takes no more.
#[test]
fn a_dropped_connection_does_not_replay_finished_text() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let typed = Typing::install(&pair.host, SCREEN, false, false)?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    desktop.submit_text("first words")?;
    typed.wait(
        &[Typed::Text("first words".to_owned())],
        "the first text is typed",
    );
    controller.drop_controller_link(connection.id)?;
    let again = pair.controller.redialed(&connection);
    wait_for("the old desktop to end", DEFAULT_WAIT, || {
        desktop.is_cancelled().then_some(())
    });
    assert!(
        desktop.submit_text("replayed words").is_err(),
        "text sent on the lost generation's desktop is refused"
    );
    pair.host.wait_released(Some(ServiceKind::Desktop));
    let fresh = pair.controller.open_desktop(&again)?;
    fresh.submit_text("second words")?;
    typed.wait(
        &[
            Typed::Text("first words".to_owned()),
            Typed::Text("second words".to_owned()),
        ],
        "the desktop on the redialled connection types only what was sent on it",
    );
    records_nothing_typed(&trace, &["first words", "second words", "replayed words"]);
    // How the host's side of the lost link ends is the link's story; this
    // one is that nothing typed was refused or lost.
    vnilink_testkit::assert_story(expect![[r#""#]], || {
        trace
            .tell(&["input"])
            .only(&["input refused", "input dropped for a test"])
            .to_string()
    });
    Ok(())
}

/// In the default Character mode, `@` reaches the host as the character,
/// whichever keys made it on the viewer, and no engine records it.
#[test]
fn the_default_mode_inserts_at_from_either_keyboard() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typing::install(&pair.host, SCREEN, false, false)?;
    assert_eq!(
        controller.keyboard_mode()?,
        KeyboardMode::Character,
        "Character is the default mode"
    );
    let desktop = open_desktop(&pair)?;
    assert_eq!(
        desktop.keyboard_mode()?,
        KeyboardMode::Character,
        "a desktop opens in the default mode"
    );
    let events = controller.subscribe()?;
    let host_events = host.subscribe()?;
    for key in [
        local(usage::GRAVE, true, "@"),
        local(usage::GRAVE, false, ""),
        local(usage::RIGHT_ALT, true, ""),
        local(usage::DIGIT_0, true, "@"),
        local(usage::DIGIT_0, false, ""),
        local(usage::RIGHT_ALT, false, ""),
    ] {
        desktop.submit_local_key(key)?;
    }
    typed.wait(
        &[Typed::Text("@".to_owned()), Typed::Text("@".to_owned())],
        "each keyboard's @ is typed as the character",
    );
    hides(&events, "@");
    hides(&host_events, "@");
    records_nothing_typed(&trace, &["@"]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// Map and Translate are chosen, not the default; a held key blocks the
/// change, Map sends the key and Translate the host layout's keys.
#[test]
fn map_and_translate_are_optional_and_a_held_key_blocks_the_change() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let typed = Typing::install(&pair.host, SCREEN, false, false)?;
    let desktop = open_desktop(&pair)?;
    desktop.submit_local_key(local(usage::LEFT_SHIFT, true, ""))?;
    assert!(
        matches!(
            desktop.set_keyboard_mode(KeyboardMode::Map),
            Err(Error::InvalidSetting(
                "release held input before changing keyboard mode"
            ))
        ),
        "the mode does not change while a key is held"
    );
    desktop.submit_local_key(local(usage::LEFT_SHIFT, false, ""))?;
    desktop.set_keyboard_mode(KeyboardMode::Map)?;
    desktop.submit_local_key(local(usage::GRAVE, true, "@"))?;
    typed.wait(
        &[Typed::Event(InputEvent::key_down(usage::GRAVE))],
        "Map sends the key, not the character",
    );
    desktop.submit_local_key(local(usage::GRAVE, false, ""))?;
    desktop.set_keyboard_mode(KeyboardMode::Translate)?;
    desktop.submit_local_key(local(usage::GRAVE, true, "@"))?;
    typed.wait(
        &[
            Typed::Event(InputEvent::key_down(usage::GRAVE)),
            Typed::Event(InputEvent::key_up(usage::GRAVE)),
            Typed::Chord("@".to_owned()),
        ],
        "Translate types the character as the host layout's keys",
    );
    let events = host.subscribe()?;
    hides(&events, "@");
    records_nothing_typed(&trace, &["@"]);
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["input"]));
    Ok(())
}

/// Translated keys the host cannot type are reported on the host and
/// recorded with why, and the keys sent after them still apply.
#[test]
fn a_refused_chord_leaves_the_keys_working() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let typed = Typing::install(&pair.host, SCREEN, false, true)?;
    let desktop = open_desktop(&pair)?;
    desktop.set_keyboard_mode(KeyboardMode::Translate)?;
    let events = host.subscribe()?;
    desktop.submit_local_key(local(usage::GRAVE, true, "@"))?;
    assert_eq!(
        input_failed(&events).1,
        "translated keys are not available",
        "the host reports the translated keys it could not type"
    );
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    typed.wait(
        &[Typed::Event(InputEvent::key_down(usage::A))],
        "a key after a refused chord applies",
    );
    assert!(
        desktop_active(&pair),
        "a refused chord leaves the desktop open"
    );
    records_nothing_typed(&trace, &["@"]);
    trace.wait_for_records("input refused", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          warn input: input refused kind=typed text reason=translated keys are not available service=1
    "#]],
        || trace.story(&["input"]),
    );
    Ok(())
}

/// The controller's input defaults reach its open desktops and the ones it
/// opens later; a held key refuses the whole change, and an invalid
/// pointer setting changes nothing.
#[test]
fn input_defaults_cover_open_and_future_desktops_and_refuse_held_input_atomically() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [
        Engine::start("host"),
        Engine::start("second-host"),
        Engine::start("third-host"),
    ];
    for host in &hosts {
        Typing::install(host, SCREEN, false, false)?;
        host.pair_for_control(&controller)?;
    }
    let first = controller.open_desktop(&controller.connect_services(&hosts[0])?)?;
    let second = controller.open_desktop(&controller.connect_services(&hosts[1])?)?;
    let pointer = PointerSettings {
        sensitivity_percent: 175,
        reverse_wheel: true,
        swap_buttons: true,
    };
    let core = &controller.core;
    core.set_input_defaults(KeyboardMode::Map, true, pointer)?;
    for desktop in [&first, &second] {
        assert_eq!(
            desktop.keyboard_mode()?,
            KeyboardMode::Map,
            "an open desktop takes the default keyboard mode"
        );
        assert!(
            desktop.swap_control_and_command()?,
            "an open desktop takes the default shortcut swap"
        );
        assert_eq!(
            desktop.pointer_settings()?,
            pointer,
            "an open desktop takes the default pointer settings"
        );
    }
    first.submit_local_key(local(usage::LEFT_CONTROL, true, ""))?;
    assert!(
        core.set_input_defaults(KeyboardMode::Character, false, PointerSettings::default())
            .is_err(),
        "a held key refuses new input defaults"
    );
    assert_eq!(
        core.keyboard_mode()?,
        KeyboardMode::Map,
        "a refused change leaves the default mode"
    );
    assert_eq!(
        second.pointer_settings()?,
        pointer,
        "a refused change leaves another desktop's pointer settings"
    );
    first.release_input()?;
    let third = controller.open_desktop(&controller.connect_services(&hosts[2])?)?;
    assert_eq!(
        third.pointer_settings()?,
        pointer,
        "a desktop opened later takes the default pointer settings"
    );
    assert_eq!(
        third.keyboard_mode()?,
        KeyboardMode::Map,
        "a desktop opened later takes the default keyboard mode"
    );
    assert!(
        core.set_input_defaults(
            KeyboardMode::Character,
            false,
            PointerSettings {
                sensitivity_percent: 0,
                ..pointer
            }
        )
        .is_err(),
        "an invalid pointer setting refuses the change"
    );
    assert_eq!(
        third.pointer_settings()?,
        pointer,
        "a refused change leaves the desktop's pointer settings"
    );
    records_nothing_typed(&trace, &[]);
    // Whether the release found the held key still queued depends on when
    // the desktop's sender ran; nothing reached the host refused.
    vnilink_testkit::assert_story(expect![[r#""#]], || {
        trace.tell(&["input"]).only(&["input refused"]).to_string()
    });
    Ok(())
}
