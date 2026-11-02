// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The shared clipboard alone: text copied on either computer reaching the
//! other through the Remote Desktop service, under the host's grant.

#[path = "clipboard/desktops.rs"]
mod desktops;

use expect_test::expect;

use vnilink::{Error, Grants, MAX_CLIPBOARD_TEXT};
use vnilink_testkit::desktop::{Copied, DesktopSession, Script, ScriptedScreen};
use vnilink_testkit::{traced, TestResult};

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

/// A viewer's Desktop on the host's scripted screen, the host's clipboard
/// recording what it is given, installed before the viewer connects.
fn session() -> Result<(DesktopSession, Copied), Box<dyn std::error::Error>> {
    let mut installed = None;
    let session = DesktopSession::prepared(ScriptedScreen::new(Script::Still), false, |pair| {
        installed = Some(Copied::install(&pair.host.core));
    });
    let copied = installed.ok_or("the session prepares its host")??;
    Ok((session, copied))
}

/// Text copied on the controller reaches the host's clipboard, and text
/// copied on the host reaches the controller; what is recorded says how
/// much and how it went, never the text itself.
#[test]
fn text_crosses_both_ways_and_is_never_recorded() -> TestResult {
    let trace = traced();
    let (session, copied) = session()?;
    session.share_clipboard(true);
    let secret = "correct horse battery staple";
    session.service.submit_clipboard(secret)?;
    assert_eq!(
        copied.texts(),
        [secret],
        "the controller's text reaches the host's clipboard"
    );
    let (hosted, generation) = session.hosted();
    session
        .pair
        .host
        .core
        .publish_desktop_clipboard(hosted, generation, "from the host")?;
    assert_eq!(
        session.service.clipboard_text()?.as_deref(),
        Some("from the host"),
        "the host's text reaches the controller"
    );

    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard sent bytes=28 service=1
          clipboard: clipboard received bytes=13 hosted=false service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=28 hosted=true service=1
          clipboard: clipboard sent bytes=13 service=1
    "#]],
        || trace.story(&["clipboard"]),
    );
    trace.assert_unrecorded("copied content", &["horse", "from the host"]);
    Ok(())
}

/// Without the grant nothing crosses either way, the host records why,
/// and the Desktop service carries on; granting it again works at once.
#[test]
fn without_the_grant_nothing_crosses_and_the_desktop_lives() -> TestResult {
    let trace = traced();
    let (session, copied) = session()?;
    assert!(
        matches!(
            session.service.submit_clipboard("denied"),
            Err(Error::ClipboardDenied)
        ),
        "a paste without the grant is refused"
    );
    let (hosted, generation) = session.hosted();
    assert!(
        matches!(
            session
                .pair
                .host
                .core
                .publish_desktop_clipboard(hosted, generation, "denied"),
            Err(Error::ClipboardDenied)
        ),
        "the host publishes nothing without the grant"
    );
    assert!(copied.texts().is_empty(), "a refused paste is not applied");
    assert!(
        !session.service.is_cancelled(),
        "a refused paste leaves the desktop open"
    );
    session.share_clipboard(true);
    session.service.submit_clipboard("granted")?;
    assert_eq!(
        copied.texts().last().map(String::as_str),
        Some("granted"),
        "once granted, text crosses at once"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard sent bytes=6 ended=clipboard denied service=1
          clipboard: clipboard sent bytes=7 service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=6 hosted=true refused=clipboard denied service=1
          clipboard: clipboard received bytes=7 hosted=true service=1
    "#]],
        || trace.story(&["clipboard"]),
    );
    Ok(())
}

/// The largest text crosses whole; one byte more is refused before it is
/// sent.
#[test]
fn the_largest_text_crosses_and_more_is_refused() -> TestResult {
    let trace = traced();
    let (session, copied) = session()?;
    session.share_clipboard(true);
    let largest = "é".repeat(MAX_CLIPBOARD_TEXT / 2);
    session.service.submit_clipboard(&largest)?;
    assert_eq!(
        copied.texts().last(),
        Some(&largest),
        "the largest text crosses whole"
    );
    assert!(
        session.service.submit_clipboard(&(largest + "a")).is_err(),
        "one byte more than the largest text is refused"
    );
    assert_eq!(
        copied.texts().len(),
        1,
        "the refused text never reaches the host"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard sent bytes=262144 service=1
          clipboard: clipboard sent bytes=262145 ended=invalid setting service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=262144 hosted=true service=1
    "#]],
        || trace.story(&["clipboard"]),
    );
    Ok(())
}
