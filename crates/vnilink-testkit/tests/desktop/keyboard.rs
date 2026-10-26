//! What a viewer types into a desktop session: the keyboard modes, the
//! Control and Command swap, finished text, and what the host does when its
//! desktop refuses a character. Typed text and key identities are the
//! person's own: no engine records them.

use expect_test::expect;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vnilink::{
    usage, Error, EventSubscription, HostDesktop, InputEvent, KeyboardMode, LocalKey,
    PointerSettings, SessionInfo, SessionPhase, MAX_KEY_TEXT,
};
use vnilink_testkit::{holds, traced, wait_for, Engine, Pair, TestResult, Traced, DEFAULT_WAIT};

use super::lifecycle::pair;
use super::streams::{input_failure, SCREEN};

/// What a host's desktop was asked to do: an input event, text typed as
/// characters, or text typed as the host layout's keys.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Seen {
    Event(InputEvent),
    Text(String),
    Chord(String),
}

/// A desktop that records what it is asked to type, and refuses text or
/// translated keys where told to, as an operating system can.
struct TypingDesktop {
    seen: Arc<Mutex<Vec<Seen>>>,
    fail_text: bool,
    fail_chord: bool,
}

impl TypingDesktop {
    fn record(&self, seen: Seen) {
        self.seen
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(seen);
    }
}

impl HostDesktop for TypingDesktop {
    fn apply(&mut self, event: InputEvent) -> Result<(), Error> {
        self.record(Seen::Event(event));
        Ok(())
    }

    fn type_text(&mut self, text: &str) -> Result<(), Error> {
        if self.fail_text {
            return Err(Error::InputFailed(
                "the operating system rejected the event",
            ));
        }
        self.record(Seen::Text(text.to_owned()));
        Ok(())
    }

    fn type_chord(&mut self, text: &str) -> Result<(), Error> {
        if self.fail_chord {
            return Err(Error::InputFailed("translated keys are not available"));
        }
        self.record(Seen::Chord(text.to_owned()));
        Ok(())
    }

    fn monitor(&self) -> (u16, u16) {
        SCREEN
    }
}

/// What a [`TypingDesktop`] was asked to do.
#[derive(Clone, Default)]
struct Typed(Arc<Mutex<Vec<Seen>>>);

impl Typed {
    /// Installs on `host` a desktop that records what it is asked to type,
    /// refusing text when `fail_text` and translated keys when
    /// `fail_chord`.
    fn install(host: &Engine, fail_text: bool, fail_chord: bool) -> Result<Self, Error> {
        let typed = Self::default();
        host.core.set_desktop(Box::new(TypingDesktop {
            seen: Arc::clone(&typed.0),
            fail_text,
            fail_chord,
        }))?;
        Ok(typed)
    }

    fn seen(&self) -> Vec<Seen> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Waits until the host was asked exactly `expected`, in order, for the
    /// contract `what`: one more, or one fewer, fails.
    fn wait(&self, expected: &[Seen], what: &str) {
        let got = wait_for(
            &format!("the host to be asked {} things", expected.len()),
            DEFAULT_WAIT,
            || {
                let got = self.seen();
                (got.len() >= expected.len()).then_some(got)
            },
        );
        assert_eq!(got, expected, "{what}");
    }
}

/// States that no engine recorded any of `typed`, and that no record names
/// a key or a character: what a person types is theirs.
pub(super) fn records_nothing_typed(trace: &Traced, typed: &[&str]) {
    for event in trace.events() {
        for text in typed {
            assert!(
                !event.contains(text),
                "an engine recorded the text typed, {text:?}: {event}"
            );
        }
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

/// The controller's control session to `host`, a desktop session as a
/// window opens one.
fn desktop_session(pair: &Pair) -> SessionInfo {
    pair.connect_control()
}

/// The Control and Command swap waits for held keys, as the keyboard mode
/// does: refused while a key is down, taken once it is up.
#[test]
fn the_swap_waits_for_held_keys_like_the_mode_switch() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let session = desktop_session(&pair);
    controller.submit_local_key(session.id, session.generation, local(usage::A, true, "a"))?;
    assert!(
        matches!(
            controller.set_swap_control_and_command(true),
            Err(Error::InvalidSetting(_))
        ),
        "the swap does not change while a key is held"
    );
    controller.submit_local_key(session.id, session.generation, local(usage::A, false, "a"))?;
    controller.set_swap_control_and_command(true)?;
    assert!(
        controller.swap_control_and_command()?,
        "the swap changes once no key is held"
    );
    records_nothing_typed(&trace, &[]);
    trace.wait_for_records(
        "input on this session closed: this computer cannot apply it",
        1,
    );
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          warn input: input failed reason=no desktop adapter is installed session=1
          warn input: input on this session closed: this computer cannot apply it session=1
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// With Control and Command swapped, a Command pressed on the viewer reaches
/// the host as Control, and Control is what the host lifts when the session
/// ends.
#[test]
fn a_swapped_command_is_the_key_the_host_releases() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typed::install(&pair.host, false, false)?;
    controller.set_swap_control_and_command(true)?;
    let session = desktop_session(&pair);
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::LEFT_META),
    )?;
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::LEFT_SHIFT),
    )?;
    let pressed = [
        Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
        Seen::Event(InputEvent::key_down(usage::LEFT_SHIFT)),
    ];
    typed.wait(&pressed, "a swapped Command reaches the host as Control");
    host.stop_hosted_session()?;
    let mut released = pressed.to_vec();
    released.extend([
        Seen::Event(InputEvent::key_up(usage::LEFT_SHIFT)),
        Seen::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
    ]);
    typed.wait(
        &released,
        "the host lifts the Control it pressed, last pressed first",
    );
    records_nothing_typed(&trace, &[]);
    expect![[r#""#]].assert_eq(&trace.story(&["input"]));
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
    let typed = Typed::install(&pair.host, false, false)?;
    let session = desktop_session(&pair);
    let events = controller.subscribe()?;
    let host_events = host.subscribe()?;
    let (id, generation) = (session.id, session.generation);
    assert!(
        matches!(
            controller.submit_text(id, generation, ""),
            Err(Error::InvalidSetting("text entry is empty"))
        ),
        "empty text is refused"
    );
    assert!(
        matches!(
            controller.submit_text(id, generation, &"a".repeat(MAX_KEY_TEXT + 1)),
            Err(Error::InvalidSetting("text entry is longer than 128 bytes"))
        ),
        "text longer than the bound is refused"
    );
    assert!(
        matches!(
            controller.submit_text(id, generation, "a\0b"),
            Err(Error::InvalidSetting("text entry contains a null byte"))
        ),
        "text with a null byte is refused"
    );
    controller.submit_input(id, generation, InputEvent::key_down(usage::LEFT_META))?;
    controller.submit_text(id, generation, "café")?;
    controller.submit_input(id, generation, InputEvent::key_down(usage::ENTER))?;
    typed.wait(
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_META)),
            Seen::Text("café".to_owned()),
            Seen::Event(InputEvent::key_down(usage::ENTER)),
        ],
        "finished text is typed between the keys sent around it",
    );
    hides(&events, "café");
    hides(&host_events, "café");
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "typing text leaves the session open"
    );
    records_nothing_typed(&trace, &["café", "caf"]);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// Text the host's operating system refuses is reported on the host, and the
/// keys sent after it still apply.
#[test]
fn a_refused_text_leaves_the_keys_working() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typed::install(&pair.host, true, false)?;
    let session = desktop_session(&pair);
    let events = host.subscribe()?;
    controller.submit_text(session.id, session.generation, "nope")?;
    assert_eq!(
        input_failure(&events),
        "the operating system rejected the event",
        "the host reports the text its system refused"
    );
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    typed.wait(
        &[Seen::Event(InputEvent::key_down(usage::A))],
        "a key after a refused text applies",
    );
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "a refused text leaves the session open"
    );
    records_nothing_typed(&trace, &["nope"]);
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

/// Text typed before the link dropped is not typed again after the session
/// reconnects, and text sent on the old generation is refused.
#[test]
fn a_dropped_session_does_not_replay_finished_text() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let controller = &pair.controller.core;
    let typed = Typed::install(&pair.host, false, false)?;
    let session = desktop_session(&pair);
    controller.submit_text(session.id, session.generation, "first words")?;
    typed.wait(
        &[Seen::Text("first words".to_owned())],
        "the first text is typed",
    );
    controller.drop_link()?;
    wait_for("the dropped session to leave Active", DEFAULT_WAIT, || {
        controller
            .snapshot()
            .ok()?
            .outgoing
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
            .then_some(())
    });
    assert!(
        matches!(
            controller.submit_text(session.id, session.generation, "replayed words"),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "text sent on a dropped session is refused"
    );
    let again = wait_for("the session to reconnect", DEFAULT_WAIT, || {
        controller.snapshot().ok()?.outgoing.filter(|outgoing| {
            outgoing.phase == SessionPhase::Active && outgoing.generation != session.generation
        })
    });
    controller.submit_text(again.id, again.generation, "second words")?;
    typed.wait(
        &[
            Seen::Text("first words".to_owned()),
            Seen::Text("second words".to_owned()),
        ],
        "a reconnected session types only what was sent on it",
    );
    records_nothing_typed(&trace, &["first words", "second words", "replayed words"]);
    trace.wait_for_records("the session's control stream ended", 1);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=active generation=1 role=Controller session=1 to=reconnecting
          session: session phase from=reconnecting generation=3 role=Controller session=1 to=active
        host:
          input: the session's control stream ended error=network session=1
          session: session phase from=active generation=1 role=Host session=1 to=closed
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// In the default Character mode, `@` reaches the host as the character,
/// whichever keys made it on the viewer, and no engine records it.
#[test]
fn the_default_mode_inserts_at_from_either_keyboard() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typed::install(&pair.host, false, false)?;
    assert_eq!(
        controller.keyboard_mode()?,
        KeyboardMode::Character,
        "Character is the default mode"
    );
    let session = desktop_session(&pair);
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
        controller.submit_local_key(session.id, session.generation, key)?;
    }
    typed.wait(
        &[Seen::Text("@".to_owned()), Seen::Text("@".to_owned())],
        "each keyboard's @ is typed as the character",
    );
    hides(&events, "@");
    hides(&host_events, "@");
    records_nothing_typed(&trace, &["@"]);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// Map and Translate are chosen, not the default; a held key blocks the
/// change, Map sends the key and Translate the host layout's keys.
#[test]
fn map_and_translate_are_optional_and_a_held_key_blocks_the_change() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typed::install(&pair.host, false, false)?;
    let session = desktop_session(&pair);
    let (id, generation) = (session.id, session.generation);
    controller.submit_local_key(id, generation, local(usage::LEFT_SHIFT, true, ""))?;
    assert!(
        matches!(
            controller.set_keyboard_mode(KeyboardMode::Map),
            Err(Error::InvalidSetting(
                "release held keys before changing the keyboard mode"
            ))
        ),
        "the mode does not change while a key is held"
    );
    controller.submit_local_key(id, generation, local(usage::LEFT_SHIFT, false, ""))?;
    controller.set_keyboard_mode(KeyboardMode::Map)?;
    controller.submit_local_key(id, generation, local(usage::GRAVE, true, "@"))?;
    typed.wait(
        &[Seen::Event(InputEvent::key_down(usage::GRAVE))],
        "Map sends the key, not the character",
    );
    controller.submit_local_key(id, generation, local(usage::GRAVE, false, ""))?;
    controller.set_keyboard_mode(KeyboardMode::Translate)?;
    controller.submit_local_key(id, generation, local(usage::GRAVE, true, "@"))?;
    typed.wait(
        &[
            Seen::Event(InputEvent::key_down(usage::GRAVE)),
            Seen::Event(InputEvent::key_up(usage::GRAVE)),
            Seen::Chord("@".to_owned()),
        ],
        "Translate types the character as the host layout's keys",
    );
    let events = host.subscribe()?;
    hides(&events, "@");
    records_nothing_typed(&trace, &["@"]);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// Translated keys the host cannot type are reported on the host, and the
/// keys sent after them still apply.
#[test]
fn a_refused_chord_leaves_the_keys_working() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let typed = Typed::install(&pair.host, false, true)?;
    let session = desktop_session(&pair);
    controller.set_keyboard_mode(KeyboardMode::Translate)?;
    let events = host.subscribe()?;
    controller.submit_local_key(
        session.id,
        session.generation,
        local(usage::GRAVE, true, "@"),
    )?;
    assert_eq!(
        input_failure(&events),
        "translated keys are not available",
        "the host reports the translated keys it could not type"
    );
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    typed.wait(
        &[Seen::Event(InputEvent::key_down(usage::A))],
        "a key after a refused chord applies",
    );
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "a refused chord leaves the session open"
    );
    records_nothing_typed(&trace, &["@"]);
    trace.wait_for_records("input failed", 1);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          warn input: input failed reason=translated keys are not available session=1
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}

/// The controller's input defaults reach its open sessions, connecting or
/// active, and the ones it opens later; a held key refuses the whole change,
/// and an invalid pointer setting changes nothing.
#[test]
fn input_defaults_cover_live_and_future_connections_and_refuse_held_input_atomically() -> TestResult
{
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [
        Engine::start("host"),
        Engine::start("second-host"),
        Engine::start("third-host"),
    ];
    for host in &hosts {
        Typed::install(host, false, false)?;
        pair(host, &controller)?;
    }
    let first = controller.connect_control(&hosts[0])?;
    let second = controller.connect_control(&hosts[1])?;
    let pointer = PointerSettings {
        sensitivity_percent: 175,
        reverse_wheel: true,
        swap_buttons: true,
    };
    let core = &controller.core;
    core.set_input_defaults(KeyboardMode::Map, true, pointer)?;
    for session in [&first, &second] {
        assert_eq!(
            core.session_keyboard_mode(session.id, session.generation)?,
            KeyboardMode::Map,
            "an open session takes the default keyboard mode"
        );
        assert!(
            core.session_swap_control_and_command(session.id, session.generation)?,
            "an open session takes the default shortcut swap"
        );
        assert_eq!(
            core.session_pointer_settings(session.id, session.generation)?,
            pointer,
            "an open session takes the default pointer settings"
        );
    }
    core.submit_local_key(
        first.id,
        first.generation,
        local(usage::LEFT_CONTROL, true, ""),
    )?;
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
        core.session_pointer_settings(second.id, second.generation)?,
        pointer,
        "a refused change leaves another session's pointer settings"
    );
    core.release_session_input(first.id, first.generation)?;
    let third = controller.connect_control(&hosts[2])?;
    assert_eq!(
        core.session_pointer_settings(third.id, third.generation)?,
        pointer,
        "a session opened later takes the default pointer settings"
    );
    assert_eq!(
        core.session_keyboard_mode(third.id, third.generation)?,
        KeyboardMode::Map,
        "a session opened later takes the default keyboard mode"
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
        core.session_pointer_settings(third.id, third.generation)?,
        pointer,
        "a refused change leaves the session's pointer settings"
    );
    records_nothing_typed(&trace, &[]);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: session phase from=connecting generation=1 role=Controller session=2 to=active
          session: session phase from=connecting generation=1 role=Controller session=3 to=active
    "#]]
    .assert_eq(&trace.story(&["input", "session"]));
    Ok(())
}
