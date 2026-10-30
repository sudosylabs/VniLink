// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Text copied through a Remote Desktop service: each desktop has its own
//! clipboard pipe, acknowledged both ways, fenced from the desktops before
//! and beside it, and a host's slow clipboard holds up neither the engine
//! nor the desktop's pictures.
//!
//! These scenarios open desktops by hand on a services connection they
//! keep, so they can reopen one on it and see the connection unchanged.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use expect_test::expect;

use super::{sharing, Copied};
use vnilink::{CoreEvent, Error, HostClipboard, ServiceKind, SessionInfo};
use vnilink_testkit::desktop::{keyframe, next_picture, InputLog};
use vnilink_testkit::{traced, wait_for, Engine, TestResult};

/// The controller's services connection to `host`, whose desktop logs the
/// input it applies; the host does not share its clipboard yet.
fn connected(host: &Engine, controller: &Engine) -> Result<SessionInfo, Error> {
    InputLog::install(host, (1920, 1080))?;
    host.pair_for_control(controller)?;
    controller.connect_services(host)
}

/// A host clipboard that blocks in each paste until released, so a test
/// can hold the host's side mid-paste.
struct Blocking {
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
    applied: Arc<Mutex<Vec<String>>>,
}

impl HostClipboard for Blocking {
    fn set_text(&mut self, text: &str) -> Result<(), Error> {
        self.entered.send(()).map_err(|_| Error::NoSession)?;
        self.release.recv().map_err(|_| Error::NoSession)?;
        self.applied
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(text.into());
        Ok(())
    }
}

/// The test's hold on a [`Blocking`] clipboard.
struct Stuck {
    /// Says a paste entered the clipboard.
    entry: mpsc::Receiver<()>,
    /// Lets the paste in it finish.
    release: mpsc::Sender<()>,
    /// What it applied.
    applied: Arc<Mutex<Vec<String>>>,
}

/// Installs a [`Blocking`] clipboard on `host`.
fn install_blocking(host: &Engine) -> Result<Stuck, Error> {
    let (entered, entry) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.core.set_clipboard(Box::new(Blocking {
        entered,
        release: released,
        applied: Arc::clone(&applied),
    }))?;
    Ok(Stuck {
        entry,
        release,
        applied,
    })
}

/// Before the grant a desktop's text crosses neither way. Once given, text
/// copied on both computers at once crosses both ways, each acknowledged,
/// and empty text clears each side; the controller is told text arrived
/// for its desktop, never what it says.
#[test]
fn text_copied_on_both_computers_at_once_crosses_both_ways() -> TestResult {
    let trace = traced();
    let (host, controller) = (Engine::start("host"), Engine::start("controller"));
    let connection = connected(&host, &controller)?;
    let copied = Copied::install(&host)?;
    let desktop = controller.open_desktop(&connection)?;
    let remote = host
        .hosted_for(ServiceKind::Desktop, &controller)
        .expect("the host serves the desktop once it opened");
    let (host, controller) = (&host.core, &controller.core);
    let info = desktop.info();
    assert_eq!(
        desktop.clipboard_text()?,
        None,
        "a new desktop holds no text"
    );
    assert!(
        matches!(
            desktop.submit_clipboard("secret-denied"),
            Err(Error::ClipboardDenied)
        ),
        "a paste without the grant is refused"
    );
    assert!(
        matches!(
            host.publish_desktop_clipboard(remote.id, remote.generation, "secret-denied"),
            Err(Error::ClipboardDenied)
        ),
        "the host publishes nothing without the grant"
    );
    assert!(copied.texts().is_empty(), "a refused paste is not applied");

    host.set_desktop_sharing(sharing(true))?;
    let events = controller.subscribe()?;
    std::thread::scope(|scope| -> Result<(), Error> {
        let outgoing = scope.spawn(|| {
            controller.submit_controller_desktop_clipboard(
                info.id,
                info.generation,
                "controller copy",
            )
        });
        let incoming = scope
            .spawn(|| host.publish_desktop_clipboard(remote.id, remote.generation, "host copy"));
        outgoing.join().expect("the paste did not panic")?;
        incoming.join().expect("the publication did not panic")
    })?;
    assert_eq!(
        copied.texts(),
        ["controller copy"],
        "the controller's text reached the host once acknowledged"
    );
    assert_eq!(
        controller
            .controller_desktop_clipboard_text(info.id, info.generation)?
            .as_deref(),
        Some("host copy"),
        "the host's text reached the controller once acknowledged"
    );
    // Presence may be told meanwhile; the clipboard's is the event asked of.
    let service = wait_for(
        "the controller to be told the host's text arrived",
        Duration::from_secs(5),
        || {
            let event = events.recv_timeout(Duration::from_millis(20))?;
            assert!(
                !format!("{event:?}").contains("host copy"),
                "an event carries the host's text: {event:?}"
            );
            match event {
                CoreEvent::DesktopClipboardReceived { service } => Some(service),
                _ => None,
            }
        },
    );
    assert!(
        service.id == info.id && service.generation == info.generation,
        "the controller is told text arrived for this desktop: {service:?}"
    );

    desktop.submit_clipboard("")?;
    assert_eq!(
        copied.texts().last().map(String::as_str),
        Some(""),
        "empty text clears the host's clipboard"
    );
    host.publish_desktop_clipboard(remote.id, remote.generation, "")?;
    assert_eq!(
        desktop.clipboard_text()?.as_deref(),
        Some(""),
        "empty text clears the controller's"
    );
    trace.wait_for_records("clipboard received", 5);
    trace.wait_for_records("clipboard sent", 5);
    // The two crossings at once are told in either order on each side.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard received bytes=0 hosted=false service=1
          clipboard: clipboard received bytes=9 hosted=false service=1
          clipboard: clipboard sent bytes=0 service=1
          clipboard: clipboard sent bytes=13 ended=clipboard denied service=1
          clipboard: clipboard sent bytes=15 service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=0 hosted=true service=1
          clipboard: clipboard received bytes=13 hosted=true refused=clipboard denied service=1
          clipboard: clipboard received bytes=15 hosted=true service=1
          clipboard: clipboard sent bytes=0 service=1
          clipboard: clipboard sent bytes=9 service=1
    "#]],
        || super::unordered(&trace.story(&["clipboard"])),
    );
    trace.assert_unrecorded(
        "copied content",
        &["secret-denied", "controller copy", "host copy"],
    );
    Ok(())
}

/// Withdrawing the grant mid-desktop refuses the next paste, and the
/// desktop and its connection carry on; granting it again lets text cross
/// at once.
#[test]
fn withdrawing_the_grant_refuses_text_and_the_desktop_lives() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let connection = connected(&host, &controller)?;
    let copied = Copied::install(&host)?;
    host.core.set_desktop_sharing(sharing(true))?;
    let desktop = controller.open_desktop(&connection)?;
    desktop.submit_clipboard("granted")?;
    host.core.set_desktop_sharing(sharing(false))?;
    assert!(
        matches!(
            desktop.submit_clipboard("revoked"),
            Err(Error::ClipboardDenied)
        ),
        "a paste after the grant is withdrawn is refused"
    );
    assert!(
        !desktop.is_cancelled(),
        "withdrawing the clipboard leaves the desktop open"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?,
        connection,
        "withdrawing the clipboard leaves the connection as it was"
    );
    host.core.set_desktop_sharing(sharing(true))?;
    desktop.submit_clipboard("restored")?;
    assert_eq!(
        copied.texts(),
        ["granted", "restored"],
        "only granted pastes reach the host"
    );
    trace.wait_for_records("clipboard received", 3);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard sent bytes=7 service=1
          clipboard: clipboard sent bytes=7 ended=clipboard denied service=1
          clipboard: clipboard sent bytes=8 service=1
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard received bytes=7 hosted=true service=1
          clipboard: clipboard received bytes=7 hosted=true refused=clipboard denied service=1
          clipboard: clipboard received bytes=8 hosted=true service=1
    "#]],
        || trace.story(&["clipboard"]),
    );
    trace.assert_unrecorded("copied content", &["granted", "revoked", "restored"]);
    Ok(())
}

/// A desktop reopened on the same connection starts with no text, and the
/// closed one is fenced: neither side can read or send through it, and
/// closing it again leaves the new one working.
#[test]
fn a_reopened_desktop_starts_without_text_and_the_closed_one_is_fenced() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let connection = connected(&host, &controller)?;
    let copied = Copied::install(&host)?;
    host.core.set_desktop_sharing(sharing(true))?;
    let desktop = controller.open_desktop(&connection)?;
    let info = desktop.info();
    let remote = host
        .hosted_for(ServiceKind::Desktop, &controller)
        .expect("the host serves the desktop once it opened");
    host.core
        .publish_desktop_clipboard(remote.id, remote.generation, "host copy")?;
    assert_eq!(
        desktop.clipboard_text()?.as_deref(),
        Some("host copy"),
        "the first desktop holds the host's text"
    );
    desktop.close();
    host.wait_released(None);
    let fresh = controller.open_desktop(&connection)?;
    assert_eq!(
        fresh.clipboard_text()?,
        None,
        "a reopened desktop starts with no text"
    );
    assert!(
        desktop.clipboard_text().is_err(),
        "a closed desktop holds no text"
    );
    assert!(
        controller
            .core
            .controller_desktop_clipboard_text(info.id, info.generation)
            .is_err(),
        "a closed desktop's text is not read by its id"
    );
    assert!(
        controller
            .core
            .submit_controller_desktop_clipboard(info.id, info.generation, "late-controller-copy")
            .is_err(),
        "nothing is sent through a closed desktop by its id"
    );
    assert_eq!(
        fresh.clipboard_text()?,
        None,
        "the closed desktop's text does not reach the new one"
    );
    assert!(
        host.core
            .publish_desktop_clipboard(remote.id, remote.generation, "late-host-copy")
            .is_err(),
        "the host cannot publish to a closed desktop"
    );
    desktop.close();
    fresh.submit_clipboard("fresh")?;
    assert_eq!(
        copied.texts().last().map(String::as_str),
        Some("fresh"),
        "the reopened desktop's paste reaches the host"
    );
    trace.wait_for_records("clipboard received", 2);
    trace.wait_for_records("clipboard sent", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard received bytes=9 hosted=false service=1
          clipboard: clipboard sent bytes=5 service=2
        host:
          clipboard: clipboard adapter installed
          clipboard: clipboard sent bytes=9 service=1
          clipboard: clipboard received bytes=5 hosted=true service=2
    "#]],
        || trace.story(&["clipboard"]),
    );
    trace.assert_unrecorded(
        "copied content",
        &[
            "host copy",
            "late-controller-copy",
            "late-host-copy",
            "fresh",
        ],
    );
    Ok(())
}

/// While the host's clipboard is stuck in a paste, the host's engine still
/// answers, and closing the desktop ends the waiting paste; the connection
/// is left as it was.
#[test]
fn a_stuck_clipboard_holds_up_neither_the_engine_nor_closing_the_desktop() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let connection = connected(&host, &controller)?;
    let Stuck { entry, release, .. } = install_blocking(&host)?;
    host.core.set_desktop_sharing(sharing(true))?;
    let desktop = Arc::new(controller.open_desktop(&connection)?);
    let (done, result) = mpsc::channel();
    let request = {
        let desktop = Arc::clone(&desktop);
        std::thread::spawn(move || {
            done.send(desktop.submit_clipboard("blocked"))
                .expect("the test waits for the paste");
        })
    };
    entry.recv_timeout(Duration::from_secs(5))?;
    let (snapshot_done, snapshot_taken) = mpsc::channel();
    let snapshot = {
        let host = Arc::clone(&host.core);
        std::thread::spawn(move || {
            snapshot_done
                .send(host.snapshot())
                .expect("the test waits for the snapshot");
        })
    };
    let snapshot_result = snapshot_taken.recv_timeout(Duration::from_secs(1));
    desktop.close();
    let request_result = result.recv_timeout(Duration::from_secs(1));
    release.send(())?;
    request.join().expect("the paste did not panic");
    snapshot.join().expect("the snapshot did not panic");
    assert!(
        snapshot_result
            .expect("the host's snapshot answers within a second while its clipboard is stuck")
            .is_ok(),
        "the host's snapshot succeeds while its clipboard is stuck"
    );
    assert!(
        request_result
            .expect("closing the desktop ends its waiting paste within a second")
            .is_err(),
        "a paste ended by closing its desktop fails"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?,
        connection,
        "closing the desktop leaves the connection as it was"
    );
    Ok(())
}

/// A controller with desktops on two computers keeps each one's clipboard
/// apart: a paste one host cannot apply is refused without ending its
/// desktop, and closing one leaves the other's text crossing.
#[test]
fn each_computers_desktop_has_its_own_clipboard() -> TestResult {
    let trace = traced();
    let controller = Engine::start("controller");
    let hosts = [Engine::start("host"), Engine::start("other-host")];
    let mut desktops = Vec::new();
    let mut owners = Vec::new();
    for host in &hosts {
        let connection = connected(host, &controller)?;
        host.core.set_desktop_sharing(sharing(true))?;
        desktops.push(controller.open_desktop(&connection)?);
        owners.push(
            host.hosted_for(ServiceKind::Desktop, &controller)
                .expect("the host serves the desktop once it opened"),
        );
    }
    assert!(
        matches!(
            desktops[0].submit_clipboard("no adapter"),
            Err(Error::Unsupported(_))
        ),
        "a host without a clipboard refuses a paste"
    );
    assert!(
        !desktops[0].is_cancelled(),
        "a refused paste leaves the desktop open"
    );
    for ((host, owner), text) in hosts.iter().zip(&owners).zip(["one", "two"]) {
        host.core
            .publish_desktop_clipboard(owner.id, owner.generation, text)?;
    }
    assert_eq!(
        desktops[0].clipboard_text()?.as_deref(),
        Some("one"),
        "the first desktop holds its own computer's text"
    );
    assert_eq!(
        desktops[1].clipboard_text()?.as_deref(),
        Some("two"),
        "the second desktop holds its own computer's text"
    );
    desktops[0].close();
    hosts[1]
        .core
        .publish_desktop_clipboard(owners[1].id, owners[1].generation, "still active")?;
    assert_eq!(
        desktops[1].clipboard_text()?.as_deref(),
        Some("still active"),
        "closing one computer's desktop leaves the other's text crossing"
    );
    assert!(
        !desktops[1].is_cancelled(),
        "closing one computer's desktop leaves the other's open"
    );
    trace.wait_for_records("clipboard received", 4);
    trace.wait_for_records("clipboard sent", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          clipboard: clipboard sent bytes=10 ended=unsupported service=1
          clipboard: clipboard received bytes=3 hosted=false service=1
          clipboard: clipboard received bytes=3 hosted=false service=2
          clipboard: clipboard received bytes=12 hosted=false service=2
        host:
          clipboard: clipboard received bytes=10 hosted=true reason=no clipboard adapter is installed refused=unsupported service=1
          clipboard: clipboard sent bytes=3 service=1
        other-host:
          clipboard: clipboard sent bytes=3 service=1
          clipboard: clipboard sent bytes=12 service=1
    "#]],
        || trace.story(&["clipboard"]),
    );
    trace.assert_unrecorded("copied content", &["no adapter", "still active"]);
    Ok(())
}

/// A paste the host does not acknowledge in time fails as timed out, the
/// paste queued behind it fails too and is never applied, and the desktop
/// stays open with its pictures still arriving.
#[test]
fn an_unacknowledged_paste_times_out_with_the_one_behind_it_and_pictures_still_arrive() -> TestResult
{
    let _trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let connection = connected(&host, &controller)?;
    let Stuck {
        entry,
        release,
        applied,
    } = install_blocking(&host)?;
    host.core.set_desktop_sharing(sharing(true))?;
    let desktop = Arc::new(controller.open_desktop(&connection)?);
    let request = {
        let desktop = Arc::clone(&desktop);
        std::thread::spawn(move || desktop.submit_clipboard("in flight"))
    };
    entry.recv_timeout(Duration::from_secs(5))?;
    let queued = {
        let desktop = Arc::clone(&desktop);
        std::thread::spawn(move || desktop.submit_clipboard("queued"))
    };
    let in_flight = request.join().expect("the paste did not panic");
    let queued = queued.join().expect("the queued paste did not panic");
    // The pipe ends after the caller stops waiting; wait for that, not
    // merely the caller's timeout.
    wait_for("the clipboard pipe to end", Duration::from_secs(8), || {
        desktop.clipboard_text().is_err().then_some(())
    });
    release.send(())?;
    assert!(
        matches!(in_flight, Err(Error::Timeout)),
        "an unacknowledged paste times out: {in_flight:?}"
    );
    assert!(queued.is_err(), "the paste queued behind it fails too");
    let applied = wait_for(
        "the stuck paste to be applied",
        Duration::from_secs(8),
        || {
            let applied = applied
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            (applied.len() == 1).then_some(applied)
        },
    );
    assert_eq!(applied, ["in flight"], "the queued paste is never applied");
    assert!(
        !desktop.is_cancelled(),
        "a timed-out paste leaves the desktop open"
    );
    let remote = host
        .hosted_for(ServiceKind::Desktop, &controller)
        .expect("the host serves the desktop once it opened");
    let video = desktop.subscribe_video()?;
    host.core
        .publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![1]))?;
    assert_eq!(
        next_picture(&video, "the host's picture").payload,
        vec![1],
        "the desktop's pictures still arrive"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?,
        connection,
        "a timed-out paste leaves the connection as it was"
    );
    Ok(())
}
