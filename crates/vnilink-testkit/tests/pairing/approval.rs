// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A host's owner approving the controller that opened its pairing file:
//! by the controller's digits, only while the file allows it and is
//! unexpired, and only once; and what a second invitation or the host's
//! own sharing leaves of what was granted.

use expect_test::expect;
use std::time::{Duration, Instant};

use vnilink::{CoreEvent, Error, Grants, ServiceKind, SessionPhase};
use vnilink_testkit::events::drain;
use vnilink_testkit::session::wait_request;
use vnilink_testkit::{holds, traced, wait_for, Engine, TestResult};

use super::support::exchange;

/// How long the host takes to ask its owner, at most.
const ASKING: Duration = Duration::from_secs(5);

/// The controller keeps waiting while the host's owner reads the digits,
/// longer than an ordinary frame read takes, and its link comes up once
/// approved.
#[test]
fn the_controller_waits_while_the_host_owner_types_the_digits() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    host.core.start_hosting()?;
    let file = host
        .core
        .export_pairing_file(Duration::from_secs(300), true)?;
    let preview = controller.core.open_pairing_file(&file.bytes)?;
    controller.core.confirm_pairing(&preview, &file.check)?;
    let pending = controller.core.begin_connect_services(&host.endpoint())?;
    let request = wait_request(&host.core, ASKING);
    // Longer than an ordinary frame read: a person is reading digits aloud.
    holds(
        "the host asking about the controller",
        Duration::from_secs(12),
        || {
            host.core
                .control_request()
                .ok()
                .flatten()
                .is_some_and(|current| current.endpoint_id == request.endpoint_id)
        },
    );
    host.core
        .confirm_control_request(&controller.core.control_check())?;
    wait_for(
        "the approved link to come up",
        Duration::from_secs(8),
        || {
            (controller
                .core
                .controller_session(pending.id)
                .expect("the controller's session")
                .phase
                == SessionPhase::Active)
                .then_some(())
        },
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=300
          pairing: control request answered
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// Digits that do not match the controller's are refused: the controller
/// is turned away and the host remembers nobody, while the controller
/// still remembers the host and a later request is approved.
#[test]
fn the_host_rejects_a_control_request_whose_digits_do_not_match() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    exchange(&host.core, &controller.core, true);
    host.core.start_hosting()?;
    let denied = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let request = wait_request(&host.core, ASKING);
        let wrong = if request.check == "000" { "001" } else { "000" };
        assert!(
            matches!(
                host.core.confirm_control_request(wrong),
                Err(Error::PairingCheckMismatch)
            ),
            "wrong digits are refused"
        );
        connecting.join().expect("the connecting thread")
    });
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "a mismatched request turns the controller away: {denied:?}"
    );
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "the host remembers nobody"
    );
    assert_eq!(
        controller.core.snapshot()?.devices.len(),
        1,
        "the controller still remembers the host"
    );

    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let request = wait_request(&host.core, ASKING);
        host.core
            .confirm_control_request(&request.check)
            .expect("the approval");
        connecting.join().expect("the connecting thread")
    })?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "the right digits approve the controller"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request answered refused=pairing check mismatch
          pairing: control request answered
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A file made without the control switch never has the host ask its
/// owner: the controller is turned away and nobody is remembered.
#[test]
fn a_file_without_the_control_switch_does_not_ask_the_host() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    exchange(&host.core, &controller.core, false);
    host.core.start_hosting()?;
    let events = host.core.subscribe()?;
    let denied = controller.connect_services(&host);
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "the controller is turned away: {denied:?}"
    );
    assert!(
        host.core.control_request()?.is_none(),
        "the host asks nothing"
    );
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "the host remembers nobody"
    );
    assert!(
        !drain(&events)
            .iter()
            .any(|event| matches!(event, CoreEvent::ControlRequested { .. })),
        "the host asked for digits with the switch off"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=false direct=1 relays=0 ttl_s=60
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// The owner rejecting a request turns the controller away and answers it;
/// the file still lets the controller ask again and be approved.
#[test]
fn rejecting_the_control_request_leaves_the_file_usable() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    exchange(&host.core, &controller.core, true);
    host.core.start_hosting()?;
    let denied = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let _request = wait_request(&host.core, ASKING);
        host.core.reject_control_request().expect("the rejection");
        connecting.join().expect("the connecting thread")
    });
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "a rejected controller is turned away: {denied:?}"
    );
    assert!(
        matches!(
            host.core.reject_control_request(),
            Err(Error::NoControlRequest)
        ),
        "a rejected request is answered"
    );
    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let request = wait_request(&host.core, ASKING);
        host.core
            .confirm_control_request(&controller.core.control_check())
            .expect("the approval");
        assert_eq!(
            request.check,
            controller.core.control_check(),
            "the host asks by the controller's digits again"
        );
        connecting.join().expect("the connecting thread")
    })?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "the file still approves the controller"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request rejected
          pairing: control request answered
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// Approving after the file's lifetime is refused as expired: the
/// controller is turned away and the request is gone.
#[test]
fn approving_control_after_the_window_expires_is_refused() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host
        .core
        .export_pairing_file(Duration::from_secs(2), true)?;
    let preview = controller.core.open_pairing_file(&file.bytes)?;
    controller.core.confirm_pairing(&preview, &file.check)?;
    host.core.start_hosting()?;
    std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let request = wait_request(&host.core, ASKING);
        // The owner answers three seconds after being asked, past the
        // file's two.
        let asked = Instant::now();
        let answered = Duration::from_secs(3);
        wait_for("the owner to answer late", answered * 2, || {
            (asked.elapsed() >= answered).then_some(())
        });
        assert!(
            matches!(
                host.core.confirm_control_request(&request.check),
                Err(Error::InvitationExpired)
            ),
            "a late approval is refused as expired"
        );
        assert!(
            matches!(
                connecting.join().expect("the connecting thread"),
                Err(Error::Unauthorized)
            ),
            "the controller is turned away"
        );
    });
    assert!(
        host.core.control_request()?.is_none(),
        "an expired request is gone"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=2
          pairing: control request answered refused=invitation expired
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// Accepting a second invitation from a known host keeps what the
/// controller had granted it since.
#[test]
fn accepting_a_second_invitation_keeps_authorized_grants() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let first = host.core.create_invitation(Duration::from_secs(60))?;
    let saved = controller.core.accept_invitation(&first)?;
    assert!(!saved.grants.clipboard, "pairing grants no clipboard");
    controller.core.set_desktop_sharing(Grants {
        view: false,
        control: false,
        clipboard: true,
        transfer: true,
    })?;
    let second = host.core.create_invitation(Duration::from_secs(60))?;
    let kept = controller.core.accept_invitation(&second)?;
    assert!(!kept.grants.control, "no control was granted");
    assert!(kept.grants.clipboard, "the clipboard grant is kept");
    assert!(kept.grants.transfer, "the transfer grant is kept");
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation accepted
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation issued direct=1 relays=0 ttl_s=60
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A controller the host already remembers is never asked about again: its
/// desktop is refused by what the host shares, without a request, while
/// what the host shares still lets it connect.
#[test]
fn remembered_peer_denied_by_global_policy_never_prompts_again() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    // The host pairs the controller first so it can authorize side grants.
    exchange(&controller.core, &host.core, false);
    host.core.set_desktop_sharing(Grants {
        view: false,
        control: false,
        clipboard: true,
        transfer: true,
    })?;

    exchange(&host.core, &controller.core, true);
    host.core.start_hosting()?;
    let session = controller.connect_services(&host)?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "the host's sharing admits a services connection"
    );
    let owner = controller.core.attach_controller_service(
        session.id,
        session.generation,
        ServiceKind::Desktop,
    )?;
    assert!(
        matches!(
            controller
                .core
                .open_desktop_service(owner.id, owner.generation),
            Err(Error::Unauthorized)
        ),
        "a desktop the host does not share is refused"
    );
    assert!(
        host.core.control_request()?.is_none(),
        "a remembered controller is not asked about"
    );
    let policy = host.core.sharing_policy()?;
    assert!(policy.clipboard, "the host still shares the clipboard");
    assert!(!policy.view, "the host still shares no view");
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file exported allow_control=false direct=1 relays=0 ttl_s=60
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file opened bytes=296
          pairing: invitation accepted
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}
