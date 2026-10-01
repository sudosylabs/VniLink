use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::SecretKey;

use crate::error::Error;
use crate::invitation::{
    canonicalize_addrs, decode_hex, encode_hex, issue, open, unix_ms, ReplayCache,
    MAX_DIRECT_ADDRS, MAX_INVITATION_CHARS, MAX_REPLAY, MAX_TTL, PREFIX,
};

#[test]
fn a_signed_invitation_opens_and_a_tampered_one_does_not() {
    let secret = SecretKey::generate();
    let relays = vec!["https://relay.example.com./".to_owned()];
    let direct = vec!["127.0.0.1:9".parse().unwrap()];
    let (token, _) = issue(
        &secret,
        &relays,
        &direct,
        Duration::from_secs(60),
        SystemTime::now(),
    )
    .unwrap();
    let (opened, _) = open(&token, SystemTime::now()).unwrap();
    assert_eq!(opened.endpoint_id, *secret.public().as_bytes());
    assert_eq!(opened.relays, relays);
    assert!(!format!("{opened:?}").contains("relay.example"));

    let mut chars: Vec<u8> = token.into_bytes();
    let flip = chars.len() - 2;
    chars[flip] = if chars[flip] == b'a' { b'b' } else { b'a' };
    let tampered = String::from_utf8(chars).unwrap();
    assert!(matches!(
        open(&tampered, SystemTime::now()),
        Err(Error::InvalidInvitation(_))
    ));
}

#[test]
fn expired_and_replayed_invitations_are_rejected() {
    let secret = SecretKey::generate();
    let now = SystemTime::now();
    let (token, _) = issue(&secret, &[], &[], Duration::from_secs(1), now).unwrap();
    let later = now + Duration::from_secs(2);
    assert!(matches!(open(&token, later), Err(Error::InvitationExpired)));

    let (token, invitation) = issue(&secret, &[], &[], Duration::from_secs(60), now).unwrap();
    let (_, nonce) = open(&token, now).unwrap();
    let mut cache = ReplayCache::new();
    cache
        .remember(nonce, invitation.expires_at_ms, unix_ms(now).unwrap())
        .unwrap();
    assert!(matches!(
        cache.remember(nonce, invitation.expires_at_ms, unix_ms(now).unwrap()),
        Err(Error::InvitationReplayed)
    ));
}

#[test]
fn lifetime_must_be_between_one_second_and_fifteen_minutes() {
    let secret = SecretKey::generate();
    let now = SystemTime::now();
    assert!(matches!(
        issue(&secret, &[], &[], Duration::ZERO, now),
        Err(Error::InvalidInvitation(
            "lifetime must be between 1 second and 15 minutes"
        ))
    ));
    assert!(matches!(
        issue(&secret, &[], &[], Duration::from_millis(999), now),
        Err(Error::InvalidInvitation(
            "lifetime must be between 1 second and 15 minutes"
        ))
    ));
    assert!(issue(&secret, &[], &[], Duration::from_secs(1), now).is_ok());
    assert!(issue(&secret, &[], &[], MAX_TTL, now).is_ok());
    assert!(matches!(
        issue(&secret, &[], &[], MAX_TTL + Duration::from_secs(1), now,),
        Err(Error::InvalidInvitation(
            "lifetime must be between 1 second and 15 minutes"
        ))
    ));
    let before = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
    assert!(matches!(
        issue(&secret, &[], &[], Duration::from_secs(60), before),
        Err(Error::InvalidInvitation(
            "system clock is before the unix epoch"
        ))
    ));
}

#[test]
fn uppercase_hex_opens_and_broken_tokens_do_not() {
    let secret = SecretKey::generate();
    let now = SystemTime::now();
    let (token, invitation) = issue(
        &secret,
        &["https://relay.example.com".to_owned()],
        &["[::1]:9".parse().unwrap()],
        Duration::from_secs(60),
        now,
    )
    .unwrap();
    let upper = format!("{PREFIX}{}", token[PREFIX.len()..].to_ascii_uppercase());
    let (opened, _) = open(&upper, now).unwrap();
    assert_eq!(opened.endpoint_id, invitation.endpoint_id);
    assert_eq!(opened.expires_at_ms, invitation.expires_at_ms);
    assert_eq!(opened.direct, vec!["[::1]:9".parse().unwrap()]);
    assert_eq!(opened.relays.len(), 1);
    assert!(opened.relays[0].contains("relay.example.com"));

    assert!(matches!(
        open("", now),
        Err(Error::InvalidInvitation("unrecognized invitation"))
    ));
    assert!(matches!(
        open("vnl1.abc", now),
        Err(Error::InvalidInvitation("unrecognized invitation"))
    ));
    assert!(matches!(
        open("vnl1.zz", now),
        Err(Error::InvalidInvitation("unrecognized invitation"))
    ));
    assert!(matches!(
        open(&token[PREFIX.len()..], now),
        Err(Error::InvalidInvitation("unrecognized invitation"))
    ));
    let too_long = format!("{PREFIX}{}", "ab".repeat(9_000));
    assert!(too_long.len() > MAX_INVITATION_CHARS);
    assert!(matches!(
        open(&too_long, now),
        Err(Error::InvalidInvitation("unrecognized invitation"))
    ));

    let mut body = decode_hex(&token[PREFIX.len()..]).unwrap();
    body.push(0);
    let trailed = format!("{PREFIX}{}", encode_hex(&body));
    assert!(matches!(
        open(&trailed, now),
        Err(Error::InvalidInvitation("trailing bytes"))
    ));

    let mut unknown_family = vec![0u8; 32 + 16 + 8];
    unknown_family.push(0);
    unknown_family.push(1);
    unknown_family.push(9);
    unknown_family.resize(32 + 16 + 8 + 1 + 1 + 64, 0);
    assert!(matches!(
        open(&format!("{PREFIX}{}", encode_hex(&unknown_family)), now),
        Err(Error::InvalidInvitation("unknown address family"))
    ));

    let mut undialable = vec![0u8; 32 + 16];
    undialable.extend_from_slice(&u64::MAX.to_be_bytes());
    undialable.push(0);
    undialable.push(1);
    undialable.push(4);
    undialable.extend_from_slice(&[0, 0, 0, 0, 0, 1]);
    undialable.extend_from_slice(&[0u8; 64]);
    assert!(matches!(
        open(&format!("{PREFIX}{}", encode_hex(&undialable)), now),
        Err(Error::InvalidInvitation("direct address is not dialable"))
    ));
}

#[test]
fn direct_addresses_are_canonicalized_and_extra_relays_are_rejected() {
    let secret = SecretKey::generate();
    let now = SystemTime::now();
    let addrs = vec![
        "0.0.0.0:9".parse().unwrap(),
        "127.0.0.1:0".parse().unwrap(),
        "10.0.0.2:9".parse().unwrap(),
        "10.0.0.1:9".parse().unwrap(),
        "10.0.0.1:9".parse().unwrap(),
    ];
    let canonical = canonicalize_addrs(&addrs).unwrap();
    assert_eq!(
        canonical,
        vec!["10.0.0.1:9".parse().unwrap(), "10.0.0.2:9".parse().unwrap(),]
    );

    let many: Vec<SocketAddr> = (1..=9)
        .map(|octet| SocketAddr::from(([10, 0, 0, octet], 9)))
        .collect();
    let (_, invitation) = issue(&secret, &[], &many, Duration::from_secs(60), now).unwrap();
    assert_eq!(invitation.direct.len(), MAX_DIRECT_ADDRS);
    assert!(!invitation
        .direct
        .iter()
        .any(|addr| addr == &SocketAddr::from(([10, 0, 0, 9], 9))));

    let relays: Vec<String> = (0..9)
        .map(|index| format!("https://relay{index}.example.com"))
        .collect();
    assert!(matches!(
        issue(&secret, &relays, &[], Duration::from_secs(60), now),
        Err(Error::InvalidInvitation("too many relay URLs"))
    ));
    assert!(issue(
        &secret,
        &["http://relay.example.com".to_owned()],
        &[],
        Duration::from_secs(60),
        now,
    )
    .is_err());
}

#[test]
fn replay_cache_drops_expired_entries_and_rejects_a_full_cache() {
    let mut cache = ReplayCache::new();
    let now = 1_000u64;
    for index in 0..MAX_REPLAY {
        let mut nonce = [0u8; 16];
        nonce[0] = index as u8;
        nonce[1] = (index / 256) as u8;
        cache.remember(nonce, now + 5_000, now).unwrap();
    }
    let replayed = [0u8; 16];
    assert!(matches!(
        cache.remember(replayed, now + 5_000, now),
        Err(Error::InvitationReplayed)
    ));
    let mut extra = [9u8; 16];
    assert!(matches!(
        cache.remember(extra, now + 5_000, now),
        Err(Error::InvitationCacheFull)
    ));
    extra[0] = 8;
    cache.remember(extra, now + 20_000, now + 5_000).unwrap();
}
