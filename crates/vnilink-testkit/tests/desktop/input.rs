//! Input on a desktop session: what the host's desktop applies, what it
//! lifts when the session ends, and what it reports when its operating
//! system refuses an event.

use expect_test::expect;
use std::sync::{Arc, Mutex};

use vnilink::{
    usage, DisplayInfo, Error, HostDesktop, InputEvent, PointerButton, SessionPhase, SessionRole,
};
use vnilink_testkit::desktop::{input_failed, InputLog};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::keyboard::records_nothing_typed;
use super::streams::SCREEN;

/// The display a session's pointer moves on: the host's one monitor.
const DISPLAY: u64 = 1;

/// The host publishes its one display, the viewer learns it and selects
/// it, and the host applies that selection: pointer input has a display to
/// land on.
fn arm_display(pair: &Pair, id: u64, generation: u64) -> Result<(), Error> {
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_topology(
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
    )?;
    wait_for(
        "the viewer to learn the host's display",
        DEFAULT_WAIT,
        || {
            controller
                .session_topology(id, generation)
                .ok()?
                .iter()
                .any(|display| display.id == DISPLAY)
                .then_some(())
        },
    );
    controller.select_displays(id, generation, &[DISPLAY])?;
    wait_for("the host to apply the selection", DEFAULT_WAIT, || {
        host.video_targets()
            .ok()?
            .iter()
            .any(|target| {
                !target.service
                    && host
                        .incoming_display_selection(target.id, target.generation)
                        .ok()
                        .as_deref()
                        == Some(&[DISPLAY][..])
            })
            .then_some(())
    });
    Ok(())
}

/// A desktop of one `monitor`-sized display that logs what it applies, and
/// refuses a press of `fail_key_down`, or every release when
/// `fail_release`, as an operating system can.
struct RefusingDesktop {
    monitor: (u16, u16),
    applied: Arc<Mutex<Vec<InputEvent>>>,
    fail_key_down: Option<u16>,
    fail_release: bool,
}

impl HostDesktop for RefusingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        match event {
            InputEvent::Key { down: false, .. } | InputEvent::Button { down: false, .. }
                if self.fail_release =>
            {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            InputEvent::Key { usage, down: true } if self.fail_key_down == Some(usage) => {
                return Err(Error::InputFailed(
                    "the operating system rejected the event",
                ));
            }
            _ => {}
        }
        self.applied
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(event);
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        self.monitor
    }
}

/// What a [`RefusingDesktop`] applied.
#[derive(Clone, Default)]
struct Applied(Arc<Mutex<Vec<InputEvent>>>);

impl Applied {
    fn install(
        host: &Engine,
        monitor: (u16, u16),
        fail_key_down: Option<u16>,
        fail_release: bool,
    ) -> Result<Self, Error> {
        let applied = Self::default();
        host.core.set_desktop(Box::new(RefusingDesktop {
            monitor,
            applied: Arc::clone(&applied.0),
            fail_key_down,
            fail_release,
        }))?;
        Ok(applied)
    }

    fn events(&self) -> Vec<InputEvent> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Waits until as many events as `expected` were applied, and states
    /// they are `expected`, for the contract `what`.
    fn wait(&self, expected: &[InputEvent], what: &str) {
        let got = wait_for(
            &format!("the host to apply {} events", expected.len()),
            DEFAULT_WAIT,
            || {
                let got = self.events();
                (got.len() >= expected.len()).then_some(got)
            },
        );
        assert_eq!(got, expected, "{what}");
    }
}

/// A host with a desktop offers keyboard, pointer and its one monitor; the
/// keys, pointer moves, button and wheel a viewer sends apply in order,
/// pointer moves still queued together coalescing to the last, and ending
/// the session lifts the held button and key, last pressed first.
#[test]
fn keyboard_and_pointer_events_are_applied_and_released() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let capabilities = host.capabilities();
    assert!(
        capabilities.keyboard && capabilities.pointer,
        "a host with a desktop offers keyboard and pointer"
    );
    assert_eq!(capabilities.monitors, 1, "the host offers its one monitor");
    let session = pair.connect_control();
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        SCREEN,
        "the viewer learns the host's monitor"
    );
    arm_display(&pair, session.id, session.generation)?;
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
        controller.submit_input(session.id, session.generation, *event)?;
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
        "ending the session lifts the held button, then the held key"
    );
    records_nothing_typed(&trace, &[]);
    expect![[r#""#]].assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// A key and buttons pressed and released apply as sent, and ending the
/// session lifts nothing more: none is still held.
#[test]
fn key_up_and_each_button_are_applied_and_not_released_again() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = InputLog::install(&pair.host, SCREEN)?;
    let session = pair.connect_control();
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
        controller.submit_input(session.id, session.generation, event)?;
    }
    assert_eq!(
        applied.exactly(expected.len()),
        expected,
        "each press and release applies as sent"
    );
    host.stop_hosted_session()?;
    pair.host.wait_closed(SessionRole::Host);
    pair.controller.wait_closed(SessionRole::Controller);
    assert_eq!(
        applied.events(),
        expected,
        "ending the session releases nothing already released"
    );
    records_nothing_typed(&trace, &[]);
    expect![[r#""#]].assert_eq(&trace.story(&["input"]));
    Ok(())
}

/// A key the host's operating system refuses is reported on the host, and
/// the session stays open: the keys and pointer moves after it apply.
#[test]
fn an_apply_failure_is_reported_and_the_session_stays_open() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = Applied::install(&pair.host, (640, 480), Some(usage::B), false)?;
    let events = host.subscribe()?;
    let session = pair.connect_control();
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (640, 480),
        "the viewer learns the host's monitor"
    );
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::B),
    )?;
    assert_eq!(
        input_failed(&events),
        (session.id, "the operating system rejected the event"),
        "the host reports the key its system refused, on the session"
    );
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    arm_display(&pair, session.id, session.generation)?;
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::pointer(3, 4, 1, 1),
    )?;
    applied.wait(
        &[
            InputEvent::key_down(usage::A),
            InputEvent::pointer(3, 4, 1, 1),
        ],
        "the key and pointer after a refused key apply",
    );
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "a refused key leaves the session open"
    );
    controller.submit_input(session.id, session.generation, InputEvent::key_up(usage::A))?;
    records_nothing_typed(&trace, &[]);
    trace.wait_for_records("input failed", 1);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          warn input: input failed reason=the operating system rejected the event session=1
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// When the session ends and the host's system refuses to lift what was
/// held, the host reports it on the session, and the session still closes.
#[test]
fn a_failed_release_is_reported_on_the_hosted_session() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let applied = Applied::install(&pair.host, (800, 600), None, true)?;
    let events = host.subscribe()?;
    let session = pair.connect_control();
    let held = [
        InputEvent::key_down(usage::A),
        InputEvent::button_down(PointerButton::Left),
    ];
    for event in held {
        controller.submit_input(session.id, session.generation, event)?;
    }
    applied.wait(&held, "the key and button apply");
    host.stop_hosted_session()?;
    assert_eq!(
        input_failed(&events),
        (session.id, "a held key or button could not be released"),
        "the host reports the failed release on the session"
    );
    pair.host.wait_closed(SessionRole::Host);
    assert_eq!(
        applied.events(),
        held,
        "a refused release applies nothing more"
    );
    records_nothing_typed(&trace, &[]);
    expect![[r#""#]].assert_eq(&trace.story(&["input"]));
    Ok(())
}
