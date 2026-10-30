// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A save that fails: a change the store refused is undone and published
//! nowhere, the engine keeping on; a save whose outcome is uncertain, or
//! that panics, also stops the engine, which tries no second write and
//! closes its endpoint once.

use expect_test::expect;
use std::time::Duration;

use vnilink::{
    CoreEvent, Error, EventSubscription, Grants, RelaySettings, ServiceKind, SessionPhase,
};
use vnilink_testkit::events::drain;
use vnilink_testkit::session::wait_request;
use vnilink_testkit::store::{Fault, Installed, TestStore};
use vnilink_testkit::{traced, wait_for, Engine, TestResult};

use super::support::{credentialed_relay, exchange, EVERYTHING};

const LIFETIME: Duration = Duration::from_secs(60);

/// Whether `events` published one that `kind` picks, taking them all.
fn published(events: &EventSubscription, kind: fn(&CoreEvent) -> bool) -> bool {
    drain(events).iter().any(kind)
}

/// After an uncertain save: the record is `before`, `attempts` saves were
/// asked of the store and the engine stopped, so no later change reaches
/// the store; shutting it down closes its endpoint once.
fn assert_fenced(host: &Installed<TestStore>, before: &[u8], attempts: usize) {
    assert_eq!(
        host.store.record().as_deref(),
        Some(before),
        "an uncertain save leaves the record as it was"
    );
    assert_eq!(
        host.store.save_attempts(),
        attempts,
        "an uncertain save is tried once"
    );
    assert!(
        matches!(host.core.snapshot(), Err(Error::ShutDown)),
        "an uncertain save stops the engine"
    );
    assert!(
        matches!(
            host.core.set_desktop_sharing(EVERYTHING),
            Err(Error::ShutDown)
        ),
        "a stopped engine changes nothing"
    );
    assert_eq!(
        host.store.save_attempts(),
        attempts,
        "a stopped engine asks no second save"
    );
    host.core.shutdown();
    assert_eq!(
        host.core.endpoint_close_count(),
        1,
        "the endpoint closes once"
    );
}

/// Each change a failed save refuses is undone and published nowhere: the
/// relay, a pairing, a grant, a refreshed invitation and a revocation, the
/// session the revocation would end staying up. Each succeeds once the
/// store saves, and the revocation then ends the session.
#[test]
fn a_failed_save_keeps_the_previous_record() -> TestResult {
    let trace = traced();
    let host = Installed::faulty("host");
    let before = host.store.record().ok_or("the first open saves a record")?;
    host.store.fail_next_save(Fault::Locked);
    assert!(
        matches!(
            host.core
                .set_relay(credentialed_relay("relay-credential-alpha")?),
            Err(Error::StoreLocked)
        ),
        "a relay whose save fails is refused"
    );
    assert_eq!(
        host.store.record(),
        Some(before),
        "the record keeps the previous relay"
    );
    assert!(
        host.core.snapshot()?.public_relay,
        "and the engine shows it"
    );

    let controller = Engine::start("controller");
    host.core.start_hosting()?;
    let events = host.core.subscribe()?;
    let invitation = controller.core.create_invitation(LIFETIME)?;
    host.store.fail_next_save(Fault::Locked);
    drain(&events);
    assert!(
        matches!(
            host.core.accept_invitation(&invitation),
            Err(Error::StoreLocked)
        ),
        "a pairing whose save fails is refused"
    );
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "the peer is not remembered"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DevicePaired { .. }
        )),
        "nor its pairing published"
    );
    host.core.accept_invitation(&invitation)?;

    host.store.fail_next_save(Fault::Locked);
    drain(&events);
    assert!(
        matches!(
            host.core.set_desktop_sharing(EVERYTHING),
            Err(Error::StoreLocked)
        ),
        "a grant whose save fails is refused"
    );
    assert_eq!(
        host.core.snapshot()?.devices[0].grants,
        Grants::default(),
        "the peer keeps no grant"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DeviceAuthorized { .. }
        )),
        "nor is an authorization published"
    );
    host.core.set_desktop_sharing(EVERYTHING)?;

    let again = controller.core.create_invitation(LIFETIME)?;
    host.store.fail_next_save(Fault::Unavailable);
    assert!(
        matches!(
            host.core.accept_invitation(&again),
            Err(Error::StoreUnavailable)
        ),
        "a refreshed invitation whose save fails is refused"
    );
    assert_eq!(
        host.core.snapshot()?.devices[0].grants,
        EVERYTHING,
        "the peer keeps its grants"
    );
    host.core.accept_invitation(&again)?;
    assert_eq!(
        host.core.snapshot()?.devices[0].grants,
        EVERYTHING,
        "and keeps them when the refresh is saved"
    );
    host.core.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })?;

    controller
        .core
        .accept_invitation(&host.core.create_invitation(LIFETIME)?)?;
    let session = controller.connect_services_to(&host.core.endpoint_id())?;
    assert_eq!(
        session.phase,
        SessionPhase::Active,
        "the controller connects"
    );
    let incoming = host
        .core
        .snapshot()?
        .incoming
        .ok_or("an incoming session")?;
    assert_eq!(incoming.phase, SessionPhase::Active, "the host serves it");
    host.store.fail_next_save(Fault::Locked);
    drain(&events);
    assert!(
        matches!(
            host.core.revoke(&controller.endpoint()),
            Err(Error::StoreLocked)
        ),
        "a revocation whose save fails is refused"
    );
    let still = host
        .core
        .snapshot()?
        .incoming
        .ok_or("an incoming session")?;
    assert_eq!(still.phase, SessionPhase::Active, "the session stays up");
    assert_eq!(
        still.generation, incoming.generation,
        "the same session, not another"
    );
    assert!(
        host.core.snapshot()?.devices[0].grants.control,
        "the peer keeps control"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DeviceRevoked { .. }
        )),
        "nor is a revocation published"
    );

    host.core.revoke(&controller.endpoint())?;
    wait_for(
        "the revocation to end the session",
        Duration::from_secs(5),
        || {
            let phase = host
                .core
                .snapshot()
                .expect("the host's snapshot")
                .incoming?
                .phase;
            (phase == SessionPhase::Closed).then_some(())
        },
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: invitation accepted
        host:
          pairing: invitation accepted refused=store locked
          pairing: invitation accepted
          pairing: invitation accepted refused=store unavailable
          pairing: invitation accepted
          pairing: invitation issued direct=1 relays=0 ttl_s=60
          pairing: unpair requested refused=store locked
          pairing: unpair requested
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A save the store reports unavailable refuses the change and publishes
/// nothing, but the engine keeps on, and the change saves when asked
/// again; shutting it down closes its endpoint once.
#[test]
fn an_unavailable_save_does_not_fence_the_engine() -> TestResult {
    let _trace = traced();
    let host = Installed::faulty("host");
    let before = host.store.record().ok_or("the first open saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::Unavailable);
    let events = host.core.subscribe()?;
    drain(&events);
    assert!(
        matches!(
            host.core.set_desktop_sharing(EVERYTHING),
            Err(Error::StoreUnavailable)
        ),
        "a grant whose save fails is refused"
    );
    assert_eq!(host.store.record(), Some(before), "the record is as it was");
    assert_eq!(
        host.store.save_attempts(),
        attempts + 1,
        "the save was tried once"
    );
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "the engine keeps on"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DeviceAuthorized { .. }
        )),
        "nothing is published"
    );
    host.core.set_desktop_sharing(EVERYTHING)?;
    assert_eq!(
        host.store.save_attempts(),
        attempts + 2,
        "asking again saves again"
    );
    let sharing = host.core.sharing_policy()?;
    assert_eq!(
        (
            sharing.view,
            sharing.control,
            sharing.clipboard,
            sharing.files
        ),
        (true, true, true, true),
        "and the change holds: {sharing:?}"
    );
    host.core.shutdown();
    assert_eq!(
        host.core.endpoint_close_count(),
        1,
        "the endpoint closes once"
    );
    Ok(())
}

/// A grant whose save is uncertain stops the engine without a second
/// write, and is published nowhere.
#[test]
fn an_uncertain_consent_save_fences_without_a_second_write() -> TestResult {
    let _trace = traced();
    let host = Installed::faulty("host");
    let before = host.store.record().ok_or("the first open saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::RecoveryRequired);
    let events = host.core.subscribe()?;
    drain(&events);
    assert!(
        matches!(
            host.core.set_desktop_sharing(EVERYTHING),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain grant says the store needs recovery"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DeviceAuthorized { .. }
        )),
        "no authorization is published"
    );
    assert_fenced(&host, &before, attempts + 1);
    Ok(())
}

/// A pairing whose save is uncertain stops the engine, which does not
/// remember the peer or publish its pairing.
#[test]
fn an_uncertain_pairing_save_fences_without_remembering_the_peer() -> TestResult {
    let trace = traced();
    let host = Installed::faulty("host");
    let controller = Engine::start("controller");
    let invitation = controller.core.create_invitation(LIFETIME)?;
    let before = host.store.record().ok_or("the first open saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::RecoveryRequired);
    let events = host.core.subscribe()?;
    drain(&events);
    assert!(
        matches!(
            host.core.accept_invitation(&invitation),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain pairing says the store needs recovery"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DevicePaired { .. }
        )),
        "no pairing is published"
    );
    assert_fenced(&host, &before, attempts + 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation accepted refused=store recovery required
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// Forgetting a peer whose save is uncertain stops the engine, keeping the
/// peer in the record and publishing no forgetting.
#[test]
fn an_uncertain_forget_save_fences_and_keeps_the_paired_record() -> TestResult {
    let trace = traced();
    let host = Installed::faulty("host");
    let controller = Engine::start("controller");
    host.core
        .accept_invitation(&controller.core.create_invitation(LIFETIME)?)?;
    let before = host.store.record().ok_or("the pairing saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::RecoveryRequired);
    let events = host.core.subscribe()?;
    drain(&events);
    assert!(
        matches!(
            host.core.revoke(&controller.endpoint()),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain revocation says the store needs recovery"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DeviceUnpaired { .. } | CoreEvent::DeviceRevoked { .. }
        )),
        "no forgetting is published"
    );
    assert_fenced(&host, &before, attempts + 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation accepted
          pairing: unpair requested refused=store recovery required
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A new identity whose save is uncertain stops the engine, which keeps
/// the identity it bound; reopened, the installation has that identity.
#[test]
fn an_uncertain_reset_keeps_the_saved_record_and_the_bound_secret() -> TestResult {
    let _trace = traced();
    let host = Installed::faulty("host");
    let before = host.store.record().ok_or("the first open saves a record")?;
    let secret = host.core.export_identity();
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::RecoveryRequired);
    assert!(
        matches!(
            host.core.reset_identity(),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain reset says the store needs recovery"
    );
    assert!(
        host.core.export_identity() == secret,
        "the stopped engine keeps the identity it bound"
    );
    assert_fenced(&host, &before, attempts + 1);
    let reopened = Installed::open_in("host", host.store.clone(), RelaySettings::public_n0())?;
    assert!(
        reopened.core.export_identity() == secret,
        "the installation keeps the identity"
    );
    Ok(())
}

/// Approving a controller whose save is uncertain stops the engine before
/// the controller is remembered or its pairing published.
#[test]
fn an_uncertain_control_approval_fences_before_the_peer_is_saved() -> TestResult {
    let trace = traced();
    let host = Installed::faulty("host");
    let controller = Engine::start("controller");
    exchange(&host.core, &controller.core, true);
    host.core.start_hosting()?;
    let _pending = controller
        .core
        .begin_connect_services(&host.core.endpoint_id())?;
    let request = wait_request(&host.core, Duration::from_secs(5));
    let before = host.store.record().ok_or("the first open saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.fail_next_save(Fault::RecoveryRequired);
    let events = host.core.subscribe()?;
    drain(&events);
    assert!(
        matches!(
            host.core.confirm_control_request(&request.check),
            Err(Error::StoreRecoveryRequired)
        ),
        "an uncertain approval says the store needs recovery"
    );
    assert!(
        !published(&events, |event| matches!(
            event,
            CoreEvent::DevicePaired { .. }
        )),
        "no pairing is published"
    );
    assert_fenced(&host, &before, attempts + 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: pairing file opened bytes=284
          pairing: invitation accepted
        host:
          pairing: pairing file exported allow_control=true direct=1 relays=0 ttl_s=60
          pairing: control request answered refused=store recovery required
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// A save that panics stays inside the engine: the change says the store
/// needs recovery, is not saved, and the engine stops; shutting it down
/// closes its endpoint once.
#[test]
fn a_panicking_save_fences_the_core_and_leaves_terminal_consent_unsaved() -> TestResult {
    let _trace = traced();
    let host = Installed::faulty("host");
    let before = host.store.record().ok_or("the first open saves a record")?;
    let attempts = host.store.save_attempts();
    host.store.panic_next_save();
    let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        host.core.set_service_sharing(ServiceKind::Terminal, true)
    }));
    let result = saved.map_err(|_| "the save's panic left the engine")?;
    assert!(
        matches!(result, Err(Error::StoreRecoveryRequired)),
        "a panicking save says the store needs recovery: {result:?}"
    );
    assert_eq!(
        host.store.record(),
        Some(before),
        "the terminal consent is not saved"
    );
    assert_eq!(
        host.store.save_attempts(),
        attempts + 1,
        "the save was tried once"
    );
    assert!(
        matches!(host.core.start_hosting(), Err(Error::ShutDown)),
        "the engine stopped"
    );
    assert!(
        matches!(
            host.core.set_desktop_sharing(EVERYTHING),
            Err(Error::ShutDown)
        ),
        "and changes nothing"
    );
    assert!(
        matches!(host.core.sharing_policy(), Err(Error::ShutDown)),
        "nor reports what it shares"
    );
    host.core.shutdown();
    assert_eq!(
        host.core.endpoint_close_count(),
        1,
        "the endpoint closes once"
    );
    Ok(())
}
