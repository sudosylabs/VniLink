// trace: records every ignored failure
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey, Signature};
use rand::RngExt;

use crate::codec::{read_array, read_u8, write_sockaddr};
use crate::error::Error;
use crate::relay::parse_relay_url;

pub(crate) const PREFIX: &str = "vnl1.";
const DOMAIN: &[u8] = b"vnilink-invitation-v1";
pub(crate) const MAX_INVITATION_CHARS: usize = 16_384;
pub(crate) const MAX_DIRECT_ADDRS: usize = 8;
pub(crate) const MAX_TTL: Duration = Duration::from_secs(15 * 60);
pub(crate) const MAX_REPLAY: usize = 64;

/// Signed pairing payload. Possession is not logged. Pairing does not grant control.
///
/// `hostname` is the writer's display name for the reader's device list, so
/// a paired computer is named before the first session. Display-only: it
/// never affects grants, addresses, or admission.
#[derive(Clone, PartialEq, Eq)]
pub struct Invitation {
    pub endpoint_id: [u8; 32],
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
    pub expires_at_ms: u64,
    pub hostname: Option<String>,
    pub os: crate::OperatingSystem,
}

impl std::fmt::Debug for Invitation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Invitation([redacted])")
    }
}

#[derive(Clone)]
pub(crate) struct ReplayCache {
    entries: Vec<([u8; 16], u64)>,
}

impl ReplayCache {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub(crate) fn remember(
        &mut self,
        nonce: [u8; 16],
        expires_at_ms: u64,
        now_ms: u64,
    ) -> Result<(), Error> {
        self.entries.retain(|(_, expiry)| *expiry > now_ms);
        if self.entries.iter().any(|(seen, _)| seen == &nonce) {
            return Err(Error::InvitationReplayed);
        }
        if self.entries.len() >= MAX_REPLAY {
            return Err(Error::InvitationCacheFull);
        }
        self.entries.push((nonce, expires_at_ms));
        Ok(())
    }

    pub(crate) fn entries(&self) -> &[([u8; 16], u64)] {
        &self.entries
    }

    pub(crate) fn from_entries(entries: Vec<([u8; 16], u64)>) -> Result<Self, Error> {
        if entries.len() > MAX_REPLAY {
            return Err(Error::CorruptRecord);
        }
        let mut seen = Vec::with_capacity(entries.len());
        for (nonce, _) in &entries {
            if seen.contains(nonce) {
                return Err(Error::CorruptRecord);
            }
            seen.push(*nonce);
        }
        Ok(Self { entries })
    }
}

pub(crate) fn issue(
    secret: &SecretKey,
    relays: &[String],
    direct: &[SocketAddr],
    ttl: Duration,
    now: SystemTime,
    hostname: Option<String>,
) -> Result<(String, Invitation), Error> {
    if ttl < Duration::from_secs(1) || ttl > MAX_TTL {
        return Err(Error::InvalidInvitation(
            "lifetime must be between 1 second and 15 minutes",
        ));
    }
    let nonce: [u8; 16] = rand::rng().random();
    let expires_at_ms = unix_ms(now)? + ttl.as_millis() as u64;
    let invitation = Invitation {
        endpoint_id: *secret.public().as_bytes(),
        relays: relays.to_vec(),
        direct: canonicalize_addrs(direct)?,
        expires_at_ms,
        os: crate::OperatingSystem::local(),
        hostname: hostname
            .filter(|name| crate::hostname::clean_hostname(name).as_deref() == Some(name.as_str())),
    };
    let token = seal(secret, &invitation, nonce)?;
    Ok((token, invitation))
}

pub(crate) fn open(token: &str, now: SystemTime) -> Result<(Invitation, [u8; 16]), Error> {
    if token.len() > MAX_INVITATION_CHARS || !token.starts_with(PREFIX) {
        return Err(Error::InvalidInvitation("unrecognized invitation"));
    }
    let bytes = decode_hex(&token[PREFIX.len()..])?;
    let (invitation, nonce, signature) = parse(&bytes)?;
    if unix_ms(now)? >= invitation.expires_at_ms {
        return Err(Error::InvitationExpired);
    }
    let key = PublicKey::from_bytes(&invitation.endpoint_id)
        .map_err(|_| Error::InvalidInvitation("endpoint id is not a valid key"))?;
    let signed = signing_bytes(&invitation, nonce);
    key.verify(&signed, &signature)
        .map_err(|_| Error::InvalidInvitation("signature does not match"))?;
    Ok((invitation, nonce))
}

fn seal(secret: &SecretKey, invitation: &Invitation, nonce: [u8; 16]) -> Result<String, Error> {
    let signature = secret.sign(&signing_bytes(invitation, nonce));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&invitation.endpoint_id);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&invitation.expires_at_ms.to_be_bytes());
    write_relays(&mut bytes, &invitation.relays)?;
    write_addrs(&mut bytes, &invitation.direct)?;
    write_hostname(&mut bytes, &invitation.hostname);
    bytes.push(invitation.os.encode());
    bytes.extend_from_slice(&signature.to_bytes());
    Ok(format!("{PREFIX}{}", encode_hex(&bytes)))
}

fn parse(bytes: &[u8]) -> Result<(Invitation, [u8; 16], Signature), Error> {
    if bytes.len() < 32 + 16 + 8 + 1 + 1 + 64 {
        return Err(Error::InvalidInvitation("truncated invitation"));
    }
    let mut cursor = 0;
    let truncated = || Error::InvalidInvitation("truncated invitation");
    let endpoint_id = read_array::<32>(bytes, &mut cursor).ok_or_else(truncated)?;
    let nonce = read_array::<16>(bytes, &mut cursor).ok_or_else(truncated)?;
    let expires_at_ms =
        u64::from_be_bytes(read_array::<8>(bytes, &mut cursor).ok_or_else(truncated)?);
    let relays = read_relays(bytes, &mut cursor)?;
    let direct = read_addrs(bytes, &mut cursor)?;
    let hostname = read_hostname(bytes, &mut cursor)?;
    let os = crate::OperatingSystem::decode(read_u8(bytes, &mut cursor).ok_or_else(truncated)?)
        .map_err(|_| Error::InvalidInvitation("invalid operating system"))?;
    let signature =
        Signature::from_bytes(&read_array::<64>(bytes, &mut cursor).ok_or_else(truncated)?);
    if cursor != bytes.len() {
        return Err(Error::InvalidInvitation("trailing bytes"));
    }
    Ok((
        Invitation {
            endpoint_id,
            relays,
            direct,
            expires_at_ms,
            hostname,
            os,
        },
        nonce,
        signature,
    ))
}

/// Appends the writer's display name. Only a valid name is written;
/// anything else encodes as absent, so issuance never fails over a label.
fn write_hostname(bytes: &mut Vec<u8>, hostname: &Option<String>) {
    let valid = hostname.as_deref().filter(|name| {
        name.len() <= crate::hostname::MAX_HOSTNAME_BYTES
            && crate::hostname::clean_hostname(name).as_deref() == Some(*name)
    });
    match valid {
        Some(name) => {
            bytes.push(name.len() as u8);
            bytes.extend_from_slice(name.as_bytes());
        }
        None => bytes.push(0),
    }
}

/// Reads the writer's display name. Structure stays strict, content stays
/// lenient: a name that fails display validation becomes absent rather than
/// failing a pairing two humans are mid-flow through.
fn read_hostname(bytes: &[u8], cursor: &mut usize) -> Result<Option<String>, Error> {
    let truncated = || Error::InvalidInvitation("truncated invitation");
    let length = read_u8(bytes, cursor).ok_or_else(truncated)? as usize;
    if length > crate::hostname::MAX_HOSTNAME_BYTES {
        return Err(Error::InvalidInvitation("hostname is too long"));
    }
    let end = cursor.checked_add(length).ok_or_else(truncated)?;
    if end > bytes.len() {
        return Err(Error::InvalidInvitation("truncated invitation"));
    }
    let text = std::str::from_utf8(&bytes[*cursor..end])
        .map_err(|_| Error::InvalidInvitation("hostname is not utf-8"))?;
    *cursor = end;
    match crate::hostname::clean_hostname(text) {
        Some(name) if name == text => Ok(Some(name)),
        _ => Ok(None),
    }
}

fn signing_bytes(invitation: &Invitation, nonce: [u8; 16]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(DOMAIN);
    bytes.push(0);
    bytes.extend_from_slice(&invitation.endpoint_id);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&invitation.expires_at_ms.to_be_bytes());
    crate::ignored!(
        crate::trace::PAIRING,
        "an invitation that encoded once encodes the same here",
        write_relays(&mut bytes, &invitation.relays)
    );
    crate::ignored!(
        crate::trace::PAIRING,
        "an invitation that encoded once encodes the same here",
        write_addrs(&mut bytes, &invitation.direct)
    );
    // The display name is signed too: a holder of the file cannot rename
    // the writer without invalidating the invitation.
    write_hostname(&mut bytes, &invitation.hostname);
    bytes.push(invitation.os.encode());
    bytes
}

fn write_relays(bytes: &mut Vec<u8>, relays: &[String]) -> Result<(), Error> {
    if relays.len() > crate::relay::MAX_CUSTOM_RELAYS {
        return Err(Error::InvalidInvitation("too many relay URLs"));
    }
    bytes.push(relays.len() as u8);
    for relay in relays {
        let canonical = parse_relay_url(relay)?.to_string();
        let length = u16::try_from(canonical.len())
            .map_err(|_| Error::InvalidInvitation("relay URL is too long"))?;
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(canonical.as_bytes());
    }
    Ok(())
}

fn read_relays(bytes: &[u8], cursor: &mut usize) -> Result<Vec<String>, Error> {
    let count =
        read_u8(bytes, cursor).ok_or(Error::InvalidInvitation("truncated invitation"))? as usize;
    if count > crate::relay::MAX_CUSTOM_RELAYS {
        return Err(Error::InvalidInvitation("too many relay URLs"));
    }
    let mut relays = Vec::with_capacity(count);
    for _ in 0..count {
        let length = u16::from_be_bytes(
            read_array::<2>(bytes, cursor)
                .ok_or(Error::InvalidInvitation("truncated invitation"))?,
        ) as usize;
        let end = cursor
            .checked_add(length)
            .ok_or(Error::InvalidInvitation("truncated invitation"))?;
        if end > bytes.len() {
            return Err(Error::InvalidInvitation("truncated invitation"));
        }
        let text = std::str::from_utf8(&bytes[*cursor..end])
            .map_err(|_| Error::InvalidInvitation("relay URL is not utf-8"))?;
        let canonical = parse_relay_url(text)?.to_string();
        relays.push(canonical);
        *cursor = end;
    }
    Ok(relays)
}

fn write_addrs(bytes: &mut Vec<u8>, addrs: &[SocketAddr]) -> Result<(), Error> {
    if addrs.len() > MAX_DIRECT_ADDRS {
        return Err(Error::InvalidInvitation("too many direct addresses"));
    }
    bytes.push(addrs.len() as u8);
    for addr in addrs {
        write_sockaddr(bytes, addr);
    }
    Ok(())
}

fn read_addrs(bytes: &[u8], cursor: &mut usize) -> Result<Vec<SocketAddr>, Error> {
    let count =
        read_u8(bytes, cursor).ok_or(Error::InvalidInvitation("truncated invitation"))? as usize;
    if count > MAX_DIRECT_ADDRS {
        return Err(Error::InvalidInvitation("too many direct addresses"));
    }
    let mut addrs = Vec::with_capacity(count);
    for _ in 0..count {
        let family =
            read_u8(bytes, cursor).ok_or(Error::InvalidInvitation("truncated invitation"))?;
        let addr = match family {
            4 => {
                let octets = read_array::<4>(bytes, cursor)
                    .ok_or(Error::InvalidInvitation("truncated invitation"))?;
                let port = u16::from_be_bytes(
                    read_array::<2>(bytes, cursor)
                        .ok_or(Error::InvalidInvitation("truncated invitation"))?,
                );
                SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port)
            }
            6 => {
                let octets = read_array::<16>(bytes, cursor)
                    .ok_or(Error::InvalidInvitation("truncated invitation"))?;
                let port = u16::from_be_bytes(
                    read_array::<2>(bytes, cursor)
                        .ok_or(Error::InvalidInvitation("truncated invitation"))?,
                );
                SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port)
            }
            _ => return Err(Error::InvalidInvitation("unknown address family")),
        };
        if addr.ip().is_unspecified() || addr.port() == 0 {
            return Err(Error::InvalidInvitation("direct address is not dialable"));
        }
        addrs.push(addr);
    }
    Ok(addrs)
}

pub(crate) fn canonicalize_addrs(addrs: &[SocketAddr]) -> Result<Vec<SocketAddr>, Error> {
    let mut direct: Vec<_> = addrs
        .iter()
        .copied()
        .filter(|addr| !addr.ip().is_unspecified() && addr.port() != 0)
        .collect();
    direct.sort_unstable();
    direct.dedup();
    if direct.len() > MAX_DIRECT_ADDRS {
        direct.truncate(MAX_DIRECT_ADDRS);
    }
    Ok(direct)
}

pub(crate) fn unix_ms(time: SystemTime) -> Result<u64, Error> {
    time.duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidInvitation("system clock is before the unix epoch"))?
        .as_millis()
        .try_into()
        .map_err(|_| Error::InvalidInvitation("system clock is out of range"))
}

pub(crate) fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub(crate) fn decode_hex(text: &str) -> Result<Vec<u8>, Error> {
    if text.len() % 2 != 0 {
        return Err(Error::InvalidInvitation("unrecognized invitation"));
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let digits = text.as_bytes();
    let mut index = 0;
    while index < digits.len() {
        let high = hex_value(digits[index])?;
        let low = hex_value(digits[index + 1])?;
        bytes.push((high << 4) | low);
        index += 2;
    }
    Ok(bytes)
}

fn hex_value(byte: u8) -> Result<u8, Error> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::InvalidInvitation("unrecognized invitation")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_os_are_signed() {
        let secret = SecretKey::generate();
        let now = SystemTime::now();
        let (token, _) = issue(
            &secret,
            &[],
            &[],
            Duration::from_secs(60),
            now,
            Some("desktop".into()),
        )
        .unwrap();
        let mut bytes = decode_hex(&token[PREFIX.len()..]).unwrap();
        let position = bytes.windows(7).position(|w| w == b"desktop").unwrap();
        bytes[position] = b'l';
        assert!(open(&format!("{PREFIX}{}", encode_hex(&bytes)), now).is_err());
        let mut bytes = decode_hex(&token[PREFIX.len()..]).unwrap();
        let os = bytes.len() - 65;
        bytes[os] = (bytes[os] + 1) % 4;
        assert!(matches!(
            open(&format!("{PREFIX}{}", encode_hex(&bytes)), now),
            Err(Error::InvalidInvitation(_))
        ));
    }

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
            None,
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
        let (token, _) = issue(&secret, &[], &[], Duration::from_secs(1), now, None).unwrap();
        let later = now + Duration::from_secs(2);
        assert!(matches!(open(&token, later), Err(Error::InvitationExpired)));

        let (token, invitation) =
            issue(&secret, &[], &[], Duration::from_secs(60), now, None).unwrap();
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
            issue(&secret, &[], &[], Duration::ZERO, now, None),
            Err(Error::InvalidInvitation(
                "lifetime must be between 1 second and 15 minutes"
            ))
        ));
        assert!(matches!(
            issue(&secret, &[], &[], Duration::from_millis(999), now, None),
            Err(Error::InvalidInvitation(
                "lifetime must be between 1 second and 15 minutes"
            ))
        ));
        assert!(issue(&secret, &[], &[], Duration::from_secs(1), now, None).is_ok());
        assert!(issue(&secret, &[], &[], MAX_TTL, now, None).is_ok());
        assert!(matches!(
            issue(
                &secret,
                &[],
                &[],
                MAX_TTL + Duration::from_secs(1),
                now,
                None
            ),
            Err(Error::InvalidInvitation(
                "lifetime must be between 1 second and 15 minutes"
            ))
        ));
        let before = UNIX_EPOCH.checked_sub(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            issue(&secret, &[], &[], Duration::from_secs(60), before, None),
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
            None,
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
        let (_, invitation) =
            issue(&secret, &[], &many, Duration::from_secs(60), now, None).unwrap();
        assert_eq!(invitation.direct.len(), MAX_DIRECT_ADDRS);
        assert!(!invitation
            .direct
            .iter()
            .any(|addr| addr == &SocketAddr::from(([10, 0, 0, 9], 9))));

        let relays: Vec<String> = (0..9)
            .map(|index| format!("https://relay{index}.example.com"))
            .collect();
        assert!(matches!(
            issue(&secret, &relays, &[], Duration::from_secs(60), now, None),
            Err(Error::InvalidInvitation("too many relay URLs"))
        ));
        assert!(issue(
            &secret,
            &["http://relay.example.com".to_owned()],
            &[],
            Duration::from_secs(60),
            now,
            None
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
}
