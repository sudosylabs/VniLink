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
#[derive(Clone, PartialEq, Eq)]
pub struct Invitation {
    pub endpoint_id: [u8; 32],
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
    pub expires_at_ms: u64,
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
    key.verify(&signing_bytes(&invitation, nonce), &signature)
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
        },
        nonce,
        signature,
    ))
}

fn signing_bytes(invitation: &Invitation, nonce: [u8; 16]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(DOMAIN);
    bytes.push(0);
    bytes.extend_from_slice(&invitation.endpoint_id);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&invitation.expires_at_ms.to_be_bytes());
    let _ = write_relays(&mut bytes, &invitation.relays);
    let _ = write_addrs(&mut bytes, &invitation.direct);
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
