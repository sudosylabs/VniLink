//! The shared clipboard alone: text and file selections copied on either
//! computer reaching the other, over a control session or through the
//! Remote Desktop service, under the host's grant.

#[path = "clipboard/desktops.rs"]
mod desktops;
#[path = "clipboard/dropped.rs"]
mod dropped;
#[path = "clipboard/files.rs"]
mod files;
#[path = "clipboard/text.rs"]
mod text;

use std::sync::{Arc, Mutex};

use expect_test::expect;

use vnilink::{Error, Grants, MAX_CLIPBOARD_TEXT};
use vnilink_testkit::desktop::{DesktopSession, RecordingClipboard, Script, ScriptedScreen};
use vnilink_testkit::{traced, wait_for, Engine, Pair, DEFAULT_WAIT};

/// What the host shares: it may be seen and controlled, and its clipboard
/// shared when `clipboard`.
fn sharing(clipboard: bool) -> Grants {
    Grants {
        view: true,
        control: true,
        clipboard,
        transfer: false,
    }
}

/// What a controller shares: nothing.
const NOTHING: Grants = Grants {
    view: false,
    control: false,
    clipboard: false,
    transfer: false,
};

/// A host and a controller paired, the host sharing its clipboard when
/// `clipboard`.
fn paired(clipboard: bool) -> Pair {
    Pair::new(sharing(clipboard), NOTHING)
}

/// The clipboard's records of what crossed and what was refused, without
/// how its stream ended: a connection that closes or is revoked ends the
/// stream on each side at moments of its own, told or not before a story
/// is read.
const CROSSINGS: &[&str] = &[
    "clipboard adapter installed",
    "clipboard stream opened",
    "clipboard sent",
    "clipboard answered",
    "clipboard received",
    "clipboard not delivered",
];

/// The text a host's clipboard was given, in order.
#[derive(Clone)]
struct Copied(Arc<Mutex<Vec<String>>>);

impl Copied {
    /// Installs a clipboard on `host` that records the text it is given.
    fn install(host: &Engine) -> Result<Self, Error> {
        let texts = Arc::new(Mutex::new(Vec::new()));
        host.core.set_clipboard(Box::new(RecordingClipboard {
            texts: Arc::clone(&texts),
        }))?;
        Ok(Self(texts))
    }

    fn texts(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }
}

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

/// `story` with each engine's lines sorted, for a scenario whose work on
/// both computers at once is recorded in either order.
fn unordered(story: &str) -> String {
    let mut told = String::new();
    let mut lines: Vec<&str> = Vec::new();
    for line in story.lines() {
        if line.starts_with(' ') {
            lines.push(line);
            continue;
        }
        lines.sort_unstable();
        for line in lines.drain(..) {
            told.push_str(line);
            told.push('\n');
        }
        told.push_str(line);
        told.push('\n');
    }
    lines.sort_unstable();
    for line in lines {
        told.push_str(line);
        told.push('\n');
    }
    told
}

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

    expect![[r#"
        controller:
          clipboard: clipboard sent bytes=28 service=1
          clipboard: clipboard received bytes=13 hosted=false service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=28 hosted=true service=1
          clipboard: clipboard sent bytes=13 service=1
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    trace.assert_unrecorded("copied content", &["horse", "from the host"]);
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
    session.share_clipboard(true);
    session.service.submit_clipboard("granted").unwrap();
    assert_eq!(
        texts.lock().unwrap().last().map(String::as_str),
        Some("granted")
    );
    expect![[r#"
        controller:
          clipboard: clipboard sent bytes=6 ended=clipboard denied service=1
          clipboard: clipboard sent bytes=7 service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=6 hosted=true refused=clipboard denied service=1
          clipboard: clipboard received bytes=7 hosted=true service=1
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
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

    expect![[r#"
        controller:
          clipboard: clipboard stream opened dial=true
          clipboard: clipboard sent amount=1 kind=files
          clipboard: clipboard answered amount=1 kind=files
        host:
          clipboard: clipboard received amount=1 kind=files
    "#]]
    .assert_eq(&trace.story(&["clipboard"]));
    trace.assert_unrecorded("a copied file's name", &["quarterly"]);
}
