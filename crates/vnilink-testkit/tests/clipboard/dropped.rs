//! A files-only session whose link drops: the controller dials again, and what
//! was copied on the lost generation is never replayed on the new one.

use expect_test::expect;
use std::time::Duration;

use super::{paired, Copied, CROSSINGS};
use vnilink::{ClipboardFileEntry, Core, CoreEvent, Error, FileSelection, SessionPhase};
use vnilink_testkit::{traced, wait_for, TestResult};

/// How long the controller has to notice the drop, and to dial again.
const NOTICED_WITHIN: Duration = Duration::from_secs(5);

/// Waits until the controller's session is no longer active.
fn wait_dropped(controller: &Core) {
    wait_for("the controller to notice the drop", NOTICED_WITHIN, || {
        controller
            .snapshot()
            .ok()?
            .outgoing
            .is_some_and(|outgoing| outgoing.phase != SessionPhase::Active)
            .then_some(())
    });
}

fn selection(path: &str, size: u64) -> FileSelection {
    FileSelection {
        entries: vec![ClipboardFileEntry {
            path: path.into(),
            size,
            dir: false,
        }],
    }
}

/// Text pasted before the link dropped reaches the host once; a paste on
/// the lost generation is refused, and the session dialled again starts
/// with nothing replayed.
#[test]
fn a_dropped_session_does_not_replay_clipboard_text() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let controller = &pair.controller.core;
    let copied = Copied::install(&pair.host)?;
    let session = super::files_session(&pair.controller, &pair.host)?;
    controller.submit_clipboard(session.id, session.generation, "one")?;
    controller.drop_link()?;
    wait_dropped(controller);
    assert!(
        matches!(
            controller.submit_clipboard(session.id, session.generation, "replayed"),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "a paste on the lost generation is refused"
    );
    let again = wait_for("the controller to dial again", NOTICED_WITHIN, || {
        controller.snapshot().ok()?.outgoing.filter(|outgoing| {
            outgoing.phase == SessionPhase::Active && outgoing.generation != session.generation
        })
    });
    controller.submit_clipboard(again.id, again.generation, "two")?;
    assert_eq!(
        copied.texts(),
        ["one", "two"],
        "each paste reaches the host once, and nothing is replayed"
    );
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=3 kind=text
          clipboard: clipboard answered amount=3 kind=text
          clipboard: clipboard not delivered direction=to the host kind=text refused=…
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=3 kind=text
          clipboard: clipboard answered amount=3 kind=text
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received amount=3 kind=text
          clipboard: clipboard received amount=3 kind=text
    "#]]
    .assert_eq(
        &trace
            .tell(&["clipboard"])
            .only(CROSSINGS)
            // The lost generation refuses as gone or as stale, by whether
            // the new one is up yet; the assert above allows either.
            .masking(&["refused"], |event| {
                event["message"] == "clipboard not delivered"
            })
            .to_string(),
    );
    Ok(())
}

/// A selection offered before the link dropped is not carried to the
/// session dialled again: the lost generation refuses a new one, the new
/// generation starts empty on both sides, and its first offer is the
/// host's first revision of it.
#[test]
fn a_dropped_session_does_not_replay_a_selection() -> TestResult {
    let trace = traced();
    let pair = paired(true);
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = super::files_session(&pair.controller, &pair.host)?;
    controller.submit_clipboard_files(session.id, session.generation, &selection("one.txt", 3))?;
    let events = controller.subscribe()?;
    controller.drop_link()?;
    wait_dropped(controller);
    wait_for(
        "the controller's session to change to active under a new generation",
        NOTICED_WITHIN,
        || {
            matches!(
                events.recv_timeout(Duration::from_millis(50)),
                Some(CoreEvent::SessionChanged {
                    generation,
                    phase: SessionPhase::Active,
                    ..
                }) if generation != session.generation
            )
            .then_some(())
        },
    );
    let again = controller
        .snapshot()?
        .outgoing
        .ok_or("the controller's session")?;
    assert_ne!(
        again.generation, session.generation,
        "the session is dialled again under a new generation"
    );
    assert!(
        matches!(
            controller.submit_clipboard_files(
                session.id,
                session.generation,
                &selection("replayed.txt", 1),
            ),
            Err(Error::NoSession | Error::StaleSession)
        ),
        "an offer on the lost generation is refused"
    );
    assert!(
        matches!(
            controller.session_clipboard_files(again.id, session.generation),
            Err(Error::StaleSession)
        ),
        "the lost generation's selection is stale"
    );
    let hosted = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert!(
        host.incoming_clipboard_files(hosted.id)?.is_none(),
        "the host holds no selection for the new generation"
    );
    controller.submit_clipboard_files(again.id, again.generation, &selection("two.txt", 3))?;
    assert_eq!(
        host.incoming_clipboard_files(hosted.id)?
            .ok_or("the new generation's selection")?
            .revision,
        1,
        "the new generation's first offer is its first revision"
    );
    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard not delivered direction=to the host kind=files refused=stale session
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(&trace.tell(&["clipboard"]).only(CROSSINGS).to_string());
    Ok(())
}
