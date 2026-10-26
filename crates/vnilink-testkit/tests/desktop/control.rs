//! Who drives the host: keyboard modes and shortcuts, pointer policy, handing
//! control over, and a view-only viewer, each releasing what it held.

use expect_test::expect;
use std::sync::{Arc, Mutex};

use vnilink::DesktopControlState::{Contended, Controlling, Viewing};
use vnilink::{
    usage, Error, Grants, HostDesktop, InputEvent, KeyboardMode, LocalKey, PointerButton,
    PointerSettings, ServiceKind, SessionPhase,
};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog, SCREEN};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::lifecycle::pair;

/// What a host's desktop was asked to do: an input event, text typed as
/// characters, or text typed as the host layout's keys.
#[derive(Clone, Debug, PartialEq)]
enum Typed {
    Event(InputEvent),
    Text(String),
    Chord(String),
}

/// A desktop that records what it is asked to type, as well as the input
/// it applies.
struct TypingDesktop {
    typed: Arc<Mutex<Vec<Typed>>>,
}

impl TypingDesktop {
    fn record(&self, typed: Typed) {
        self.typed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(typed);
    }
}

impl HostDesktop for TypingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.record(Typed::Event(event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), Error> {
        self.record(Typed::Text(text.to_owned()));
        Ok(())
    }

    fn type_chord(&mut self, text: &str) -> Result<(), Error> {
        self.record(Typed::Chord(text.to_owned()));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        SCREEN
    }
}

/// Installs a [`TypingDesktop`] on `host`, and gives what it records.
fn install_typing(host: &Engine) -> Result<Arc<Mutex<Vec<Typed>>>, Error> {
    let typed = Arc::new(Mutex::new(Vec::new()));
    host.core.set_desktop(Box::new(TypingDesktop {
        typed: Arc::clone(&typed),
    }))?;
    Ok(typed)
}

/// Waits until `typed` holds as many items as `expected`, then states that
/// they are `expected`, for the contract `what`.
fn assert_typed(typed: &Mutex<Vec<Typed>>, expected: &[Typed], what: &str) {
    let got = wait_for(
        &format!("the host to be asked {} things", expected.len()),
        DEFAULT_WAIT,
        || {
            let got = typed
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            (got.len() >= expected.len()).then_some(got)
        },
    );
    assert_eq!(got, expected, "{what}");
}

/// Each desktop keeps the keyboard mode and shortcut swap it opened with:
/// changing one's leaves the other's, and neither changes while a key is
/// held. Releasing input lifts the held keys before a key sent after it,
/// closing one computer's desktop leaves the other's, and a desktop opened
/// afterwards takes the engine's settings, not the closed desktop's.
#[test]
fn keyboard_modes_and_shortcuts_belong_to_each_desktop_and_release_lifts_held_keys() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [Engine::start("host"), Engine::start("other-host")];
    let mut typed = Vec::new();
    let mut desktops = Vec::new();
    let mut connections = Vec::new();
    for (host, mode) in hosts
        .iter()
        .zip([KeyboardMode::Map, KeyboardMode::Character])
    {
        typed.push(install_typing(host)?);
        pair(host, &controller)?;
        controller.core.set_keyboard_mode(mode)?;
        let connection = controller.connect_services(&host)?;
        desktops.push(controller.open_desktop(&connection)?);
        connections.push(connection);
    }
    let (first, second) = (&desktops[0], &desktops[1]);
    assert_eq!(
        first.keyboard_mode()?,
        KeyboardMode::Map,
        "a desktop keeps the mode it opened with"
    );
    assert_eq!(
        second.keyboard_mode()?,
        KeyboardMode::Character,
        "each desktop opens with the engine's mode at the time"
    );
    first.set_swap_control_and_command(true)?;
    assert!(
        !second.swap_control_and_command()?,
        "swapping one desktop's shortcuts leaves another's"
    );

    let key = LocalKey::new;
    first.submit_local_key(key(usage::LEFT_META, true, ""))?;
    first.submit_local_key(key(usage::A, true, "a"))?;
    first.submit_local_key(key(usage::A, true, "a"))?;
    assert!(
        first.set_keyboard_mode(KeyboardMode::Character).is_err(),
        "a desktop's mode does not change while a key is held"
    );
    assert!(
        first.set_swap_control_and_command(false).is_err(),
        "a desktop's shortcut swap does not change while a key is held"
    );
    let pressed = [
        Typed::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
        Typed::Event(InputEvent::key_down(usage::A)),
        Typed::Event(InputEvent::key_down(usage::A)),
    ];
    assert_typed(
        &typed[0],
        &pressed,
        "a swapped Command reaches the host as Control, and a repeat as a repeat",
    );
    first.release_input()?;
    // A key sent at once after the release: the release goes before it.
    first.submit_input(InputEvent::key_down(usage::B))?;
    let mut released = pressed.to_vec();
    released.extend([
        Typed::Event(InputEvent::key_up(usage::A)),
        Typed::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
        Typed::Event(InputEvent::key_down(usage::B)),
    ]);
    assert_typed(
        &typed[0],
        &released,
        "releasing input lifts each held key once, before the key sent after it",
    );

    second.submit_local_key(key(usage::GRAVE, true, "@"))?;
    assert_typed(
        &typed[1],
        &[Typed::Text("@".into())],
        "Character mode types the character",
    );
    assert!(
        second.set_keyboard_mode(KeyboardMode::Map).is_err(),
        "a desktop's mode does not change while a key is held"
    );
    second.submit_local_key(key(usage::GRAVE, false, ""))?;
    second.set_keyboard_mode(KeyboardMode::Translate)?;
    second.submit_local_key(key(usage::GRAVE, true, "@"))?;
    second.submit_local_key(key(usage::GRAVE, false, ""))?;
    assert_typed(
        &typed[1],
        &[Typed::Text("@".into()), Typed::Chord("@".into())],
        "Translate mode types the character as the host layout's keys",
    );

    first.close();
    hosts[0].wait_released(Some(ServiceKind::Desktop));
    assert!(
        !second.is_cancelled(),
        "closing one computer's desktop leaves the other's open"
    );
    assert_eq!(
        second.keyboard_mode()?,
        KeyboardMode::Translate,
        "closing another desktop leaves this one's mode"
    );
    let fresh = controller.open_desktop(&connections[0])?;
    assert_eq!(
        fresh.keyboard_mode()?,
        KeyboardMode::Character,
        "a reopened desktop takes the engine's mode, not the closed desktop's"
    );
    assert!(
        !fresh.swap_control_and_command()?,
        "a reopened desktop takes the engine's shortcut swap, not the closed desktop's"
    );
    first.close();
    assert!(
        !fresh.is_cancelled(),
        "closing the old desktop again leaves the reopened one"
    );
    assert!(
        trace.events().iter().all(|event| !event.contains('@')),
        "no engine records the text typed"
    );
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "input"]));
    Ok(())
}

/// Pointer settings belong to the desktop they are set on, and do not change
/// while a button is held: the buttons swapped, a drag started with the
/// primary button is released with the button it pressed, and a desktop
/// opened afterwards starts from the defaults.
#[test]
fn pointer_settings_belong_to_one_desktop_and_a_swapped_drag_releases_the_button_it_pressed(
) -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    let policy = PointerSettings {
        sensitivity_percent: 50,
        reverse_wheel: true,
        swap_buttons: true,
    };
    desktop.set_pointer_settings(policy)?;
    assert!(
        desktop
            .submit_input(InputEvent::precise_wheel(9000, 0))
            .is_err(),
        "a wheel step beyond the limit is refused"
    );
    desktop.submit_input(InputEvent::motion(1, 0))?;
    desktop.submit_input(InputEvent::button_down(PointerButton::Left))?;
    assert!(
        desktop
            .set_pointer_settings(PointerSettings::default())
            .is_err(),
        "pointer settings do not change while a button is held"
    );
    desktop.submit_input(InputEvent::motion(1, 0))?;
    desktop.submit_input(InputEvent::precise_wheel(15, -30))?;
    desktop.submit_input(InputEvent::button_down(PointerButton::Back))?;
    assert_eq!(
        applied.wait_for(4),
        vec![
            InputEvent::button_down(PointerButton::Right),
            InputEvent::motion(1, 0),
            InputEvent::precise_wheel(-15, 30),
            InputEvent::button_down(PointerButton::Back),
        ],
        "the buttons are swapped, motion is halved and the wheel reversed"
    );
    desktop.release_input()?;
    assert_eq!(
        applied.wait_for(6)[4..],
        [
            InputEvent::button_up(PointerButton::Back),
            InputEvent::button_up(PointerButton::Right),
        ],
        "releasing input lifts each held button as it was pressed, last first"
    );
    desktop.set_pointer_settings(PointerSettings::default())?;
    desktop.close();
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert!(
        desktop.set_pointer_settings(policy).is_err(),
        "a closed desktop takes no pointer settings"
    );
    let fresh = pair.controller.open_desktop(&connection)?;
    assert_eq!(
        fresh.pointer_settings()?,
        PointerSettings::default(),
        "a reopened desktop starts from the default pointer settings"
    );
    fresh.close();
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=2 to=closed: PeerClose
    "#]].assert_eq(&trace.story(&["service", "input"]));
    Ok(())
}

/// Three viewers of one host: the first controls, the others contend. Asking
/// for control changes nothing until the host hands it over, and the
/// handover lifts the key the old controller held before the new one
/// presses it. The old controller's close does not lift the new one's key,
/// a yield does, and of two viewers asking for a free control exactly one
/// wins. Every viewer keeps its pictures throughout.
#[test]
fn handing_control_over_releases_held_keys_and_keeps_every_viewer() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let applied = InputLog::install(&host, SCREEN)?;
    let peers = [
        Engine::start("first"),
        Engine::start("second"),
        Engine::start("third"),
    ];
    let mut desktops = Vec::new();
    let mut owners = Vec::new();
    let mut videos = Vec::new();
    for peer in &peers {
        pair(&host, peer)?;
        let connection = peer.connect_services(&host)?;
        let desktop = peer.open_desktop(&connection)?;
        owners.push(
            host.hosted_for(ServiceKind::Desktop, &peer)
                .ok_or("the host serves the Desktop")?,
        );
        videos.push(desktop.subscribe_video()?);
        desktops.push(desktop);
    }
    let states = |desktops: &[vnilink::RemoteDesktopService]| {
        desktops
            .iter()
            .map(|desktop| desktop.control_state().ok())
            .collect::<Option<Vec<_>>>()
    };
    wait_for(
        "the first viewer to control and the others to contend",
        DEFAULT_WAIT,
        || (states(&desktops)? == [Controlling, Contended, Contended]).then_some(()),
    );

    let down = InputEvent::key_down(usage::A);
    let up = InputEvent::key_up(usage::A);
    desktops[0].submit_input(down)?;
    applied.wait_for(1);
    desktops[1].acquire_control()?;
    // The host answers the keyframe asked after the acquire, so it has seen
    // the acquire too.
    desktops[1].request_keyframe()?;
    wait_for(
        "the host to see the second viewer's keyframe request",
        DEFAULT_WAIT,
        || {
            host.core
                .take_desktop_keyframe_request(owners[1].id, owners[1].generation)
                .ok()?
                .then_some(())
        },
    );
    assert_eq!(
        desktops[1].control_state()?,
        Contended,
        "asking for held control does not take it"
    );

    host.core.transfer_desktop_control(
        owners[0].id,
        owners[0].generation,
        owners[1].id,
        owners[1].generation,
    )?;
    wait_for(
        "control to pass from the first viewer to the second",
        DEFAULT_WAIT,
        || {
            let states = states(&desktops)?;
            (states[0] == Viewing && states[1] == Controlling).then_some(())
        },
    );
    assert_eq!(
        applied.events(),
        vec![down, up],
        "handing control over lifts the old controller's key"
    );
    desktops[1].submit_input(down)?;
    assert_eq!(
        applied.wait_for(3),
        vec![down, up, down],
        "the new controller's key reaches the host"
    );
    assert!(
        matches!(
            host.core.transfer_desktop_control(
                owners[0].id,
                owners[0].generation,
                owners[2].id,
                owners[2].generation
            ),
            Err(Error::Busy)
        ),
        "only the controller's control can be handed over"
    );

    desktops[0].close();
    wait_for("the host to release the first viewer", DEFAULT_WAIT, || {
        (host.core.hosted_services().ok()?.len() == 2).then_some(())
    });
    assert_eq!(
        applied.events(),
        vec![down, up, down],
        "the old controller's close does not lift the new controller's key"
    );
    desktops[1].yield_control()?;
    wait_for("the second viewer to yield", DEFAULT_WAIT, || {
        (desktops[1].control_state().ok()? == Viewing).then_some(())
    });
    assert_eq!(
        applied.events(),
        vec![down, up, down, up],
        "yielding control lifts the held key"
    );
    // The race below has a winner of its own each run: the story ends here.
    trace.wait_for_records("control yielded", 1);
    trace.wait_for_records("control changed", 3);
    expect![[r#"
        first:
          input: control changed epoch=2 service=1 state=Viewing
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          input: control asked for epoch=0 service=2 state=Contended
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          input: control yielded current=true service=2
        second:
          input: control changed epoch=1 service=1 state=Controlling
          input: control changed epoch=1 service=1 state=Viewing
    "#]].assert_eq(&trace.story(&["service", "input"]));

    // Both remaining viewers ask for the free control at once.
    std::thread::scope(|scope| {
        for desktop in &desktops[1..] {
            scope.spawn(|| desktop.acquire_control());
        }
    });
    wait_for("exactly one of two askers to win", DEFAULT_WAIT, || {
        let states = states(&desktops[1..])?;
        (states.iter().filter(|state| **state == Controlling).count() == 1
            && states.contains(&Contended))
        .then_some(())
    });
    for index in 1..3 {
        let owner = &owners[index];
        host.core.publish_desktop_video(
            owner.id,
            owner.generation,
            keyframe(10, vec![index as u8]),
        )?;
        assert_eq!(
            next_picture(&videos[index], "a remaining viewer's picture").payload,
            vec![index as u8],
            "a viewer keeps its pictures through handovers"
        );
        assert!(
            !desktops[index].is_cancelled(),
            "a handover closes no viewer"
        );
    }
    assert!(
        desktops[0].acquire_control().is_err(),
        "a closed desktop cannot ask for control"
    );
    assert!(
        host.core
            .transfer_desktop_control(
                owners[0].id,
                owners[0].generation,
                owners[1].id,
                owners[1].generation
            )
            .is_err(),
        "a closed desktop's control cannot be handed over"
    );
    Ok(())
}

/// A viewer the host shares only to be seen cannot take control: its
/// acquire and its keys change nothing. Once control is shared it takes it;
/// when control is withdrawn again its held key is lifted, it views on, and
/// its pictures keep arriving until Desktop itself is withdrawn, the
/// connection staying up.
#[test]
fn a_view_only_viewer_cannot_take_control_and_losing_control_keeps_its_pictures() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let view_only = Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    };
    host.set_desktop_sharing(view_only)?;
    let connection = pair.connect_services();
    let desktop = pair.controller.open_desktop(&connection)?;
    let remote = pair
        .host
        .hosted_for(ServiceKind::Desktop, &pair.controller)
        .ok_or("the host serves the Desktop")?;
    assert_eq!(
        desktop.control_state()?,
        Viewing,
        "a view-only viewer opens viewing"
    );
    desktop.acquire_control()?;
    desktop.submit_input(InputEvent::key_down(usage::A))?;
    trace.wait_for_records("control refused: the peer may not control this computer", 1);
    trace.wait_for_records("input discarded", 1);
    host.take_desktop_keyframe_request(remote.id, remote.generation)?;
    desktop.request_keyframe()?;
    wait_for("the host to see the keyframe request", DEFAULT_WAIT, || {
        host.take_desktop_keyframe_request(remote.id, remote.generation)
            .ok()?
            .then_some(())
    });
    assert!(
        applied.events().is_empty(),
        "a view-only viewer's key is not applied"
    );
    assert_eq!(
        desktop.control_state()?,
        Viewing,
        "a view-only viewer cannot take control"
    );

    host.set_desktop_sharing(Grants {
        control: true,
        ..view_only
    })?;
    desktop.acquire_control()?;
    wait_for("the viewer to take shared control", DEFAULT_WAIT, || {
        (desktop.control_state().ok()? == Controlling).then_some(())
    });
    desktop.submit_input(InputEvent::key_down(usage::B))?;
    applied.wait_for(1);
    host.set_desktop_sharing(view_only)?;
    wait_for("the viewer to lose control", DEFAULT_WAIT, || {
        (desktop.control_state().ok()? == Viewing).then_some(())
    });
    assert_eq!(
        applied.wait_for(2),
        vec![InputEvent::key_down(usage::B), InputEvent::key_up(usage::B)],
        "withdrawing control lifts the held key"
    );
    // Withdrawing Desktop below may or may not overtake the viewer's last
    // word on control: the story ends here.
    trace.wait_for_records("control changed", 2);
    expect![[r#"
        controller:
          input: control changed epoch=1 service=1 state=Controlling
          input: control changed epoch=2 service=1 state=Viewing
        host:
          input: control refused: the peer may not control this computer service=1
          input: input discarded kind=key down service=1 why=the peer may not control this computer
          input: control asked for epoch=0 service=1 state=Controlling
    "#]]
    .assert_eq(&trace.story(&["input"]));

    let video = desktop.subscribe_video()?;
    host.publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![7]))?;
    assert_eq!(
        next_picture(&video, "the viewer's picture after losing control").payload,
        vec![7],
        "a viewer that lost control keeps its pictures"
    );
    host.set_service_sharing(ServiceKind::Desktop, false)?;
    wait_for("the withdrawn desktop to end", DEFAULT_WAIT, || {
        desktop.is_cancelled().then_some(())
    });
    assert_eq!(
        controller.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "withdrawing Desktop leaves the connection up"
    );
    Ok(())
}
