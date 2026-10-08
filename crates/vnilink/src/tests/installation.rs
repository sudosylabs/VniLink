use std::net::SocketAddr;
use std::time::SystemTime;

use iroh::SecretKey;

use crate::error::Error;
use crate::installation;
use crate::invitation::{self, ReplayCache};
use crate::relay::{CustomRelay, RelaySettings};
use crate::session::Grants;
use crate::trust::{Device, Trust};

const TOKEN: &str = "relay-credential-alpha";

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
            terminal: false,
            camera: false,
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
            terminal: false,
            camera: false,
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

#[test]
fn service_permissions_migrate_old_records_with_new_grants_denied() {
    let owner = SecretKey::generate();
    let peer = SecretKey::generate();
    let mut trust = Trust::default();
    trust
        .pair(Device {
            endpoint_id: *peer.public().as_bytes(),
            display_id: peer.public().to_string(),
            relays: vec![],
            direct: vec![],
            grants: all_grants(),
            terminal: true,
            camera: true,
        })
        .unwrap();
    let current = installation::encode(
        &owner.to_bytes(),
        &RelaySettings::public_n0(),
        &trust,
        &ReplayCache::new(),
    )
    .unwrap();
    assert_eq!(current[4], 3);
    let decoded = installation::decode(&current).unwrap();
    assert!(decoded.trust.devices().next().unwrap().terminal);
    assert!(decoded.trust.devices().next().unwrap().camera);
    // V2 layout is unchanged, but its reserved bits must stay zero. Historical
    // control/clipboard/files grants cannot imply terminal or camera access.
    let mut old = current.clone();
    old[4] = 2;
    // 5-byte header + secret32 + public relay1 + count2 + key32 + relay/address counts2.
    let grant_offset = 5 + 32 + 1 + 2 + 32 + 2;
    old[grant_offset] &= 7;
    resign(&mut old);
    let restored = installation::decode(&old).unwrap();
    let device = restored.trust.devices().next().unwrap();
    assert_eq!(device.grants, all_grants());
    assert!(!device.terminal && !device.camera);
    old[grant_offset] |= 8;
    resign(&mut old);
    reject(&old);
    let mut unknown = current;
    unknown[grant_offset] |= 32;
    resign(&mut unknown);
    reject(&unknown);
    let payload = device_payload(&owner.to_bytes(), peer.public().as_bytes(), 7, &[], &[]);
    let restored = installation::decode(&installation::frame(&payload)).unwrap();
    let device = restored.trust.devices().next().unwrap();
    assert!(!device.terminal && !device.camera);
}
