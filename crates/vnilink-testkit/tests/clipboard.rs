//! The shared clipboard alone: text copied on either computer reaching the
//! other through the Remote Desktop service, under the host's grant.

use std::sync::{Arc, Mutex};

use vnilink::{Error, MAX_CLIPBOARD_TEXT};
use vnilink_testkit::desktop::{DesktopSession, RecordingClipboard, Script, ScriptedScreen};
use vnilink_testkit::traced;

fn session() -> (DesktopSession, Arc<Mutex<Vec<String>>>) {
    let texts = Arc::new(Mutex::new(Vec::new()));
    let session = DesktopSession::prepared(ScriptedScreen::new(Script::Still), false, |pair| {
        pair.host
            .core
            .set_clipboard(Box::new(RecordingClipboard {
                texts: Arc::clone(&texts),
            }))
            .unwrap();
    });
    (session, texts)
}

/// Text copied on the controller reaches the host's clipboard, and text
/// copied on the host reaches the controller; what is recorded says how
/// much and how it went, never the text itself.
#[test]
fn text_crosses_both_ways_and_is_never_recorded() {
    let trace = traced();
    let (session, texts) = session();
    session.share_clipboard(true);
    let secret = "correct horse battery staple";
    session.service.submit_clipboard(secret).unwrap();
    assert_eq!(*texts.lock().unwrap(), vec![secret]);
    let (hosted, generation) = session.hosted();
    session
        .pair
        .host
        .core
        .publish_desktop_clipboard(hosted, generation, "from the host")
        .unwrap();
    assert_eq!(
        session.service.clipboard_text().unwrap().as_deref(),
        Some("from the host")
    );

    let sent = trace.named("clipboard sent");
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert!(
        sent.iter().all(|event| event["ended"].is_null()),
        "{sent:?}"
    );
    assert_eq!(sent[0]["bytes"], secret.len());
    let everything = trace.events().join("\n");
    assert!(!everything.contains("horse"), "copied text was recorded");
    assert!(
        !everything.contains("from the host"),
        "copied text was recorded"
    );
}

/// Without the grant nothing crosses either way, the host records why,
/// and the Desktop service carries on; granting it again works at once.
#[test]
fn without_the_grant_nothing_crosses_and_the_desktop_lives() {
    let trace = traced();
    let (session, texts) = session();
    assert!(matches!(
        session.service.submit_clipboard("denied"),
        Err(Error::ClipboardDenied)
    ));
    let (hosted, generation) = session.hosted();
    assert!(matches!(
        session
            .pair
            .host
            .core
            .publish_desktop_clipboard(hosted, generation, "denied"),
        Err(Error::ClipboardDenied)
    ));
    assert!(texts.lock().unwrap().is_empty());
    assert!(!session.service.is_cancelled());
    let refused = trace.named("clipboard received");
    assert!(
        refused
            .iter()
            .any(|event| event["refused"] == "clipboard denied"),
        "{refused:?}"
    );
    session.share_clipboard(true);
    session.service.submit_clipboard("granted").unwrap();
    assert_eq!(
        texts.lock().unwrap().last().map(String::as_str),
        Some("granted")
    );
}

/// The largest text crosses whole; one byte more is refused before it is
/// sent.
#[test]
fn the_largest_text_crosses_and_more_is_refused() {
    let _trace = traced();
    let (session, texts) = session();
    session.share_clipboard(true);
    let largest = "é".repeat(MAX_CLIPBOARD_TEXT / 2);
    session.service.submit_clipboard(&largest).unwrap();
    assert_eq!(texts.lock().unwrap().last(), Some(&largest));
    assert!(session.service.submit_clipboard(&(largest + "a")).is_err());
    assert_eq!(texts.lock().unwrap().len(), 1);
}

/// A file selection offered on a control session reaches the host as names
/// and sizes, and the story is recorded on both sides, by the engine that
/// lived it, without a single name.
#[test]
fn a_file_selection_crosses_a_session_and_its_story_is_recorded() {
    let trace = traced();
    let share = vnilink::Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: false,
    };
    let none = vnilink::Grants {
        view: false,
        control: false,
        clipboard: false,
        transfer: false,
    };
    let pair = vnilink_testkit::Pair::new(share, none);
    let session = pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let offer = vnilink::FileSelection {
        entries: vec![vnilink::ClipboardFileEntry {
            path: "quarterly-report.pdf".into(),
            size: 12_000,
            dir: false,
        }],
    };
    pair.controller
        .core
        .submit_clipboard_files(session.id, session.generation, &offer)
        .unwrap();
    let hosted = pair.host.core.snapshot().unwrap().incoming.unwrap();
    let stored = pair
        .host
        .core
        .incoming_clipboard_files(hosted.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.selection, offer);

    let story = |message: &str| -> Vec<(String, String)> {
        trace
            .named(message)
            .iter()
            .filter(|event| event["kind"] == "files")
            .map(|event| {
                (
                    event["engine"].as_str().unwrap_or_default().to_owned(),
                    event["refused"].as_str().unwrap_or("delivered").to_owned(),
                )
            })
            .collect()
    };
    assert_eq!(
        story("clipboard sent"),
        [("controller".into(), "delivered".into())]
    );
    assert_eq!(
        story("clipboard received"),
        [("host".into(), "delivered".into())]
    );
    assert_eq!(
        story("clipboard answered"),
        [("controller".into(), "delivered".into())]
    );
    assert!(
        !trace.events().join("\n").contains("quarterly"),
        "a file's name was recorded"
    );
}
