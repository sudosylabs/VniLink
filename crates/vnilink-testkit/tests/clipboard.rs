//! The shared clipboard alone: text copied on either computer reaching the
//! other through the Remote Desktop service, under the host's grant.

#[path = "clipboard/desktops.rs"]
mod desktops;

use std::sync::{Arc, Mutex};

use expect_test::expect;

use vnilink::{Error, Grants, MAX_CLIPBOARD_TEXT};
use vnilink_testkit::desktop::{DesktopSession, RecordingClipboard, Script, ScriptedScreen};
use vnilink_testkit::{traced, Engine};

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
