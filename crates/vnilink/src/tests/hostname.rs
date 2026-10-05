use std::sync::Arc;

use iroh::SecretKey;

use crate::core::Core;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::hostname::clean_hostname;
use crate::installation::{self, MemoryStore};
use crate::relay::RelaySettings;
use crate::session::SessionPhase;

use super::support::*;

fn open_loop(store: &MemoryStore, relay: RelaySettings) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), relay).expect("installation opens");
    core.set_h264_support(true, true).expect("flags");
    core
}

#[test]
fn peers_exchange_display_names_on_connect() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let host_events = host.subscribe().unwrap();
    let controller_events = controller.subscribe().unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);

    // Whatever this machine reports is what both sides store: the feature
    // is transport and storage, not any particular name.
    let own = crate::hostname::local_hostname();
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);
    assert_eq!(host.peer_hostname(&controller.endpoint_id()).unwrap(), own);
    if let Some(name) = &own {
        assert_eq!(clean_hostname(name).as_deref(), Some(name.as_str()));
    }

    // Pairing already stored the name, so the unchanged handshake is quiet.
    for events in [&host_events, &controller_events] {
        while let Some(event) = events.try_recv() {
            assert!(!matches!(event, CoreEvent::PeerRenamed { .. }));
        }
    }

    controller.disconnect().unwrap();
    wait_closed(&controller, crate::session::SessionRole::Controller);
}

#[test]
fn unknown_devices_have_no_name() {
    let core = start_direct();
    let other = start_direct();
    assert!(matches!(
        core.peer_hostname(&other.endpoint_id()),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        core.peer_hostname("not-a-key"),
        Err(Error::InvalidSetting(_))
    ));
}

#[test]
fn names_survive_a_restart_and_old_records_still_open() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.connect(&host.endpoint_id()).unwrap();
    let own = crate::hostname::local_hostname();
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);

    // Reopening the store keeps the names.
    drop(host);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(
        reopened.peer_hostname(&controller.endpoint_id()).unwrap(),
        own
    );

    // A version 1 record (no hostname section) still opens, with no names.
    let secret = SecretKey::generate();
    let peer_key = SecretKey::generate();
    let peer_id = *peer_key.public().as_bytes();
    assert_ne!(peer_id, *secret.public().as_bytes());
    let mut body = Vec::new();
    body.extend_from_slice(b"VNLK");
    body.push(1);
    body.extend_from_slice(&secret.to_bytes());
    body.push(0); // public relay
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&peer_id);
    body.push(0); // no relays
    body.push(0); // no direct addresses
    body.push(0b001); // control grant
    body.extend_from_slice(&0u16.to_be_bytes()); // empty replay cache
    let checksum = installation::checksum(&body);
    body.extend_from_slice(&checksum.to_be_bytes());
    let record = installation::decode(&body).unwrap();
    assert!(record.trust.hostname_of(&peer_id).is_none());
    let device = record.trust.get(&peer_id).unwrap();
    assert!(device.grants.control);

    // And the current encoder writes version 2, which round-trips names.
    let bytes = installation::encode(
        &secret.to_bytes(),
        &RelaySettings::public_n0(),
        &record.trust,
        &record.replay,
    )
    .unwrap();
    assert_eq!(body[4], 1);
    assert_eq!(bytes[4], 2);
    let again = installation::decode(&bytes).unwrap();
    assert!(again.trust.get(&peer_id).is_some());
}

#[test]
fn unpairing_drops_the_name_with_the_device() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.connect(&host.endpoint_id()).unwrap();
    let own = crate::hostname::local_hostname();
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);
    host.unpair(&controller.endpoint_id()).unwrap();
    // The controller still knows the host until it forgets it too.
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);
    controller.unpair(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.peer_hostname(&host.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn pairing_learns_names_before_connect_and_local_names_survive_refresh() {
    use std::time::{Duration, SystemTime};
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let own = crate::local_hostname();
    assert_eq!(host.peer_hostname(&controller.endpoint_id()).unwrap(), own);
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);
    let id = controller.endpoint_id();
    host.set_peer_pet_name(&id, Some("  Work laptop  "))
        .unwrap();
    let device = host
        .snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|d| d.endpoint_id == id)
        .unwrap();
    assert_eq!(device.pet_name.as_deref(), Some("Work laptop"));
    let old_token = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    let (invitation, _) = crate::invitation::open(&old_token, SystemTime::now()).unwrap();
    let secret = SecretKey::from_bytes(&controller.export_identity());
    let (token, _) = crate::invitation::issue(
        &secret,
        &invitation.relays,
        &invitation.direct,
        Duration::from_secs(60),
        SystemTime::now(),
        Some("renamed-laptop".into()),
    )
    .unwrap();
    host.accept_invitation(&token).unwrap();
    assert_eq!(
        host.peer_hostname(&id).unwrap().as_deref(),
        Some("renamed-laptop")
    );
    let events = host.subscribe().unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(host.peer_hostname(&id).unwrap(), own);
    let mut renamed = false;
    while let Some(event) = events.try_recv() {
        renamed |= matches!(event, CoreEvent::PeerRenamed { endpoint_id, .. } if endpoint_id == id);
    }
    assert!(renamed, "hostname refresh must notify subscribers");
    assert!(
        controller.snapshot().unwrap().devices[0].pet_name.is_none(),
        "local names never reach the peer"
    );
    controller.disconnect().unwrap();
    wait_closed(&controller, crate::session::SessionRole::Controller);
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        host.set_peer_pet_name(&id, Some("Bad save")),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(
        host.snapshot().unwrap().devices[0].pet_name.as_deref(),
        Some("Work laptop")
    );
    assert!(host.set_peer_pet_name(&id, Some("bad\nname")).is_err());
    drop(host);
    let host = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(
        host.snapshot().unwrap().devices[0].pet_name.as_deref(),
        Some("Work laptop")
    );
    host.set_peer_pet_name(&id, None).unwrap();
    assert!(host.snapshot().unwrap().devices[0].pet_name.is_none());
    host.unpair(&id).unwrap();
    assert!(matches!(
        host.set_peer_pet_name(&id, Some("gone")),
        Err(Error::NotPaired)
    ));
}
