use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen, OpenConfig};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::installation::{self, MemoryStore};
use crate::invitation::{self};
use crate::relay::{CustomRelay, RelaySettings};
use crate::session::{Grants, SessionPhase};

const TOKEN: &str = "relay-credential-alpha";

fn peer() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_h264_support(true, true).expect("flags");
    core
}

fn open_loop(store: &MemoryStore, relay: RelaySettings) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), relay).expect("installation opens");
    core.set_h264_support(true, true).expect("flags");
    core
}

fn credentialed_relay() -> RelaySettings {
    let relay = CustomRelay::new("https://relay.example.com./")
        .unwrap()
        .with_auth_token(TOKEN)
        .unwrap();
    RelaySettings::custom(vec![relay]).unwrap()
}

fn all_grants() -> Grants {
    Grants {
        control: true,
        clipboard: true,
        transfer: true,
    }
}

fn drain(events: &EventSubscription) {
    while events.try_recv().is_some() {}
}

fn saw(events: &EventSubscription, kind: fn(&CoreEvent) -> bool) -> bool {
    let mut found = false;
    while let Some(event) = events.try_recv() {
        if kind(&event) {
            found = true;
        }
    }
    found
}

#[test]
fn restart_keeps_the_endpoint_grants_and_addresses() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    host.start_hosting().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    host.authorize(&controller.endpoint_id(), all_grants())
        .unwrap();
    let id = host.endpoint_id();
    let secret = host.export_identity();
    let devices = host.snapshot().unwrap().devices;
    assert!(host.snapshot().unwrap().hosting);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].grants, all_grants());
    assert!(!devices[0].direct.is_empty());
    drop(host);

    let host = open_loop(&store, credentialed_relay());
    assert_eq!(host.endpoint_id(), id);
    assert!(
        host.export_identity() == secret,
        "restart changed the installation secret"
    );
    let snapshot = host.snapshot().unwrap();
    assert!(!snapshot.hosting);
    assert!(snapshot.incoming.is_none());
    assert!(snapshot.outgoing.is_none());
    assert!(snapshot.public_relay);
    assert_eq!(snapshot.devices, devices);
    assert_eq!(format!("{store:?}"), "MemoryStore");
}

#[test]
fn a_used_invitation_cannot_be_replayed_after_restart() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    drop(host);
    let host = open_loop(&store, RelaySettings::public_n0());
    assert!(matches!(
        host.accept_invitation(&invitation),
        Err(Error::InvitationReplayed)
    ));
}

#[test]
fn reaccepting_an_invitation_keeps_saved_grants() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let first = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&first).unwrap();
    host.authorize(&controller.endpoint_id(), all_grants())
        .unwrap();
    let second = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    let paired = host.accept_invitation(&second).unwrap();
    assert_eq!(paired.grants, all_grants());
    drop(host);

    let host = open_loop(&store, RelaySettings::public_n0());
    let device = &host.snapshot().unwrap().devices[0];
    assert_eq!(device.endpoint_id, controller.endpoint_id());
    assert_eq!(device.grants, all_grants());
}

#[test]
fn locked_and_unavailable_stores_fail_without_writing() {
    assert_eq!(
        Error::StoreLocked.to_string(),
        "the credential store is locked"
    );
    assert_eq!(
        Error::StoreUnavailable.to_string(),
        "the credential store is unavailable"
    );
    assert_eq!(
        Error::CorruptRecord.to_string(),
        "the installation record is not valid"
    );

    let locked = MemoryStore::locked();
    assert!(matches!(
        Core::open_direct(Arc::new(locked.clone()), RelaySettings::public_n0()),
        Err(Error::StoreLocked)
    ));
    assert!(locked.record().is_none());

    let unavailable = MemoryStore::unavailable();
    assert!(matches!(
        Core::open_direct(Arc::new(unavailable.clone()), RelaySettings::public_n0()),
        Err(Error::StoreUnavailable)
    ));
    assert!(unavailable.record().is_none());
}

#[test]
fn a_corrupt_record_is_left_unchanged() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let id = host.endpoint_id();
    drop(host);
    let good = store.record().unwrap();
    let mut corrupt = good.clone();
    corrupt[10] ^= 0x5a;
    let broken = MemoryStore::containing(corrupt.clone());
    assert!(matches!(
        Core::open_direct(Arc::new(broken.clone()), RelaySettings::public_n0()),
        Err(Error::CorruptRecord)
    ));
    assert_eq!(broken.record().as_deref(), Some(corrupt.as_slice()));

    let host = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(host.endpoint_id(), id);
    assert_eq!(store.record().as_deref(), Some(good.as_slice()));
}

#[test]
fn the_first_save_failure_does_not_keep_an_identity() {
    let store = MemoryStore::empty();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()),
        Err(Error::StoreUnavailable)
    ));
    assert!(store.record().is_none());

    let host = open_loop(&store, RelaySettings::public_n0());
    let id = host.endpoint_id();
    drop(host);
    assert_eq!(
        open_loop(&store, RelaySettings::public_n0()).endpoint_id(),
        id
    );
}

#[test]
fn reset_identity_forgets_trust_and_keeps_the_saved_relay() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(120))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    host.authorize(&controller.endpoint_id(), all_grants())
        .unwrap();
    let settings = credentialed_relay();
    host.set_relay(settings.clone()).unwrap();
    assert_eq!(host.relay_configuration().unwrap(), settings);
    assert!(!format!("{:?}", host.relay_configuration().unwrap()).contains(TOKEN));
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
    assert!(host.snapshot().unwrap().public_relay);
    let issued = host.create_invitation(Duration::from_secs(60)).unwrap();
    let (opened, _) = invitation::open(&issued, SystemTime::now()).unwrap();
    assert!(opened.relays.is_empty());
    assert!(!issued.contains(TOKEN));
    let old_id = host.endpoint_id();
    let old_secret = host.export_identity();
    host.reset_identity().unwrap();
    assert!(matches!(host.snapshot(), Err(Error::ShutDown)));
    assert_eq!(host.endpoint_id(), old_id);
    assert!(
        host.export_identity() == old_secret,
        "the stopped core dropped the key it bound"
    );
    drop(host);

    let host = open_loop(&store, RelaySettings::public_n0());
    assert_ne!(host.endpoint_id(), old_id);
    assert!(
        host.export_identity() != old_secret,
        "reset kept the previous identity"
    );
    let snapshot = host.snapshot().unwrap();
    assert!(snapshot.devices.is_empty());
    assert!(!snapshot.public_relay);
    assert_eq!(snapshot.relay_urls, settings.relay_urls().unwrap());
    assert!(!format!("{snapshot:?}").contains(TOKEN));
    assert_eq!(format!("{store:?}"), "MemoryStore");
    let saved = installation::decode(store.record().as_ref().unwrap()).unwrap();
    assert_eq!(saved.relay, settings);
    let rendered = format!("{saved:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains(TOKEN));
    let config = OpenConfig {
        initial_relay: settings,
        ..OpenConfig::default()
    };
    assert!(!format!("{config:?}").contains(TOKEN));

    let paired = host.accept_invitation(&invitation).unwrap();
    assert_eq!(paired.grants, Grants::default());
    assert_eq!(paired.endpoint_id, controller.endpoint_id());
}

#[test]
fn a_failed_save_keeps_the_previous_record() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_relay(credentialed_relay()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(store.record().as_deref(), Some(before.as_slice()));
    assert!(host.snapshot().unwrap().public_relay);

    let controller = peer();
    host.start_hosting().unwrap();
    let events = host.subscribe().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.accept_invitation(&invitation),
        Err(Error::StoreLocked)
    ));
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DevicePaired { .. })
    }));
    host.accept_invitation(&invitation).unwrap();

    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.authorize(&controller.endpoint_id(), all_grants()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(
        host.snapshot().unwrap().devices[0].grants,
        Grants::default()
    );
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceAuthorized { .. })
    }));
    host.authorize(&controller.endpoint_id(), all_grants())
        .unwrap();

    let again = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        host.accept_invitation(&again),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(host.snapshot().unwrap().devices[0].grants, all_grants());
    host.accept_invitation(&again).unwrap();
    assert_eq!(host.snapshot().unwrap().devices[0].grants, all_grants());
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            control: true,
            clipboard: false,
            transfer: false,
        },
    )
    .unwrap();

    let host_invitation = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&host_invitation).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    let generation = host.snapshot().unwrap().incoming.unwrap().generation;
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.revoke(&controller.endpoint_id()),
        Err(Error::StoreLocked)
    ));
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    assert_eq!(incoming.phase, SessionPhase::Active);
    assert_eq!(incoming.generation, generation);
    assert!(host.snapshot().unwrap().devices[0].grants.control);
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceRevoked { .. })
    }));

    host.revoke(&controller.endpoint_id()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let phase = host
            .snapshot()
            .unwrap()
            .incoming
            .as_ref()
            .map(|session| session.phase.clone());
        if phase.as_ref() == Some(&SessionPhase::Closed) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("revoke did not stop the session: {phase:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_core_without_a_store_does_not_save() {
    let core = peer();
    let id = core.endpoint_id();
    assert!(matches!(
        core.set_relay(RelaySettings::public_n0()),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        core.reset_identity(),
        Err(Error::InvalidSetting(_))
    ));
    assert_eq!(core.endpoint_id(), id);
    assert!(core.snapshot().is_ok());
    assert!(core.snapshot().unwrap().devices.is_empty());
}

#[test]
fn unpair_removal_and_rollback_persist() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let events = host.subscribe().unwrap();
    host.start_hosting().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);

    // A failed save restores the device and emits nothing.
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.unpair(&controller.endpoint_id()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceUnpaired { .. })
    }));

    // Removal survives a reopen.
    host.unpair(&controller.endpoint_id()).unwrap();
    assert!(saw(&events, |event| {
        matches!(event, CoreEvent::DeviceUnpaired { .. })
    }));
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert!(reopened.snapshot().unwrap().devices.is_empty());
    assert!(matches!(
        reopened.unpair(&controller.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn service_permissions_are_independent_persisted_and_rollback_before_revocation() {
    use crate::{ServiceKind as Kind, ServicePermissions};
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    let id = controller.endpoint_id();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions::default()
    );
    host.authorize_service(&id, Kind::Terminal, true).unwrap();
    host.authorize_service(&id, Kind::Camera, true).unwrap();
    host.authorize(&id, all_grants()).unwrap();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: true,
            terminal: true,
            camera: true
        }
    );
    host.authorize_service(&id, Kind::Files, false).unwrap();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true
        }
    );
    assert!(host.snapshot().unwrap().devices[0].grants.clipboard);
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.authorize_service(&id, Kind::Terminal, false),
        Err(Error::StoreLocked)
    ));
    assert!(host.service_permissions(&id).unwrap().terminal);
    assert_eq!(store.record().unwrap(), saved);
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(host.revoke(&id), Err(Error::StoreUnavailable)));
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true
        }
    );
    assert_eq!(store.record().unwrap(), saved);
    // Invitation refresh preserves the explicit grants, rather than treating
    // a peer metadata refresh as permission removal.
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    assert!(host.service_permissions(&id).unwrap().camera);
    host.shutdown();
    drop(host);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(
        reopened.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true
        }
    );
    reopened.revoke(&id).unwrap();
    assert_eq!(
        reopened.service_permissions(&id).unwrap(),
        ServicePermissions::default()
    );
    reopened.unpair(&id).unwrap();
    assert!(matches!(
        reopened.service_permissions(&id),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        reopened.authorize_service(&id, Kind::Camera, true),
        Err(Error::NotPaired)
    ));
}
