//! Pairing alone: two engines that do not know each other becoming paired
//! through a pairing file and its digits, as two people do, and forgetting
//! each other again.

use expect_test::expect;
use std::time::Duration;

use vnilink::{Error, SessionPhase};
use vnilink_testkit::{traced, wait_for, Engine, DEFAULT_WAIT};

const FILE_LIFETIME: Duration = Duration::from_secs(60);

/// The controller's services connection to `host`, or why it was refused.
fn connect(controller: &Engine, host: &Engine) -> Result<vnilink::SessionInfo, Error> {
    let pending = controller.core.begin_connect_services(&host.endpoint())?;
    wait_for(
        "the connection to settle",
        DEFAULT_WAIT,
        || match controller.core.controller_session(pending.id) {
            Ok(session) if session.phase == SessionPhase::Active => Some(Ok(session)),
            Ok(session)
                if matches!(
                    session.phase,
                    SessionPhase::Failed(_) | SessionPhase::Closed
                ) =>
            {
                Some(Err(Error::Unauthorized))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        },
    )
}

/// One file, its digits read across, pairs both ways: the host then asks
/// its owner to approve the controller by the controller's digits, and
/// nothing is granted beyond being remembered. Each step is recorded,
/// peers only by fingerprint.
#[test]
fn a_pairing_file_and_its_digits_pair_both_computers() {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, true).unwrap();
    let preview = controller.core.open_pairing_file(&file.bytes).unwrap();
    assert_eq!(preview.check, file.check);
    let saved = controller
        .core
        .confirm_pairing(&preview, &file.check)
        .unwrap();
    assert_eq!(saved.endpoint_id, host.endpoint());
    assert!(!saved.grants.control && !saved.grants.transfer && !saved.grants.clipboard);

    host.core.start_hosting().unwrap();
    let session = std::thread::scope(|scope| {
        let connecting = scope.spawn(|| connect(&controller, &host));
        let request = wait_for("the host to ask its owner", DEFAULT_WAIT, || {
            host.core.control_request().ok().flatten()
        });
        assert_eq!(request.check, controller.core.control_check());
        host.core.confirm_control_request(&request.check).unwrap();
        connecting.join().unwrap()
    })
    .unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    assert_eq!(host.core.snapshot().unwrap().devices.len(), 1);

    expect![[r#"
        controller:
          pairing: pairing file opened bytes=320
          pairing: invitation accepted
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
    let everything = trace.events().join("\n");
    assert!(
        !everything.contains(&host.endpoint()),
        "a full peer identity was recorded"
    );
}

/// Digits that do not match save nothing, and the file can still be used.
#[test]
fn mismatched_digits_save_nothing() {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, false).unwrap();
    let preview = controller.core.open_pairing_file(&file.bytes).unwrap();
    let wrong = if file.check == "000" { "111" } else { "000" };
    assert!(matches!(
        controller.core.confirm_pairing(&preview, wrong),
        Err(Error::PairingCheckMismatch)
    ));
    assert!(controller.core.snapshot().unwrap().devices.is_empty());
    expect![[r#"
        controller:
          pairing: pairing file opened bytes=320
          pairing: pairing digits did not match
        host:
          pairing: pairing file exported allow_control=false direct=1 relays=0 ttl_s=60
    "#]]
    .assert_eq(&trace.story(&["pairing"]));
    controller
        .core
        .confirm_pairing(&preview, &file.check)
        .unwrap();
    assert_eq!(controller.core.snapshot().unwrap().devices.len(), 1);
}

/// A damaged file is refused and says so; a file pairs only once.
#[test]
fn a_damaged_file_is_refused_and_a_file_pairs_once() {
    let trace = traced();
    let host = Engine::start("host");
    let controller = Engine::start("controller");
    let file = host.core.export_pairing_file(FILE_LIFETIME, false).unwrap();
    let mut damaged = file.bytes.clone();
    let middle = damaged.len() / 2;
    damaged[middle] ^= 0x55;
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
          pairing: pairing file opened bytes=320 reason=unrecognized invitation refused=invalid invitation
          pairing: pairing file opened bytes=320
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
        host:
          pairing: pairing file exported allow_control=false direct=1 relays=0 ttl_s=60
    "#]].assert_eq(&trace.story(&["pairing"]));
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
    assert!(connect(&pair.controller, &pair.host).is_err());
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
