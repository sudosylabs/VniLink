//! Services while their connection is admitted: attached before the host
//! approves, refused with it, and on a neutral services connection that
//! grants nothing until the host shares each service.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CloseCause, Error, Grants, InputEvent, ServiceKind, ServicePhase, SessionPhase, SharingPolicy,
    VideoCodecCaps,
};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

use super::support::{hosted_ends, phase, reaches, redialed, whole_disk};

/// The host forgets the controller and asks its owner before admitting it
/// again, as a pairing file with the control switch does.
fn ask_before_admitting(pair: &Pair) -> TestResult {
    let host = &pair.host.core;
    host.unpair(&pair.controller.endpoint())?;
    host.export_pairing_file(Duration::from_secs(60), true)?;
    Ok(())
}

/// Waits until the host's owner is asked to admit a controller.
fn wait_asked(host: &Engine) {
    wait_for("the host to ask its owner", DEFAULT_WAIT, || {
        host.core.control_request().ok().flatten().map(|_| ())
    });
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
    let pending = controller.begin_connect(&pair.host.endpoint())?;
    wait_asked(&pair.host);
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
    let session = reaches(&pair.controller, pending.id, SessionPhase::Active);
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
    expect![[r#"
        controller:
          service: service phase from=waiting generation=0 kind=Desktop service=1 to=closed: OwnerClose
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          service: service phase from=waiting generation=1 kind=Files service=2 to=active
    "#]].assert_eq(&trace.story(&["service", "session"]));
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
    let pending = controller.begin_connect(&pair.host.endpoint())?;
    wait_asked(&pair.host);
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
    expect![[r#"
        controller:
          session: session failed failure=unauthorized session=1
          session: session phase from=connecting generation=0 role=Controller session=1 to=failed
          service: service phase from=waiting generation=0 kind=Desktop service=1 to=failed
    "#]]
    .assert_eq(&trace.story(&["service", "session"]));
    Ok(())
}

/// A services connection is admitted without any desktop grant or codec:
/// it carries no input, video or Desktop, and Files opens only once the
/// host shares it, while the connection's own file calls stay refused; a
/// redial moves the owner on, and withdrawing Files ends it and leaves the
/// connection.
#[test]
fn a_neutral_connection_grants_nothing_until_files_is_shared() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let (root, folder) = whole_disk(&pair.host)?;
    host.set_desktop_sharing(Grants::default())?;
    host.set_video_codecs(VideoCodecCaps::default())?;
    controller.set_video_codecs(VideoCodecCaps::default())?;
    let session = pair.connect_services();
    let incoming = host.snapshot()?.incoming.ok_or("the host's session")?;
    assert_eq!(
        (session.monitor_width, session.monitor_height),
        (0, 0),
        "the controller sees no monitor"
    );
    assert_eq!(
        (incoming.monitor_width, incoming.monitor_height),
        (0, 0),
        "the host offers no monitor"
    );
    assert!(
        session.quality.is_none() && incoming.quality.is_none(),
        "neither side has a picture quality"
    );
    assert!(
        matches!(
            controller.submit_input(
                session.id,
                session.generation,
                InputEvent::pointer(1, 1, 1, 1)
            ),
            Err(Error::Unsupported(_))
        ),
        "a neutral connection carries no input"
    );
    assert!(
        matches!(
            controller.subscribe_session_video(session.id, session.generation),
            Err(Error::Unsupported(_))
        ),
        "a neutral connection carries no video"
    );
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
    // Even with Transfer, the connection's own calls cannot bypass service
    // admission.
    assert!(
        controller
            .fs_mkdir(session.id, session.generation, root, &folder, "legacy")
            .is_err(),
        "the connection's own file calls stay refused"
    );
    assert!(
        !pair.host.root().join("legacy").exists(),
        "a refused call changes nothing"
    );
    let old_lease = controller.service_lease(owner.id, owner.generation)?;
    controller.drop_controller_link(session.id)?;
    redialed(&pair.controller, &session);
    assert!(old_lease.is_cancelled(), "the redial ends the old lease");
    let fresh = controller.controller_service(owner.id)?;
    let next = controller.open_files_service(fresh.id, fresh.generation)?;
    pipe.close();
    assert_eq!(
        next.list(root, &folder, 0, 20)?.entries[0].name,
        "owned",
        "the owner's next pipe works after the old one closed"
    );
    assert!(
        controller.controller_session(session.id)?.quality.is_none(),
        "the redial stays neutral"
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
        hosted_ends(&trace, "host"),
        [
            (1, "closed: ParentLoss".to_owned()),
            (2, "closed: Revoked".to_owned())
        ],
        "the host ends the old generation's Files as its lost parent, and the next as revoked"
    );
    // The host names the lost parent on either of two paths, at a moment of
    // its own, and the redial suspends and resumes the controller's services
    // together, in no order.
    expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=suspended
          service: service phase from=active generation=1 kind=Files service=2 to=suspended
          service: service phase from=active generation=3 kind=Files service=2 to=closed: PeerClose
          service: service phase from=suspended generation=3 kind=Desktop service=1 to=active
          service: service phase from=suspended generation=3 kind=Files service=2 to=active
    "#]]
    .assert_eq(
        &trace
            .tell(&["service"])
            .of("controller")
            .concurrent()
            .to_string(),
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
    wait_asked(&host);
    host.core
        .confirm_control_request(&controller.core.control_check())?;
    reaches(&controller, pending.id, SessionPhase::Active);
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
    expect![[r#"
        controller:
          pairing: invitation accepted
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request answered
    "#]]
    .assert_eq(&trace.story(&["pairing", "session"]));
    Ok(())
}
