use crate::{
    installation,
    invitation::ReplayCache,
    relay::{CustomRelay, RelaySettings},
    trust::{Device, Trust},
    Error, OperatingSystem, SharingPolicy,
};
use iroh::SecretKey;

fn encode(secret: &[u8; 32], trust: &Trust) -> Vec<u8> {
    installation::encode(
        secret,
        &RelaySettings::custom(vec![CustomRelay::new("https://relay.example.com")
            .unwrap()
            .with_auth_token("private-token")
            .unwrap()])
        .unwrap(),
        trust,
        &ReplayCache::new(),
        &crate::terminal::TerminalPolicy::default(),
        &crate::job::FileRetentionPolicy::default(),
        &crate::power::PowerPolicy::default(),
        &crate::job::FileResourcePolicy::default(),
        &crate::system::SystemPolicy::default(),
    )
    .unwrap()
}
fn resign(bytes: &mut [u8]) {
    let end = bytes.len() - 4;
    let checksum = installation::checksum(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_be_bytes());
}
fn peer(trust: &mut Trust) -> [u8; 32] {
    let key = SecretKey::generate().public();
    let id = *key.as_bytes();
    trust
        .pair(Device {
            endpoint_id: id,
            display_id: key.to_string(),
            relays: vec!["https://peer.example.com".into()],
            direct: vec!["127.0.0.1:48123".parse().unwrap()],
        })
        .unwrap();
    trust.set_hostname(&id, "Remote computer".into()).unwrap();
    trust.set_pet_name(&id, "My computer".into()).unwrap();
    trust.set_os(&id, OperatingSystem::Windows).unwrap();
    id
}
#[test]
fn installation_roundtrips_global_policy_and_authenticated_device_metadata() {
    assert_eq!(installation::checksum(b"123456789"), 0xCBF4_3926);
    let secret = SecretKey::generate().to_bytes();
    let mut trust = Trust::default();
    let id = peer(&mut trust);
    let other = peer(&mut trust);
    for bits in 0..1024 {
        trust.sharing = SharingPolicy::from_bits(bits).unwrap();
        let decoded = installation::decode(&encode(&secret, &trust)).unwrap();
        assert_eq!(decoded.secret, secret);
        assert_eq!(decoded.trust.sharing, trust.sharing);
        assert_eq!(
            decoded.trust.service_permissions(&id).unwrap(),
            decoded.trust.service_permissions(&other).unwrap()
        );
        assert_eq!(decoded.trust.os_of(&id), Some(OperatingSystem::Windows));
        assert_eq!(
            decoded.trust.hostname_of(&id).as_deref(),
            Some("Remote computer")
        );
        assert_eq!(
            decoded.trust.pet_name_of(&id).as_deref(),
            Some("My computer")
        );
        let debug = format!("{decoded:?}");
        assert!(!debug.contains("private-token"));
        assert!(!debug.contains(&crate::invitation::encode_hex(&secret)));
    }
}
#[test]
fn installation_rejects_truncation_corruption_and_reserved_policy_bits() {
    let secret = SecretKey::generate().to_bytes();
    let mut trust = Trust::default();
    peer(&mut trust);
    let bytes = encode(&secret, &trust);
    for length in 0..bytes.len() {
        assert!(matches!(
            installation::decode(&bytes[..length]),
            Err(Error::CorruptRecord)
        ));
    }
    for index in 0..bytes.len() {
        let mut invalid = bytes.clone();
        invalid[index] ^= 128;
        assert!(matches!(
            installation::decode(&invalid),
            Err(Error::CorruptRecord)
        ));
    }
    let mut invalid = bytes.clone();
    let end = invalid.len();
    invalid[end - 6] = 0xff;
    resign(&mut invalid);
    assert!(matches!(
        installation::decode(&invalid),
        Err(Error::CorruptRecord)
    ));
}
#[test]
fn installation_rejects_invalid_metadata_and_self_identity_even_with_valid_checksum() {
    let secret = SecretKey::generate().to_bytes();
    let mut trust = Trust::default();
    let id = peer(&mut trust);
    let bytes = encode(&secret, &trust);
    let positions: Vec<_> = bytes
        .windows(32)
        .enumerate()
        .filter_map(|(index, value)| (value == id).then_some(index))
        .collect();
    let device_position = positions[0];
    let os_position = positions[positions.len() - 1] + 32;
    let mut invalid = bytes.clone();
    invalid[os_position] = 99;
    resign(&mut invalid);
    assert!(matches!(
        installation::decode(&invalid),
        Err(Error::CorruptRecord)
    ));
    let mut invalid = bytes.clone();
    invalid[device_position..device_position + 32]
        .copy_from_slice(SecretKey::from_bytes(&secret).public().as_bytes());
    resign(&mut invalid);
    assert!(matches!(
        installation::decode(&invalid),
        Err(Error::CorruptRecord)
    ));
}
