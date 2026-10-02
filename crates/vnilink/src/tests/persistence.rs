use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iroh::{RelayMode, SecretKey};

use crate::core::{Core, CoreConfig, Listen, OpenConfig};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::installation::{self, MemoryStore};
use crate::invitation::{self, ReplayCache};
use crate::relay::{CustomRelay, RelaySettings};
use crate::session::{Grants, SessionPhase};
use crate::trust::{Device, Trust};

const TOKEN: &str = "relay-credential-alpha";

fn peer() -> Core {
    Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
        true,
    )
    .expect("loopback core")
}

fn open_loop(store: &MemoryStore, relay: RelaySettings) -> Core {
    Core::open_direct(Arc::new(store.clone()), relay).expect("installation opens")
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

#[track_caller]
fn reject(bytes: &[u8]) {
    match installation::decode(bytes) {
        Err(Error::CorruptRecord) => {}
        other => panic!("malformed record was accepted: {other:?}"),
    }
}

fn resign(bytes: &mut Vec<u8>) {
    let sum = installation::checksum(&bytes[..bytes.len() - 4]);
    let start = bytes.len() - 4;
    bytes[start..].copy_from_slice(&sum.to_be_bytes());
}

fn push_text(out: &mut Vec<u8>, text: &str) {
    let len = u16::try_from(text.len()).unwrap();
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(text.as_bytes());
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
fn reaccepting_an_invitation_resets_saved_grants() {
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
    assert_eq!(paired.grants, Grants::default());
    drop(host);

    let host = open_loop(&store, RelaySettings::public_n0());
    let device = &host.snapshot().unwrap().devices[0];
    assert_eq!(device.endpoint_id, controller.endpoint_id());
    assert_eq!(device.grants, Grants::default());
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
    assert_eq!(
        host.snapshot().unwrap().devices[0].grants,
        Grants::default()
    );
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
fn the_record_roundtrips_trust_relay_credentials_and_replay() {
    assert_eq!(installation::checksum(b"123456789"), 0xCBF4_3926);

    let owner = SecretKey::generate();
    let peer_key = SecretKey::generate();
    let addr: SocketAddr = "[2001:db8::1]:443".parse().unwrap();
    let relay_url = crate::relay::parse_relay_url("https://relay.example.com./")
        .unwrap()
        .to_string();
    let mut trust = Trust::default();
    trust
        .pair(Device {
            endpoint_id: *peer_key.public().as_bytes(),
            display_id: peer_key.public().to_string(),
            relays: vec![relay_url.clone()],
            direct: vec![addr],
            grants: all_grants(),
        })
        .unwrap();
    let now = invitation::unix_ms(SystemTime::now()).unwrap();
    let future_nonce = [7u8; 16];
    let past_nonce = [8u8; 16];
    let replay =
        ReplayCache::from_entries(vec![(future_nonce, now + 3_600_000), (past_nonce, 1)]).unwrap();
    let settings = credentialed_relay();
    let secret = owner.to_bytes();
    let bytes = installation::encode(&secret, &settings, &trust, &replay).unwrap();
    let decoded = installation::decode(&bytes).unwrap();
    assert!(decoded.secret == secret, "decode changed the secret");
    assert_eq!(decoded.relay, settings);
    let rendered = format!("{decoded:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains(TOKEN));
    assert!(!rendered.contains(&invitation::encode_hex(&secret)));
    let device = decoded.trust.devices().next().unwrap();
    assert_eq!(device.endpoint_id, *peer_key.public().as_bytes());
    assert_eq!(device.display_id, peer_key.public().to_string());
    assert_eq!(device.relays, vec![relay_url]);
    assert_eq!(device.direct, vec![addr]);
    assert_eq!(device.grants, all_grants());
    let mut restored = decoded.replay;
    assert!(matches!(
        restored.remember(future_nonce, now + 3_600_000, now),
        Err(Error::InvitationReplayed)
    ));
    assert!(restored.remember(past_nonce, now + 3_600_000, now).is_ok());

    let mut many = Trust::default();
    for _ in 0..128 {
        let peer_key = SecretKey::generate();
        many.pair(Device {
            endpoint_id: *peer_key.public().as_bytes(),
            display_id: peer_key.public().to_string(),
            relays: Vec::new(),
            direct: Vec::new(),
            grants: Grants {
                control: true,
                clipboard: false,
                transfer: true,
            },
        })
        .unwrap();
    }
    let bytes = installation::encode(
        &secret,
        &RelaySettings::public_n0(),
        &many,
        &ReplayCache::new(),
    )
    .unwrap();
    let decoded = installation::decode(&bytes).unwrap();
    assert_eq!(decoded.trust.devices().count(), 128);
    assert!(decoded
        .trust
        .devices()
        .all(|device| device.grants.control && device.grants.transfer && !device.grants.clipboard));
    assert!(decoded.relay.is_public());
}

#[test]
fn the_record_rejects_malformed_bytes() {
    let owner = SecretKey::generate();
    let secret = owner.to_bytes();
    let peer_key = SecretKey::generate();
    let peer_id = *peer_key.public().as_bytes();

    let mut empty = Vec::new();
    empty.extend_from_slice(&secret);
    empty.push(0);
    empty.extend_from_slice(&0u16.to_be_bytes());
    empty.extend_from_slice(&0u16.to_be_bytes());
    let good = installation::frame(&empty);
    assert!(installation::decode(&good).is_ok());

    let mut bad_crc = good.clone();
    let last = bad_crc.len() - 1;
    bad_crc[last] ^= 0xff;
    reject(&bad_crc);

    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    resign(&mut bad_magic);
    reject(&bad_magic);

    let mut bad_version = good.clone();
    bad_version[4] = 9;
    resign(&mut bad_version);
    reject(&bad_version);

    reject(&good[..good.len() - 5]);
    reject(&vec![0u8; 1024 * 1024 + 1]);

    let mut trailing = empty.clone();
    trailing.push(0xff);
    reject(&installation::frame(&trailing));

    let mut too_many = Vec::new();
    too_many.extend_from_slice(&secret);
    too_many.push(0);
    too_many.extend_from_slice(&129u16.to_be_bytes());
    too_many.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&too_many));

    let mut replay_overflow = Vec::new();
    replay_overflow.extend_from_slice(&secret);
    replay_overflow.push(0);
    replay_overflow.extend_from_slice(&0u16.to_be_bytes());
    replay_overflow.extend_from_slice(&65u16.to_be_bytes());
    reject(&installation::frame(&replay_overflow));

    reject(&installation::frame(&device_payload(
        &secret,
        &peer_id,
        0x08,
        &[],
        &[],
    )));
    reject(&installation::frame(&device_payload(
        &secret,
        owner.public().as_bytes(),
        0,
        &[],
        &[],
    )));
    let mut unusable = [0u8; 32];
    unusable[31] = 0xff;
    assert!(
        iroh::PublicKey::from_bytes(&unusable).is_err(),
        "this endpoint id is a real key, so the record may store it"
    );
    reject(&installation::frame(&device_payload(
        &secret,
        &unusable,
        0,
        &[],
        &[],
    )));

    let mut duplicate = Vec::new();
    duplicate.extend_from_slice(&secret);
    duplicate.push(0);
    duplicate.extend_from_slice(&2u16.to_be_bytes());
    push_device(&mut duplicate, &peer_id, 0, &[], &[]);
    push_device(&mut duplicate, &peer_id, 0, &[], &[]);
    duplicate.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&duplicate));

    let unspecified: SocketAddr = "0.0.0.0:1".parse().unwrap();
    reject(&installation::frame(&device_payload(
        &secret,
        &peer_id,
        0,
        &[],
        &[unspecified],
    )));
    let closed: SocketAddr = "127.0.0.1:0".parse().unwrap();
    reject(&installation::frame(&device_payload(
        &secret,
        &peer_id,
        0,
        &[],
        &[closed],
    )));
    let loopback: SocketAddr = "127.0.0.1:9".parse().unwrap();
    reject(&installation::frame(&device_payload(
        &secret,
        &peer_id,
        0,
        &[],
        &[loopback, loopback],
    )));

    reject(&installation::frame(&device_payload(
        &secret,
        &peer_id,
        0,
        &["https://relay.example.com./", "https://relay.example.com./"],
        &[],
    )));

    let mut bad_relay = Vec::new();
    bad_relay.extend_from_slice(&secret);
    bad_relay.push(7);
    bad_relay.extend_from_slice(&0u16.to_be_bytes());
    bad_relay.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&bad_relay));

    let mut empty_custom = Vec::new();
    empty_custom.extend_from_slice(&secret);
    empty_custom.push(1);
    empty_custom.push(0);
    empty_custom.extend_from_slice(&0u16.to_be_bytes());
    empty_custom.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&empty_custom));

    let mut insecure = Vec::new();
    insecure.extend_from_slice(&secret);
    insecure.push(1);
    insecure.push(1);
    push_text(&mut insecure, "http://relay.example.com");
    insecure.push(0);
    insecure.extend_from_slice(&0u16.to_be_bytes());
    insecure.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&insecure));

    let mut userinfo = Vec::new();
    userinfo.extend_from_slice(&secret);
    userinfo.push(1);
    userinfo.push(1);
    push_text(&mut userinfo, "https://user:secret@relay.example.com");
    userinfo.push(0);
    userinfo.extend_from_slice(&0u16.to_be_bytes());
    userinfo.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&userinfo));

    let mut spaced_token = Vec::new();
    spaced_token.extend_from_slice(&secret);
    spaced_token.push(1);
    spaced_token.push(1);
    push_text(&mut spaced_token, "https://relay.example.com./");
    spaced_token.push(1);
    push_text(&mut spaced_token, "bad token");
    spaced_token.extend_from_slice(&0u16.to_be_bytes());
    spaced_token.extend_from_slice(&0u16.to_be_bytes());
    reject(&installation::frame(&spaced_token));

    let mut nonce = Vec::new();
    nonce.extend_from_slice(&secret);
    nonce.push(0);
    nonce.extend_from_slice(&0u16.to_be_bytes());
    nonce.extend_from_slice(&2u16.to_be_bytes());
    nonce.extend_from_slice(&[9u8; 16]);
    nonce.extend_from_slice(&u64::MAX.to_be_bytes());
    nonce.extend_from_slice(&[9u8; 16]);
    nonce.extend_from_slice(&u64::MAX.to_be_bytes());
    reject(&installation::frame(&nonce));
}

fn device_payload(
    secret: &[u8; 32],
    endpoint_id: &[u8; 32],
    grants: u8,
    relays: &[&str],
    addrs: &[SocketAddr],
) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(secret);
    payload.push(0);
    payload.extend_from_slice(&1u16.to_be_bytes());
    push_device(&mut payload, endpoint_id, grants, relays, addrs);
    payload.extend_from_slice(&0u16.to_be_bytes());
    payload
}

fn push_device(
    out: &mut Vec<u8>,
    endpoint_id: &[u8; 32],
    grants: u8,
    relays: &[&str],
    addrs: &[SocketAddr],
) {
    out.extend_from_slice(endpoint_id);
    out.push(u8::try_from(relays.len()).unwrap());
    for relay in relays {
        push_text(out, relay);
    }
    out.push(u8::try_from(addrs.len()).unwrap());
    for addr in addrs {
        match addr {
            SocketAddr::V4(addr) => {
                out.push(4);
                out.extend_from_slice(&addr.ip().octets());
                out.extend_from_slice(&addr.port().to_be_bytes());
            }
            SocketAddr::V6(addr) => {
                out.push(6);
                out.extend_from_slice(&addr.ip().octets());
                out.extend_from_slice(&addr.port().to_be_bytes());
            }
        }
    }
    out.push(grants);
}
