//! Text copied over a control session: the controller pastes onto the
//! host, the host publishes to the session it selected, each under the
//! host's grant and adapter.

use expect_test::expect;

use super::{assert_unrecorded, paired, sharing, Copied};
use vnilink::{usage, CoreEvent, Error, InputEvent, SessionPhase, MAX_CLIPBOARD_TEXT};
use vnilink_testkit::desktop::{InputLog, RecordingClipboard};
use vnilink_testkit::{traced, wait_for, TestResult, DEFAULT_WAIT};

/// Retries the host's `publish` until its side of the clipboard stream is
/// open: a controller sends Ready before the host installs its sender.
/// Every other failure fails at once.
fn publish_once_open(what: &str, mut publish: impl FnMut() -> Result<(), Error>) {
    wait_for(what, DEFAULT_WAIT, || match publish() {
        Ok(()) => Some(()),
        Err(Error::Unsupported("clipboard stream is not open")) => None,
        Err(error) => panic!("the host's publication failed: {error}"),
    });
}

/// Without the clipboard grant a paste is refused and the session carries
/// on, keys and all; the adapter cannot be swapped under the session, and
/// once the grant is given the paste, empty text too, reaches the host.
/// A clipboard alone does not make the host offer files.
#[test]
fn a_paste_needs_the_grant_and_the_session_lives_without_it() -> TestResult {
    let trace = traced();
    let pair = paired(false);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The host shares no folders, so only the clipboard could offer files.
    host.set_file_roots(Vec::new())?;
    let copied = Copied::install(&pair.host)?;
    let keys = InputLog::install(&pair.host, (1920, 1080))?;
    assert!(
        host.capabilities().clipboard,
        "an installed clipboard is a capability"
    );
    assert!(
        !host.capabilities().file_transfer,
        "a clipboard does not offer files"
    );
    let session = pair.connect_control();
    assert!(
        matches!(
            host.set_clipboard(Box::new(RecordingClipboard {
                texts: std::sync::Arc::default(),
            })),
            Err(Error::Busy)
        ),
        "a session's clipboard is not swapped under it"
    );
    assert!(
        matches!(
            controller.submit_clipboard(session.id, session.generation, "secret-paste"),
            Err(Error::ClipboardDenied)
        ),
        "a paste without the grant is refused"
    );
    assert!(copied.texts().is_empty(), "a refused paste is not applied");
    assert_eq!(
        controller.snapshot()?.outgoing.ok_or("the session")?.phase,
        SessionPhase::Active,
        "a refused paste leaves the session up"
    );
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(usage::A),
    )?;
    assert_eq!(
        keys.exactly(1),
        [InputEvent::key_down(usage::A)],
        "control goes on after a refused paste"
    );
    assert!(
        matches!(
            controller.submit_clipboard(session.id, session.generation + 1, "secret-nope"),
            Err(Error::StaleSession)
        ),
        "a paste for another generation is stale"
    );
    host.set_desktop_sharing(sharing(true))?;
    controller.submit_clipboard(session.id, session.generation, "secret-paste")?;
    controller.submit_clipboard(session.id, session.generation, "")?;
    assert_eq!(
        copied.texts(),
        ["secret-paste", ""],
        "granted pastes reach the host, empty text clearing it"
    );
    assert!(
        !host.capabilities().file_transfer,
        "granting the clipboard does not offer files"
    );
    trace.wait_for_records("clipboard received", 3);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=12 kind=text
          clipboard: clipboard answered amount=12 kind=text refused=clipboard denied
          clipboard: clipboard not delivered direction=to the host kind=text refused=clipboard denied
          clipboard: clipboard not delivered direction=to the host kind=text refused=stale session
          clipboard: clipboard sent amount=12 kind=text
          clipboard: clipboard answered amount=12 kind=text
          clipboard: clipboard sent amount=0 kind=text
          clipboard: clipboard answered amount=0 kind=text
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received amount=12 kind=text refused=clipboard denied
          clipboard: clipboard received amount=12 kind=text
          clipboard: clipboard received amount=0 kind=text
    "#]].assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["secret-paste", "secret-nope"]);
    Ok(())
}

/// Text over the limit is refused before it leaves the controller; the
/// next paste crosses and the session carries on.
#[test]
fn text_over_the_limit_is_refused_before_it_is_sent() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let copied = Copied::install(&pair.host)?;
    let session = pair.connect_control();
    let too_big = "a".repeat(MAX_CLIPBOARD_TEXT + 1);
    assert!(
        matches!(
            controller.submit_clipboard(session.id, session.generation, &too_big),
            Err(Error::InvalidSetting(_))
        ),
        "text over the limit is refused"
    );
    assert!(copied.texts().is_empty(), "text over the limit is not sent");
    controller.submit_clipboard(session.id, session.generation, "ok")?;
    assert_eq!(copied.texts(), ["ok"], "the next paste crosses");
    assert_eq!(
        controller.snapshot()?.outgoing.ok_or("the session")?.phase,
        SessionPhase::Active,
        "refusing text leaves the session up"
    );
    assert!(
        host.snapshot()?.incoming.is_some(),
        "the host keeps its session"
    );
    trace.wait_for_records("clipboard received", 1);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard not delivered direction=to the host kind=text reason=clipboard text is larger than 256 KiB refused=invalid setting
          clipboard: clipboard sent amount=2 kind=text
          clipboard: clipboard answered amount=2 kind=text
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received amount=2 kind=text
    "#]].assert_eq(&trace.story(&["clipboard"]));
    Ok(())
}

/// The host's text reaches the controller only under the grant, empty text
/// too, and the controller is told it arrived without being told what it
/// says; a controller has nothing of its own to publish.
#[test]
fn the_hosts_text_reaches_the_controller_under_the_grant() -> TestResult {
    let trace = traced();
    let pair = paired(false);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    let events = controller.subscribe()?;
    assert!(
        matches!(
            host.publish_clipboard(hosted.id, "secret-from-host"),
            Err(Error::ClipboardDenied)
        ),
        "the host publishes nothing without the grant"
    );
    assert!(
        controller.clipboard_text()?.is_none(),
        "a refused publication does not arrive"
    );
    host.set_desktop_sharing(sharing(true))?;
    publish_once_open("the host's text to be sent", || {
        host.publish_clipboard(hosted.id, "secret-from-host")
    });
    wait_for("the host's text to arrive", DEFAULT_WAIT, || {
        (controller
            .clipboard_text()
            .expect("the clipboard text")
            .as_deref()
            == Some("secret-from-host"))
        .then_some(())
    });
    host.publish_clipboard(hosted.id, "")?;
    wait_for("the host's empty text to arrive", DEFAULT_WAIT, || {
        (controller
            .clipboard_text()
            .expect("the clipboard text")
            .as_deref()
            == Some(""))
        .then_some(())
    });
    let mut told = false;
    while let Some(event) = events.try_recv() {
        assert!(
            !format!("{event:?}").contains("secret-from-host"),
            "an event carries the host's text: {event:?}"
        );
        if let CoreEvent::ClipboardReceived { id } = event {
            assert_eq!(id, session.id, "the event names the session");
            told = true;
        }
    }
    assert!(told, "the controller is told the host's text arrived");
    assert!(
        matches!(
            controller.publish_clipboard(session.id, "secret-nope"),
            Err(Error::NoSession)
        ),
        "a controller hosts no session to publish to"
    );
    trace.wait_for_records("clipboard received", 2);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard received amount=16 kind=text
          clipboard: clipboard received amount=0 kind=text
          clipboard: clipboard not delivered direction=to the controller kind=text refused=no session
        host:
          clipboard: clipboard not delivered direction=to the controller kind=text refused=clipboard denied
          clipboard: clipboard sent amount=16 kind=text
          clipboard: clipboard sent amount=0 kind=text
    "#]].assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["secret-from-host", "secret-nope"]);
    Ok(())
}

/// A host without a clipboard refuses a paste as unsupported, and the
/// session carries on.
#[test]
fn a_host_without_a_clipboard_refuses_a_paste_and_the_session_lives() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    assert!(
        !host.capabilities().clipboard,
        "no clipboard is installed, so none is offered"
    );
    let session = pair.connect_control();
    assert!(
        matches!(
            controller.submit_clipboard(session.id, session.generation, "secret-paste"),
            Err(Error::Unsupported(_))
        ),
        "a host without a clipboard refuses a paste"
    );
    assert_eq!(
        host.snapshot()?.incoming.ok_or("the host's session")?.phase,
        SessionPhase::Active,
        "a refused paste leaves the session up"
    );
    assert!(
        controller.clipboard_text()?.is_none(),
        "the controller was sent nothing"
    );
    trace.wait_for_records("clipboard received", 1);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=12 kind=text
          clipboard: clipboard answered amount=12 kind=text refused=unsupported
          clipboard: clipboard not delivered direction=to the host kind=text reason=clipboard is not available refused=unsupported
        host:
          clipboard: clipboard received amount=12 kind=text reason=no clipboard adapter is installed refused=unsupported
    "#]].assert_eq(&trace.story(&["clipboard"]));
    assert_unrecorded(&trace, &["secret-paste"]);
    Ok(())
}

/// The host's clipboard belongs to the session it admitted last: one that
/// closes is gone, not merely deselected, and the next controller's session
/// takes the clipboard without seeing the first one's text.
#[test]
fn the_clipboard_follows_the_hosts_session_from_one_controller_to_the_next() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, first) = (&pair.host, &pair.controller);
    Copied::install(host)?;
    let second = pair.add_viewer("other-controller");
    assert!(
        host.core.clipboard_owner_session()?.is_none(),
        "no session, no clipboard owner"
    );

    let _one = first.connect_control(host)?;
    let incoming_one = host.core.snapshot()?.incoming.ok_or("the first session")?;
    assert_eq!(
        host.core.clipboard_owner_session()?.ok_or("an owner")?.id,
        incoming_one.id,
        "the admitted session owns the clipboard"
    );
    publish_once_open("the host's text to be sent", || {
        host.core.publish_clipboard(incoming_one.id, "from-first")
    });
    wait_for("the first controller's text", DEFAULT_WAIT, || {
        (first
            .core
            .clipboard_text()
            .expect("the clipboard text")
            .as_deref()
            == Some("from-first"))
        .then_some(())
    });

    first.core.disconnect()?;
    wait_for("the first session to close", DEFAULT_WAIT, || {
        host.core
            .snapshot()
            .ok()?
            .incoming
            .is_some_and(|session| session.phase == SessionPhase::Closed)
            .then_some(())
    });
    assert!(
        host.core.clipboard_owner_session()?.is_none(),
        "a closed session owns no clipboard"
    );
    assert!(
        matches!(
            host.core.publish_clipboard(incoming_one.id, "stale-text"),
            Err(Error::NoSession)
        ),
        "a closed session is gone, not merely deselected"
    );

    let _two = second.connect_control(host)?;
    let incoming_two = host.core.snapshot()?.incoming.ok_or("the second session")?;
    assert_ne!(
        incoming_two.id, incoming_one.id,
        "the next session is a new one"
    );
    assert_eq!(
        host.core
            .clipboard_owner_session()?
            .ok_or("an owner")?
            .peer_id,
        second.endpoint(),
        "the next controller's session owns the clipboard"
    );
    publish_once_open("the host's text to be sent", || {
        host.core.publish_clipboard(incoming_two.id, "from-second")
    });
    wait_for("the second controller's text", DEFAULT_WAIT, || {
        (second
            .core
            .clipboard_text()
            .expect("the clipboard text")
            .as_deref()
            == Some("from-second"))
        .then_some(())
    });
    assert_ne!(
        first.core.clipboard_text()?.as_deref(),
        Some("from-second"),
        "the second session's text does not reach the first controller"
    );
    trace.wait_for_records("clipboard received", 2);
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard received amount=10 kind=text
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard sent amount=10 kind=text
          clipboard: clipboard not delivered direction=to the controller kind=text refused=no session
          clipboard: clipboard sent amount=11 kind=text
        other-controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard received amount=11 kind=text
    "#]].assert_eq(&trace.tell(&["clipboard"]).only(super::CROSSINGS).to_string());
    assert_unrecorded(&trace, &["from-first", "from-second", "stale-text"]);
    Ok(())
}
