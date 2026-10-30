// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Services while their connection is admitted: attached before the host
//! approves, refused with it, and on a neutral services connection that
//! grants nothing until the host shares each service.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CloseCause, Error, Grants, ServiceKind, ServicePhase, SessionPhase, SharingPolicy,
    VideoCodecCaps,
};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::phase;

/// The host forgets the controller and asks its owner before admitting it
/// again, as a pairing file with the control switch does.
fn ask_before_admitting(pair: &Pair) -> TestResult {
    let host = &pair.host.core;
    host.unpair(&pair.controller.endpoint())?;
    host.export_pairing_file(Duration::from_secs(60), true)?;
    Ok(())
}

/// Services attached while the host's owner decides wait; closing one does
/// not withdraw the request, and once approved the closed one stays closed
/// while the others, and new ones, open under the admitted generation.
#[test]
fn a_waiting_service_closes_without_withdrawing_the_request() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    ask_before_admitting(&pair)?;
    let pending = controller.begin_connect_services(&pair.host.endpoint())?;
    pair.host.wait_request();
    let service = pair.attach(&pending, ServiceKind::Desktop);
    assert_eq!(
        service.phase,
        ServicePhase::Waiting,
        "a service attached before admission waits"
    );
    let files = pair.attach(&pending, ServiceKind::Files);
    assert!(
        matches!(
            controller.service_lease(service.id, service.generation),
            Err(Error::NoSession)
        ),
        "a waiting service has no lease"
    );
    controller.close_controller_service(service.id, service.generation)?;
    assert!(
        host.control_request()?.is_some(),
        "closing a waiting service leaves the request"
    );
    host.confirm_control_request(&controller.control_check())?;
    let session = pair.controller.wait_phase(pending.id, SessionPhase::Active);
    assert_eq!(
        phase(&pair.controller, &service),
        ServicePhase::Closed(CloseCause::OwnerClose),
        "the closed service stays closed after admission"
    );
    let waiting = pair.attach(&session, ServiceKind::Desktop);
    assert_eq!(
        waiting.phase,
        ServicePhase::Active,
        "a service attached after admission is active"
    );
    let files = controller.controller_service(files.id)?;
    assert_eq!(
        files.phase,
        ServicePhase::Active,
        "a waiting sibling opens on admission"
    );
    assert_eq!(
        files.generation, session.generation,
        "under the admitted generation"
    );
    assert!(
        !controller
            .service_lease(files.id, files.generation)?
            .is_cancelled(),
        "its lease is live"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=waiting generation=0 kind=Desktop service=1 to=closed: OwnerClose
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          service: service phase from=waiting generation=1 kind=Files service=2 to=active
    "#]],
        || trace.story(&["service", "session"]),
    );
    Ok(())
}

/// A host's owner rejecting the connection fails the services attached to
/// it; they give no lease and can be forgotten with it.
#[test]
fn a_rejected_connection_fails_its_attached_services() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    ask_before_admitting(&pair)?;
    let pending = controller.begin_connect_services(&pair.host.endpoint())?;
    pair.host.wait_request();
    let service = pair.attach(&pending, ServiceKind::Desktop);
    host.reject_control_request()?;
    wait_for("the connection to fail", DEFAULT_WAIT, || {
        matches!(
            controller.controller_session(pending.id).ok()?.phase,
            SessionPhase::Failed(_)
        )
        .then_some(())
    });
    assert!(
        matches!(phase(&pair.controller, &service), ServicePhase::Failed(_)),
        "a rejected connection fails its services"
    );
    assert!(
        matches!(
            controller.service_lease(service.id, service.generation),
            Err(Error::NoSession)
        ),
        "a failed service has no lease"
    );
    controller.forget_controller_session(pending.id)?;
    controller.forget_controller_service(service.id)?;
    assert!(
        controller.controller_services()?.is_empty(),
        "the failed service is forgotten"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          session: session failed failure=unauthorized session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
          service: service phase from=waiting generation=0 kind=Desktop service=1 to=failed
    "#]],
        || trace.story(&["service", "session"]),
    );
    Ok(())
}

/// A services connection is admitted without any desktop grant or codec:
/// it opens no Desktop, and Files opens only once the
/// host shares it, while the connection's own file calls stay refused; a
/// redial moves the owner on, and withdrawing Files ends it and leaves the
/// connection.
#[test]
fn a_neutral_connection_grants_nothing_until_files_is_shared() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let here = pair.host.location();
    let (root, folder) = (here.root, here.path(""));
    host.set_desktop_sharing(Grants::default())?;
    host.set_video_codecs(VideoCodecCaps::default())?;
    controller.set_video_codecs(VideoCodecCaps::default())?;
    let session = pair.connect_services();
    let desktop = pair.attach(&session, ServiceKind::Desktop);
    assert!(
        matches!(
            controller.open_desktop_service(desktop.id, desktop.generation),
            Err(Error::Unsupported(_))
        ),
        "a neutral connection opens no Desktop"
    );
    let owner = pair.attach(&session, ServiceKind::Files);
    assert!(
        controller
            .remote_service_support(session.id, session.generation)?
            .files,
        "the host supports Files"
    );
    assert!(
        matches!(
            controller.open_files_service(owner.id, owner.generation),
            Err(Error::TransferDenied)
        ),
        "Files needs the host's consent"
    );
    assert!(
        pair.host.hosted(None).is_empty(),
        "the refused Files is not served"
    );
    host.set_service_sharing(ServiceKind::Files, true)?;
    let pipe = controller.open_files_service(owner.id, owner.generation)?;
    pipe.mkdir(root, &folder, "owned")?;
    assert_eq!(
        pipe.list(root, &folder, 0, 20)?.entries[0].name,
        "owned",
        "the shared Files changes the host's disk"
    );
    let old_lease = controller.service_lease(owner.id, owner.generation)?;
    controller.drop_controller_link(session.id)?;
    pair.controller.redialed(&session);
    assert!(old_lease.is_cancelled(), "the redial ends the old lease");
    let fresh = controller.controller_service(owner.id)?;
    let next = controller.open_files_service(fresh.id, fresh.generation)?;
    pipe.close();
    assert_eq!(
        next.list(root, &folder, 0, 20)?.entries[0].name,
        "owned",
        "the owner's next pipe works after the old one closed"
    );
    host.set_service_sharing(ServiceKind::Files, false)?;
    wait_for("withdrawing Files to end the pipe", DEFAULT_WAIT, || {
        next.is_cancelled().then_some(())
    });
    trace.wait_for_records("service released", 2);
    assert_eq!(
        controller.controller_session(session.id)?.phase,
        SessionPhase::Active,
        "withdrawing Files leaves the connection"
    );
    assert_eq!(
        trace.hosted_ends("host"),
        [
            (1, "closed: ParentLoss".to_owned()),
            (2, "closed: Revoked".to_owned())
        ],
        "the host ends the old generation's Files as its lost parent, and the next as revoked"
    );
    // The host names the lost parent on either of two paths, at a moment of
    // its own, and the redial suspends and resumes the controller's services
    // together, in no order.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=suspended
          service: service phase from=active generation=1 kind=Files service=2 to=suspended
          service: service phase from=active generation=3 kind=Files service=2 to=closed: PeerClose
          service: service phase from=suspended generation=3 kind=Desktop service=1 to=active
          service: service phase from=suspended generation=3 kind=Files service=2 to=active
    "#]],
        || {
            trace
                .tell(&["service"])
                .of("controller")
                .concurrent()
                .to_string()
        },
    );
    Ok(())
}

/// A controller the host does not yet trust asks for a services
/// connection; the host's owner approves it, which pairs them and shares
/// nothing.
#[test]
fn a_neutral_connection_needs_the_hosts_consent_and_shares_nothing() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    host.core.start_hosting()?;
    // Only the controller knows the host; the host has not accepted it.
    controller
        .core
        .accept_invitation(&host.core.create_invitation(Duration::from_secs(60))?)?;
    host.core
        .export_pairing_file(Duration::from_secs(60), true)?;
    let events = host.core.subscribe()?;
    let pending = controller.core.begin_connect_services(&host.endpoint())?;
    host.wait_request();
    host.core
        .confirm_control_request(&controller.core.control_check())?;
    controller.wait_phase(pending.id, SessionPhase::Active);
    assert_eq!(
        host.core.snapshot()?.devices.len(),
        1,
        "approving pairs the controller"
    );
    assert_eq!(
        host.core.sharing_policy()?,
        SharingPolicy::default(),
        "approving shares nothing"
    );
    assert!(
        events.try_recv().is_some(),
        "the host publishes what changed"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation accepted
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request answered
    "#]],
        || trace.story(&["pairing", "session"]),
    );
    Ok(())
}
