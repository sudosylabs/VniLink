//! Pairing alone: two engines that do not know each other becoming paired
//! through a pairing file and its digits, as two people do, and forgetting
//! each other again; what paired computers know of each other, how they
//! reach each other again, and what an installation keeps across a restart.

#[path = "pairing/approval.rs"]
mod approval;
#[path = "pairing/identity.rs"]
mod identity;
#[path = "pairing/link.rs"]
mod link;
#[path = "pairing/names.rs"]
mod names;
#[path = "pairing/persistence.rs"]
mod persistence;
#[path = "pairing/policies.rs"]
mod policies;
#[path = "pairing/presence.rs"]
mod presence;
#[path = "pairing/reconnect.rs"]
mod reconnect;
#[path = "pairing/records.rs"]
mod records;
#[path = "pairing/saving.rs"]
mod saving;
#[path = "pairing/support.rs"]
mod support;

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CoreEvent, DevicePresence, Error, EventSubscription, OperatingSystem, SessionPhase,
    SessionRole, SharingPolicy,
};
use vnilink_testkit::events::drain;
use vnilink_testkit::session::{wait_closed, wait_request};
use vnilink_testkit::{traced, wait_for, Engine, TestResult, DEFAULT_WAIT};

const FILE_LIFETIME: Duration = Duration::from_secs(60);

/// How many devices `events` says were paired and authorized; pairing
/// publishes nothing else besides a session's lifecycle and what a connect
/// learns of its peer.
fn pairing_counts(events: &EventSubscription) -> (usize, usize) {
    let mut paired = 0;
    let mut authorized = 0;
    for event in drain(events) {
        match event {
            CoreEvent::DevicePaired { .. } => paired += 1,
            CoreEvent::DeviceAuthorized { .. } => authorized += 1,
            CoreEvent::ControlRequested { .. }
            | CoreEvent::SessionChanged { .. }
            | CoreEvent::PeerRenamed { .. }
            | CoreEvent::AddressesUpdated { .. }
            | CoreEvent::TopologyChanged { .. } => {}
            other => panic!("pairing published {other:?}"),
        }
    }
    (paired, authorized)
}

/// One file, its digits read across, pairs both ways: the host then asks
/// its owner to approve the controller by the controller's digits, and
/// nothing is granted beyond being remembered, on either side. Each
/// computer publishes one pairing; the pair reconnects and connects the
/// other way without another file. Each step is recorded, peers only by
/// fingerprint.
#[test]
fn a_pairing_file_and_its_digits_pair_both_computers() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, true)?;
    let preview = controller.core.open_pairing_file(&file.bytes)?;
    assert_eq!(preview.check, file.check, "both computers show the digits");
    assert_eq!(
        preview.endpoint_id,
        host.endpoint(),
        "the file names its host"
    );
    assert!(
        !format!("{preview:?}").contains("vnl1."),
        "a preview's Debug hides the invitation"
    );
    assert!(
        controller.core.snapshot()?.devices.is_empty(),
        "opening a file remembers nobody"
    );
    let controller_events = controller.core.subscribe()?;
    let saved = controller.core.confirm_pairing(&preview, &file.check)?;
    assert_eq!(saved.endpoint_id, host.endpoint(), "the host is remembered");
    assert!(
        !saved.grants.control && !saved.grants.transfer && !saved.grants.clipboard,
        "the controller grants the host nothing: {:?}",
        saved.grants
    );
    assert_eq!(
        pairing_counts(&controller_events),
        (1, 0),
        "the controller publishes one pairing and no authorization"
    );
    assert!(
        matches!(
            controller.core.confirm_pairing(&preview, &file.check),
            Err(Error::InvitationReplayed)
        ),
        "a file pairs once"
    );

    host.core.start_hosting()?;
    let host_events = host.core.subscribe()?;
    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| controller.connect_services(&host));
        let request = wait_request(&host.core, Duration::from_secs(5));
        assert_eq!(
            request.endpoint_id,
            controller.endpoint(),
            "the host asks about the controller"
        );
        assert_eq!(
            request.check,
            controller.core.control_check(),
            "the host asks by the controller's digits"
        );
        let granted = host
            .core
            .confirm_control_request(&request.check)
            .expect("the approval");
        assert!(
            !granted.grants.control && !granted.grants.clipboard && !granted.grants.transfer,
            "approving grants nothing: {:?}",
            granted.grants
        );
        connecting.join().expect("the connecting thread")
    })?;
    assert_eq!(session.role, SessionRole::Controller);
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "the approved link opens"
    );
    assert_eq!(
        pairing_counts(&host_events),
        (1, 0),
        "the host publishes one pairing and no authorization"
    );
    let devices = host.core.snapshot()?.devices;
    assert_eq!(devices.len(), 1, "the host remembers the controller");
    assert!(!devices[0].grants.control, "the host grants no control");
    assert!(
        host.core.control_request()?.is_none(),
        "an approved request is answered"
    );

    controller.core.disconnect()?;
    wait_closed(&host.core, SessionRole::Host, Duration::from_secs(5));
    wait_closed(
        &controller.core,
        SessionRole::Controller,
        Duration::from_secs(5),
    );
    let again = controller.connect_services(&host)?;
    assert_eq!(
        again.phase,
        SessionPhase::Active,
        "a paired controller returns without a file"
    );
    assert_eq!(
        saved.os,
        Some(OperatingSystem::local()),
        "the file names its host's system"
    );
    let remembered = &host.core.snapshot()?.devices[0];
    assert_eq!(
        remembered.os,
        Some(OperatingSystem::local()),
        "the host learns the controller's system"
    );
    assert_eq!(
        remembered.presence,
        DevicePresence::Connected,
        "the host sees the controller connected"
    );
    controller.core.start_hosting()?;
    assert_eq!(
        host.connect_services(&controller)?.phase,
        SessionPhase::Active,
        "the pair connects the other way without a file"
    );
    assert_eq!(
        host.core.sharing_policy()?,
        SharingPolicy::default(),
        "pairing changes no sharing on the host"
    );
    assert_eq!(
        controller.core.sharing_policy()?,
        SharingPolicy::default(),
        "pairing changes no sharing on the controller"
    );

    expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request answered
    "#]]
    .assert_eq(&trace.story(&["pairing"]));
    // Peers are recorded by fingerprint only.
    let accepted = &trace.named("invitation accepted")[0];
    assert_eq!(
        accepted["peer"].as_str().map(str::len),
        Some(8),
        "{accepted}"
    );
    trace.assert_unrecorded("a full peer identity", &[&host.endpoint()]);
    Ok(())
}

/// Digits that do not match save nothing and publish nothing, and the file
/// can still be used.
#[test]
fn mismatched_digits_save_nothing() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    // With control asked for, as the merged original did.
    let file = host.core.export_pairing_file(FILE_LIFETIME, true)?;
    let preview = controller.core.open_pairing_file(&file.bytes)?;
    let events = controller.core.subscribe()?;
    let wrong = if file.check == "000" { "111" } else { "000" };
    assert!(
        matches!(
            controller.core.confirm_pairing(&preview, wrong),
            Err(Error::PairingCheckMismatch)
        ),
        "wrong digits are refused"
    );
    assert!(
        controller.core.snapshot()?.devices.is_empty(),
        "wrong digits remember nobody"
    );
    assert!(events.try_recv().is_none(), "wrong digits publish nothing");
    expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: pairing digits did not match
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
    "#]]
    .assert_eq(&trace.story(&["pairing"]));
    controller.core.confirm_pairing(&preview, &file.check)?;
    assert_eq!(
        controller.core.snapshot()?.devices.len(),
        1,
        "the file still pairs with the right digits"
    );
    Ok(())
}

/// A damaged file is refused and says so; a file pairs only once.
#[test]
fn a_damaged_file_is_refused_and_a_file_pairs_once() {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, false).unwrap();
    let mut damaged = file.bytes.clone();
    // The file ends in the signature's hex digits: another digit there
    // reads as a file and fails its signature, the same way every run.
    let last = damaged.len() - 1;
    damaged[last] = if damaged[last] == b'0' { b'1' } else { b'0' };
    assert!(controller.core.open_pairing_file(&damaged).is_err());
    let preview = controller.core.open_pairing_file(&file.bytes).unwrap();
    controller
        .core
        .confirm_pairing(&preview, &file.check)
        .unwrap();
    assert!(matches!(
        controller.core.confirm_pairing(&preview, &file.check),
        Err(Error::InvitationReplayed)
    ));
    expect![[r#"
        controller:
          pairing: pairing file opened bytes=284 reason=signature does not match refused=invalid invitation
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
        host:
          pairing: pairing file exported allow_control=false direct=1 relays=0 ttl_s=60
    "#]].assert_eq(&trace.story(&["pairing"]));
}

/// What is not a pairing file, or only its start, is refused as such, and
/// a computer cannot open its own file; a file's Debug hides the
/// invitation it carries.
#[test]
fn a_file_that_is_not_one_or_is_ones_own_is_refused() -> TestResult {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, true)?;
    assert!(file.bytes.starts_with(b"VNLF"), "a pairing file says so");
    assert!(
        !format!("{file:?}").contains("vnl1."),
        "a file's Debug hides the invitation"
    );
    let mut damaged = file.bytes.clone();
    damaged[0] = b'X';
    assert!(
        matches!(
            controller.core.open_pairing_file(&damaged),
            Err(Error::InvalidPairingFile)
        ),
        "a file with another start is no pairing file"
    );
    assert!(
        matches!(
            controller.core.open_pairing_file(b"VNLF"),
            Err(Error::InvalidPairingFile)
        ),
        "a file's start alone is no pairing file"
    );
    assert!(
        matches!(
            host.core.open_pairing_file(&file.bytes),
            Err(Error::InvalidInvitation(_))
        ),
        "a computer cannot open its own file"
    );
    expect![[r#"
        controller:
          pairing: pairing file opened bytes=284 refused=invalid pairing file
          pairing: pairing file opened bytes=4 refused=invalid pairing file
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: pairing file opened bytes=284 reason=an installation cannot pair with itself refused=invalid invitation
    "#]].assert_eq(&trace.story(&["pairing"]));
    Ok(())
}

/// A host that forgets its controller ends their connection, and the
/// controller cannot connect again.
#[test]
fn a_forgotten_controller_is_disconnected_and_cannot_return() {
    let trace = traced();
    let pair = vnilink_testkit::Pair::for_desktop();
    let session = pair.connect_services();
    pair.host.core.unpair(&pair.controller.endpoint()).unwrap();
    wait_for("the connection to end", DEFAULT_WAIT, || {
        let phase = pair
            .controller
            .core
            .controller_session(session.id)
            .ok()?
            .phase;
        (phase != SessionPhase::Active).then_some(())
    });
    assert!(pair.controller.connect_services(&pair.host).is_err());
    expect![[r#"
        controller:
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
          pairing: unpair requested
    "#]]
    .assert_eq(&trace.story(&["pairing"]));
}
