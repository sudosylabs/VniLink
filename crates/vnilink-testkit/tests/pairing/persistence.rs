// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What an installation keeps across a restart: its identity, the peers it
//! paired with, what it granted them and the invitations it used; and what
//! a failed save leaves of a peer it was forgetting.

use expect_test::expect;
use std::time::Duration;

use vnilink::{CoreEvent, Error, OperatingSystem};
use vnilink_testkit::events::drain;
use vnilink_testkit::store::Installed;
use vnilink_testkit::{traced, Engine, TestResult};

use super::support::{credentialed_relay, EVERYTHING};

const LIFETIME: Duration = Duration::from_secs(60);

/// A restart keeps the identity, the paired peer with its grants and
/// addresses, and the saved relay over the one offered at opening; it
/// does not keep hosting or any session.
#[test]
fn restart_keeps_the_endpoint_grants_and_addresses() -> TestResult {
    let _trace = traced();
    let mut host = Installed::start("host");
    let controller = Engine::start("controller");
    host.core.start_hosting()?;
    host.core
        .accept_invitation(&controller.core.create_invitation(LIFETIME)?)?;
    host.core.set_desktop_sharing(EVERYTHING)?;
    let id = host.core.endpoint_id();
    let secret = host.core.export_identity();
    let devices = host.core.snapshot()?.devices;
    assert!(host.core.snapshot()?.hosting, "the host was hosting");
    assert_eq!(devices.len(), 1, "the host remembers the controller");
    assert_eq!(devices[0].grants, EVERYTHING, "with everything granted");
    assert!(
        !devices[0].direct.is_empty(),
        "and where the controller listens"
    );

    // A relay with a credential, offered as a first setting that a saved
    // one overrides.
    host.restart_with(credentialed_relay("relay-credential-alpha")?);
    assert_eq!(host.core.endpoint_id(), id, "a restart keeps the id");
    assert!(
        host.core.export_identity() == secret,
        "restart changed the installation secret"
    );
    let snapshot = host.core.snapshot()?;
    assert!(!snapshot.hosting, "a restart does not keep hosting");
    assert!(snapshot.incoming.is_none(), "nor an incoming session");
    assert!(snapshot.outgoing.is_none(), "nor an outgoing session");
    assert!(
        snapshot.public_relay,
        "the saved relay wins over the one offered"
    );
    assert_eq!(
        snapshot.devices, devices,
        "a restart keeps the peer, its grants and addresses"
    );
    assert_eq!(
        format!("{:?}", host.store),
        "MemoryStore",
        "a store's Debug shows nothing it holds"
    );
    Ok(())
}

/// An invitation used once is refused after a restart too.
#[test]
fn a_used_invitation_cannot_be_replayed_after_restart() -> TestResult {
    let trace = traced();
    let mut host = Engine::start("host");
    let controller = Engine::start("controller");
    let invitation = controller.core.create_invitation(LIFETIME)?;
    host.core.accept_invitation(&invitation)?;
    host.restart();
    assert!(
        matches!(
            host.core.accept_invitation(&invitation),
            Err(Error::InvitationReplayed)
        ),
        "a used invitation stays used across a restart"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          pairing: invitation issued direct=1 relays=0 ttl_s=60
        host:
          pairing: invitation accepted
          pairing: invitation accepted refused=invitation replayed
    "#]],
        || trace.story(&["pairing"]),
    );
    Ok(())
}

/// Accepting a peer's newer invitation keeps what was granted it, and so
/// does a restart.
#[test]
fn reaccepting_an_invitation_keeps_saved_grants() -> TestResult {
    let _trace = traced();
    let mut host = Engine::start("host");
    let controller = Engine::start("controller");
    host.core
        .accept_invitation(&controller.core.create_invitation(LIFETIME)?)?;
    host.core.set_desktop_sharing(EVERYTHING)?;
    let paired = host
        .core
        .accept_invitation(&controller.core.create_invitation(LIFETIME)?)?;
    assert_eq!(
        paired.grants, EVERYTHING,
        "a newer invitation keeps the grants"
    );
    host.restart();
    let device = &host.core.snapshot()?.devices[0];
    assert_eq!(
        device.endpoint_id,
        controller.endpoint(),
        "the restarted host remembers the controller"
    );
    assert_eq!(device.grants, EVERYTHING, "with its grants");
    Ok(())
}

/// Forgetting a peer whose save fails keeps the peer exactly as it was,
/// nickname and system included, and publishes nothing; forgetting it
/// again is published and survives a restart.
#[test]
fn unpair_removal_and_rollback_persist() -> TestResult {
    let _trace = traced();
    let mut host = Installed::start("host");
    let controller = Engine::start("controller");
    let events = host.core.subscribe()?;
    host.core.start_hosting()?;
    host.core
        .accept_invitation(&controller.core.create_invitation(LIFETIME)?)?;
    assert_eq!(
        host.core.snapshot()?.devices.len(),
        1,
        "the host remembers the controller"
    );
    host.core
        .set_peer_pet_name(&controller.endpoint(), Some("Office computer"))?;
    let before = host.core.snapshot()?.devices;
    assert_eq!(
        before[0].os,
        Some(OperatingSystem::local()),
        "the invitation told the controller's system"
    );
    // A failed save restores the entire identity and metadata, and emits nothing.
    host.store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(
        matches!(
            host.core.unpair(&controller.endpoint()),
            Err(Error::StoreLocked)
        ),
        "forgetting whose save fails is refused"
    );
    assert_eq!(
        host.core.snapshot()?.devices,
        before,
        "a failed save keeps the peer as it was"
    );
    assert!(
        !drain(&events)
            .iter()
            .any(|event| matches!(event, CoreEvent::DeviceUnpaired { .. })),
        "a failed save publishes no forgetting"
    );

    // Removal survives a reopen.
    host.core.unpair(&controller.endpoint())?;
    assert!(
        drain(&events)
            .iter()
            .any(|event| matches!(event, CoreEvent::DeviceUnpaired { .. })),
        "forgetting is published"
    );
    host.restart();
    assert!(
        host.core.snapshot()?.devices.is_empty(),
        "forgetting survives a restart"
    );
    assert!(
        matches!(
            host.core.unpair(&controller.endpoint()),
            Err(Error::NotPaired)
        ),
        "a forgotten peer cannot be forgotten again"
    );
    Ok(())
}
