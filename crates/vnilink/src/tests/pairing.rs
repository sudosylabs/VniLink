use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::pairing::{self, PairingFile, PairingPreview, PAIRING_FILE_EXTENSION};
use crate::session::{Grants, SessionPhase, SessionRole};

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

fn wait_closed(core: &Core, role: SessionRole) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = core.snapshot().unwrap();
        let session = match role {
            SessionRole::Host => snapshot.incoming,
            SessionRole::Controller => snapshot.outgoing,
        };
        if session
            .as_ref()
            .is_some_and(|session| session.phase == SessionPhase::Closed)
        {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("session did not close: {session:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_request(host: &Core) -> crate::ControlRequest {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(request) = host.control_request().unwrap() {
            return request;
        }
        if std::time::Instant::now() > deadline {
            panic!("the host did not report a control request");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pairing_counts(events: &EventSubscription) -> (usize, usize) {
    let mut paired = 0;
    let mut authorized = 0;
    while let Some(event) = events.try_recv() {
        match event {
            CoreEvent::DevicePaired { .. } => paired += 1,
            CoreEvent::DeviceAuthorized { .. } => authorized += 1,
            CoreEvent::ControlRequested { .. } => {}
            // Connect publishes the session lifecycle and address refresh.
            CoreEvent::SessionChanged { .. }
            | CoreEvent::PeerRenamed { .. }
            | CoreEvent::AddressesUpdated { .. }
            | CoreEvent::TopologyChanged { .. } => {}
            other => panic!("pairing emitted {other:?}"),
        }
    }
    (paired, authorized)
}

#[test]
fn file_digits_change_when_the_invitation_changes() {
    let endpoint = [7u8; 32];
    let nonce = [9u8; 16];
    let check = pairing::file_digits(&endpoint, &nonce, 50);
    assert_eq!(check, pairing::file_digits(&endpoint, &nonce, 50));
    assert_eq!(check.len(), 3);
    assert!(check.bytes().all(|byte| byte.is_ascii_digit()));
    assert_ne!(check, pairing::file_digits(&endpoint, &nonce, 51));
    assert_ne!(check, pairing::control_digits(&endpoint));
    assert_eq!(PAIRING_FILE_EXTENSION, "vnl");
}

#[test]
fn a_damaged_file_is_rejected_and_debug_hides_the_invitation() {
    let host = start_direct();
    let file = host
        .export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    assert!(file.bytes.starts_with(b"VNLF"));
    assert!(!format!("{file:?}").contains("vnl1."));
    let mut damaged = file.bytes.clone();
    damaged[0] = b'X';
    let controller = start_direct();
    assert!(matches!(
        controller.open_pairing_file(&damaged),
        Err(Error::InvalidPairingFile)
    ));
    assert!(matches!(
        controller.open_pairing_file(b"VNLF"),
        Err(Error::InvalidPairingFile)
    ));
    assert!(matches!(
        host.open_pairing_file(&file.bytes),
        Err(Error::InvalidInvitation(_))
    ));
}

#[test]
fn an_expired_file_is_rejected() {
    let host = start_direct();
    let controller = start_direct();
    let past = SystemTime::now() - Duration::from_secs(20 * 60);
    let file = host
        .export_pairing_file_at(Duration::from_secs(60), true, past)
        .unwrap();
    assert!(matches!(
        controller.open_pairing_file(&file.bytes),
        Err(Error::InvitationExpired)
    ));
}

#[test]
fn one_file_pairs_both_directions_without_granting_services() {
    let host = start_direct();
    let controller = start_direct();
    let file = host
        .export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let preview = controller.open_pairing_file(&file.bytes).unwrap();
    assert_eq!(preview.check, file.check);
    assert_eq!(preview.endpoint_id, host.endpoint_id());
    assert!(!format!("{preview:?}").contains("vnl1."));
    assert!(controller.snapshot().unwrap().devices.is_empty());

    let controller_events = controller.subscribe().unwrap();
    let saved = controller.confirm_pairing(&preview, &file.check).unwrap();
    assert_eq!(saved.endpoint_id, host.endpoint_id());
    assert!(!saved.grants.control);
    assert!(!saved.grants.clipboard);
    assert!(!saved.grants.transfer);
    assert_eq!(pairing_counts(&controller_events), (1, 0));
    assert!(matches!(
        controller.confirm_pairing(&preview, &file.check),
        Err(Error::InvitationReplayed)
    ));

    host.start_hosting().unwrap();
    let host_events = host.subscribe().unwrap();
    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let request = wait_request(&host);
        assert_eq!(request.endpoint_id, controller.endpoint_id());
        assert_eq!(request.check, controller.control_check());
        let granted = host.confirm_control_request(&request.check).unwrap();
        assert!(!granted.grants.control);
        assert!(!granted.grants.clipboard);
        assert!(!granted.grants.transfer);
        connecting.join().unwrap().unwrap()
    });
    assert_eq!(session.role, SessionRole::Controller);
    assert_eq!(session.phase, SessionPhase::Active);
    assert_eq!(pairing_counts(&host_events), (1, 0));
    assert!(!host.snapshot().unwrap().devices[0].grants.control);
    assert!(host.control_request().unwrap().is_none());

    controller.disconnect().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let again = connect_neutral(&controller, &host).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
    assert_eq!(saved.os, Some(crate::OperatingSystem::local()));
    assert_eq!(
        host.snapshot().unwrap().devices[0].os,
        Some(crate::OperatingSystem::local())
    );
    assert_eq!(
        host.snapshot().unwrap().devices[0].presence,
        crate::DevicePresence::Connected
    );
    controller.start_hosting().unwrap();
    assert_eq!(
        connect_neutral(&host, &controller).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        host.sharing_policy().unwrap(),
        crate::SharingPolicy::default()
    );
    assert_eq!(
        controller.sharing_policy().unwrap(),
        crate::SharingPolicy::default()
    );
}

#[test]
fn the_controller_waits_while_the_host_owner_types_the_digits() {
    let host = start_direct();
    let controller = start_direct();
    host.start_hosting().unwrap();
    let file = host
        .export_pairing_file(Duration::from_secs(300), true)
        .unwrap();
    let preview = controller.open_pairing_file(&file.bytes).unwrap();
    controller.confirm_pairing(&preview, &file.check).unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let request = wait_request(&host);
    // Longer than an ordinary frame read: a person is reading digits aloud.
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(
        host.control_request()
            .unwrap()
            .map(|current| current.endpoint_id),
        Some(request.endpoint_id),
        "the controller hung up while the host was still asking"
    );
    host.confirm_control_request(&controller.control_check())
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while controller.controller_session(pending.id).unwrap().phase != SessionPhase::Active {
        assert!(
            std::time::Instant::now() < deadline,
            "the approved link did not come up"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_mismatched_file_check_saves_nothing() {
    let host = start_direct();
    let controller = start_direct();
    let file = host
        .export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let preview = controller.open_pairing_file(&file.bytes).unwrap();
    let events = controller.subscribe().unwrap();
    let wrong = if file.check == "000" { "001" } else { "000" };
    assert!(matches!(
        controller.confirm_pairing(&preview, wrong),
        Err(Error::PairingCheckMismatch)
    ));
    assert!(controller.snapshot().unwrap().devices.is_empty());
    assert!(events.try_recv().is_none());
}

#[test]
fn the_host_rejects_a_control_request_whose_digits_do_not_match() {
    let host = start_direct();
    let controller = start_direct();
    exchange(&host, &controller, true);
    host.start_hosting().unwrap();
    let denied = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let request = wait_request(&host);
        let wrong = if request.check == "000" { "001" } else { "000" };
        assert!(matches!(
            host.confirm_control_request(wrong),
            Err(Error::PairingCheckMismatch)
        ));
        connecting.join().unwrap()
    });
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "mismatched control request returned {denied:?}"
    );
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert_eq!(controller.snapshot().unwrap().devices.len(), 1);

    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let request = wait_request(&host);
        host.confirm_control_request(&request.check).unwrap();
        connecting.join().unwrap().unwrap()
    });
    assert_eq!(session.phase, SessionPhase::Active);
}

#[test]
fn a_file_without_the_control_switch_does_not_ask_the_host() {
    let host = start_direct();
    let controller = start_direct();
    exchange(&host, &controller, false);
    host.start_hosting().unwrap();
    let events = host.subscribe().unwrap();
    let denied = connect_neutral(&controller, &host);
    assert!(
        matches!(denied, Err(Error::Unauthorized)),
        "control switch off returned {denied:?}"
    );
    assert!(host.control_request().unwrap().is_none());
    assert!(host.snapshot().unwrap().devices.is_empty());
    while let Some(event) = events.try_recv() {
        assert!(
            !matches!(event, CoreEvent::ControlRequested { .. }),
            "the host asked for digits with the switch off"
        );
    }
}

#[test]
fn rejecting_the_control_request_leaves_the_file_usable() {
    let host = start_direct();
    let controller = start_direct();
    exchange(&host, &controller, true);
    host.start_hosting().unwrap();
    let denied = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let _request = wait_request(&host);
        host.reject_control_request().unwrap();
        connecting.join().unwrap()
    });
    assert!(matches!(denied, Err(Error::Unauthorized)), "{denied:?}");
    assert!(matches!(
        host.reject_control_request(),
        Err(Error::NoControlRequest)
    ));
    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let request = wait_request(&host);
        host.confirm_control_request(&controller.control_check())
            .unwrap();
        assert_eq!(request.check, controller.control_check());
        connecting.join().unwrap().unwrap()
    });
    assert_eq!(session.phase, SessionPhase::Active);
}

fn exchange(host: &Core, controller: &Core, allow_control: bool) -> PairingFile {
    let file = host
        .export_pairing_file(Duration::from_secs(60), allow_control)
        .unwrap();
    let preview: PairingPreview = controller.open_pairing_file(&file.bytes).unwrap();
    controller.confirm_pairing(&preview, &file.check).unwrap();
    file
}

#[test]
fn approving_control_after_the_window_expires_is_refused() {
    let host = start_direct();
    let controller = start_direct();
    let file = host
        .export_pairing_file_at(Duration::from_secs(2), true, SystemTime::now())
        .unwrap();
    let preview = controller.open_pairing_file(&file.bytes).unwrap();
    controller.confirm_pairing(&preview, &file.check).unwrap();
    host.start_hosting().unwrap();
    std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect_neutral(&controller, &host));
        let request = wait_request(&host);
        std::thread::sleep(Duration::from_secs(3));
        assert!(matches!(
            host.confirm_control_request(&request.check),
            Err(Error::InvitationExpired)
        ));
        assert!(matches!(
            connecting.join().unwrap(),
            Err(Error::Unauthorized)
        ));
    });
    assert!(host.control_request().unwrap().is_none());
}

#[test]
fn accepting_a_second_invitation_keeps_authorized_grants() {
    let host = start_direct();
    let controller = start_direct();
    let first = host.create_invitation(Duration::from_secs(60)).unwrap();
    let saved = controller.accept_invitation(&first).unwrap();
    assert!(!saved.grants.clipboard);
    controller
        .set_desktop_sharing(Grants {
            view: false,
            control: false,
            clipboard: true,
            transfer: true,
        })
        .unwrap();
    let second = host.create_invitation(Duration::from_secs(60)).unwrap();
    let kept = controller.accept_invitation(&second).unwrap();
    assert!(!kept.grants.control);
    assert!(kept.grants.clipboard);
    assert!(kept.grants.transfer);
}

#[test]
fn remembered_peer_denied_by_global_policy_never_prompts_again() {
    let host = start_direct();
    let controller = start_direct();
    // The host pairs the controller first so it can authorize side grants.
    let listing = controller
        .export_pairing_file(Duration::from_secs(60), false)
        .unwrap();
    let preview = host.open_pairing_file(&listing.bytes).unwrap();
    host.confirm_pairing(&preview, &listing.check).unwrap();
    host.set_desktop_sharing(Grants {
        view: false,
        control: false,
        clipboard: true,
        transfer: true,
    })
    .unwrap();

    exchange(&host, &controller, true);
    host.start_hosting().unwrap();
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(host.control_request().unwrap().is_none());
    let session = connect_neutral(&controller, &host).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    assert!(host.sharing_policy().unwrap().clipboard);
    assert!(!host.sharing_policy().unwrap().view);
}

fn connect_neutral(controller: &Core, host: &Core) -> Result<crate::SessionInfo, Error> {
    let pending = controller.begin_connect_services(&host.endpoint_id())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    loop {
        let session = controller.controller_session(pending.id)?;
        match session.phase {
            SessionPhase::Active => return Ok(session),
            SessionPhase::Failed(_) => return Err(Error::Unauthorized),
            SessionPhase::Closed => return Err(Error::Unauthorized),
            _ => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "neutral connect timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
