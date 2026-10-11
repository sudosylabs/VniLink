//! Bytes of one installation record.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey};

use crate::error::Error;
use crate::invitation::{self, ReplayCache, MAX_DIRECT_ADDRS};
use crate::relay::{CustomRelay, RelaySettings, MAX_CUSTOM_RELAYS};
use crate::session::Grants;
use crate::trust::{Device, Trust, MAX_DEVICES};

const MAGIC: &[u8; 4] = b"VNLK";
/// Version 2 appends the peer hostname section after the devices. Version 1
/// records still decode, with no names. Version 3 adds independent Terminal
/// and Camera grant bits; older records default both to denied. Version 4
/// adds the System grant bit and the system-action policy; older records
/// keep their other policies and default the new fields to denied.
const VERSION: u8 = 4;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_TEXT: usize = 512;

pub(crate) struct Record {
    pub secret: [u8; 32],
    pub relay: RelaySettings,
    pub trust: Trust,
    pub replay: ReplayCache,
    pub file_resource_policy: crate::job::FileResourcePolicy,
    pub power_policy: crate::power::PowerPolicy,
    pub terminal_policy: crate::terminal::TerminalPolicy,
    pub file_retention_policy: crate::job::FileRetentionPolicy,
    pub system_policy: crate::system::SystemPolicy,
}

impl std::fmt::Debug for Record {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Record")
            .field("secret", &"[redacted]")
            .field("relay", &self.relay)
            .field("devices", &self.trust.devices().count())
            .finish()
    }
}

pub(crate) fn encode(
    secret: &[u8; 32],
    relay: &RelaySettings,
    trust: &Trust,
    replay: &ReplayCache,
    terminal_policy: &crate::terminal::TerminalPolicy,
    file_retention_policy: &crate::job::FileRetentionPolicy,
    power_policy: &crate::power::PowerPolicy,
    file_resource_policy: &crate::job::FileResourcePolicy,
    system_policy: &crate::system::SystemPolicy,
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    body.extend_from_slice(MAGIC);
    body.push(VERSION);
    body.extend_from_slice(secret);
    write_relay(&mut body, relay)?;
    write_devices(&mut body, trust)?;
    write_names(&mut body, trust)?;
    write_replay(&mut body, replay)?;
    terminal_policy.validate()?;
    body.push(u8::from(terminal_policy.allow_persistence));
    body.extend(terminal_policy.max_sessions.to_be_bytes());
    body.extend(terminal_policy.max_sessions_per_peer.to_be_bytes());
    body.extend(terminal_policy.history_bytes.to_be_bytes());
    body.extend(terminal_policy.replay_bytes.to_be_bytes());
    body.extend(terminal_policy.detached_timeout_secs.to_be_bytes());
    file_retention_policy.validate()?;
    body.extend(file_retention_policy.resume_cache_bytes.to_be_bytes());
    body.extend(file_retention_policy.resume_cache_age_secs.to_be_bytes());
    body.extend(file_retention_policy.recent_fetch_bytes.to_be_bytes());
    body.extend(file_retention_policy.recent_fetch_age_secs.to_be_bytes());
    power_policy.validate()?;
    body.push(power_policy.bits());
    body.extend_from_slice(&power_policy.incoming_idle_secs.to_be_bytes());
    body.extend_from_slice(&power_policy.outgoing_idle_secs.to_be_bytes());
    file_resource_policy.validate()?;
    body.extend(file_resource_policy.max_concurrent_receivers.to_be_bytes());
    body.extend(file_resource_policy.max_reserved_bytes.to_be_bytes());
    body.extend(file_resource_policy.minimum_free_bytes.to_be_bytes());
    system_policy.validate()?;
    body.push(system_policy.bits());
    if body.len() + 4 > MAX_RECORD_BYTES {
        return Err(Error::InvalidSetting(
            "installation record field is too long",
        ));
    }
    let checksum = crc32(&body);
    body.extend_from_slice(&checksum.to_be_bytes());
    Ok(body)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Record, Error> {
    if bytes.len() < 46 || bytes.len() > MAX_RECORD_BYTES {
        return Err(Error::CorruptRecord);
    }
    let (signed, checksum_bytes) = bytes.split_at(bytes.len() - 4);
    let checksum = u32::from_be_bytes(
        checksum_bytes
            .try_into()
            .map_err(|_| Error::CorruptRecord)?,
    );
    if crc32(signed) != checksum
        || signed.len() < 5
        || signed[..4] != *MAGIC
        || !(1..=VERSION).contains(&signed[4])
    {
        return Err(Error::CorruptRecord);
    }
    let version = signed[4];
    let mut reader = Reader {
        bytes: &signed[5..],
        cursor: 0,
    };
    let secret = reader.array::<32>()?;
    let relay = read_relay(&mut reader)?;
    let trust = read_devices(&mut reader, &secret, version)?;
    let replay = read_replay(&mut reader)?;
    let terminal_policy = if version >= 3 {
        let allow_persistence = match reader.byte()? {
            0 => false,
            1 => true,
            _ => return Err(Error::CorruptRecord),
        };
        crate::terminal::TerminalPolicy {
            allow_persistence,
            max_sessions: u16::from_be_bytes(reader.array()?),
            max_sessions_per_peer: u16::from_be_bytes(reader.array()?),
            history_bytes: u32::from_be_bytes(reader.array()?),
            replay_bytes: u32::from_be_bytes(reader.array()?),
            detached_timeout_secs: u32::from_be_bytes(reader.array()?),
        }
        .validate()
        .map_err(|_| Error::CorruptRecord)?
    } else {
        crate::terminal::TerminalPolicy::default()
    };
    let file_retention_policy = if version >= 3 {
        crate::job::FileRetentionPolicy {
            resume_cache_bytes: u64::from_be_bytes(reader.array()?),
            resume_cache_age_secs: u64::from_be_bytes(reader.array()?),
            recent_fetch_bytes: u64::from_be_bytes(reader.array()?),
            recent_fetch_age_secs: u64::from_be_bytes(reader.array()?),
        }
        .validate()
        .map_err(|_| Error::CorruptRecord)?
    } else {
        crate::job::FileRetentionPolicy::default()
    };
    let power_policy = if version >= 3 {
        let mut policy = crate::power::PowerPolicy::from_bits(reader.byte()?)?;
        policy.incoming_idle_secs = u32::from_be_bytes(reader.array()?);
        policy.outgoing_idle_secs = u32::from_be_bytes(reader.array()?);
        policy.validate().map_err(|_| Error::CorruptRecord)?
    } else {
        Default::default()
    };
    let file_resource_policy = if version >= 3 {
        crate::job::FileResourcePolicy {
            max_concurrent_receivers: u16::from_be_bytes(reader.array()?),
            max_reserved_bytes: u64::from_be_bytes(reader.array()?),
            minimum_free_bytes: u64::from_be_bytes(reader.array()?),
        }
        .validate()
        .map_err(|_| Error::CorruptRecord)?
    } else {
        crate::job::FileResourcePolicy::default()
    };
    let system_policy = if version >= 4 {
        crate::system::SystemPolicy::from_bits(reader.byte()?).map_err(|_| Error::CorruptRecord)?
    } else {
        crate::system::SystemPolicy::default()
    };
    reader.finish()?;
    Ok(Record {
        secret,
        relay,
        trust,
        replay,
        terminal_policy,
        file_retention_policy,
        power_policy,
        file_resource_policy,
        system_policy,
    })
}

fn write_relay(out: &mut Vec<u8>, relay: &RelaySettings) -> Result<(), Error> {
    match relay {
        RelaySettings::Public => out.push(0),
        RelaySettings::Custom(relays) => {
            if relays.is_empty() || relays.len() > MAX_CUSTOM_RELAYS {
                return Err(Error::InvalidSetting(
                    "installation record field is too long",
                ));
            }
            out.push(1);
            out.push(relays.len() as u8);
            for relay in relays {
                write_text(out, relay.url())?;
                match relay.auth_token() {
                    None => out.push(0),
                    Some(token) => {
                        out.push(1);
                        write_text(out, token)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn write_devices(out: &mut Vec<u8>, trust: &Trust) -> Result<(), Error> {
    let devices: Vec<&Device> = trust.devices().collect();
    let count = u16::try_from(devices.len())
        .map_err(|_| Error::InvalidSetting("installation record field is too long"))?;
    out.extend_from_slice(&count.to_be_bytes());
    for device in devices {
        if device.relays.len() > MAX_CUSTOM_RELAYS || device.direct.len() > MAX_DIRECT_ADDRS {
            return Err(Error::InvalidSetting(
                "installation record field is too long",
            ));
        }
        out.extend_from_slice(&device.endpoint_id);
        out.push(device.relays.len() as u8);
        for relay in &device.relays {
            write_text(out, relay)?;
        }
        out.push(device.direct.len() as u8);
        for addr in &device.direct {
            write_addr(out, *addr);
        }
        let bits = u16::from(grant_bits(device.grants))
            | (u16::from(device.terminal) << 3)
            | (u16::from(device.camera) << 4)
            | (u16::from(device.system_audio) << 6)
            | (u16::from(device.microphone) << 7)
            | (u16::from(device.recording) << 8)
            | (u16::from(device.system_actions) << 9);
        out.extend_from_slice(&bits.to_be_bytes());
    }
    Ok(())
}

/// Reported and pet display names ride beside the devices. Spaces are
/// legal here, unlike relay URLs: only control characters are refused.
fn write_names(out: &mut Vec<u8>, trust: &Trust) -> Result<(), Error> {
    write_name_map(out, trust.hostnames())?;
    write_name_map(out, trust.pet_names())?;
    Ok(())
}

fn write_name_map<'a>(
    out: &mut Vec<u8>,
    names: impl Iterator<Item = (&'a [u8; 32], &'a String)>,
) -> Result<(), Error> {
    let names: Vec<(&[u8; 32], &String)> = names.collect();
    let count = u16::try_from(names.len())
        .map_err(|_| Error::InvalidSetting("installation record field is too long"))?;
    out.extend_from_slice(&count.to_be_bytes());
    for (endpoint_id, name) in names {
        if name.len() > crate::hostname::MAX_HOSTNAME_BYTES {
            return Err(Error::InvalidSetting(
                "installation record field is too long",
            ));
        }
        out.extend_from_slice(endpoint_id);
        let len = u16::try_from(name.len())
            .map_err(|_| Error::InvalidSetting("installation record field is too long"))?;
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    Ok(())
}

fn write_replay(out: &mut Vec<u8>, replay: &ReplayCache) -> Result<(), Error> {
    let entries = replay.entries();
    let count = u16::try_from(entries.len())
        .map_err(|_| Error::InvalidSetting("installation record field is too long"))?;
    out.extend_from_slice(&count.to_be_bytes());
    for (nonce, expires_at_ms) in entries {
        out.extend_from_slice(nonce);
        out.extend_from_slice(&expires_at_ms.to_be_bytes());
    }
    Ok(())
}

fn write_text(out: &mut Vec<u8>, text: &str) -> Result<(), Error> {
    let len = u16::try_from(text.len())
        .map_err(|_| Error::InvalidSetting("installation record field is too long"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

fn write_addr(out: &mut Vec<u8>, addr: SocketAddr) {
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

fn read_relay(reader: &mut Reader<'_>) -> Result<RelaySettings, Error> {
    match reader.byte()? {
        0 => Ok(RelaySettings::public_n0()),
        1 => {
            let count = reader.byte()? as usize;
            if count == 0 || count > MAX_CUSTOM_RELAYS {
                return Err(Error::CorruptRecord);
            }
            let mut relays = Vec::with_capacity(count);
            for _ in 0..count {
                let url = reader.text()?;
                let mut relay = CustomRelay::new(url).map_err(|_| Error::CorruptRecord)?;
                match reader.byte()? {
                    0 => {}
                    1 => {
                        let token = reader.text()?;
                        relay = relay
                            .with_auth_token(token)
                            .map_err(|_| Error::CorruptRecord)?;
                    }
                    _ => return Err(Error::CorruptRecord),
                }
                relays.push(relay);
            }
            RelaySettings::custom(relays).map_err(|_| Error::CorruptRecord)
        }
        _ => Err(Error::CorruptRecord),
    }
}

fn read_devices(reader: &mut Reader<'_>, secret: &[u8; 32], version: u8) -> Result<Trust, Error> {
    let count = u16::from_be_bytes(reader.array()?) as usize;
    if count > MAX_DEVICES {
        return Err(Error::CorruptRecord);
    }
    let own = *SecretKey::from_bytes(secret).public().as_bytes();
    let mut trust = Trust::default();
    for _ in 0..count {
        let endpoint_id = reader.array::<32>()?;
        if endpoint_id == own || PublicKey::from_bytes(&endpoint_id).is_err() {
            return Err(Error::CorruptRecord);
        }
        let display_id = PublicKey::from_bytes(&endpoint_id)
            .map_err(|_| Error::CorruptRecord)?
            .to_string();
        let relay_count = reader.byte()? as usize;
        if relay_count > MAX_CUSTOM_RELAYS {
            return Err(Error::CorruptRecord);
        }
        let mut relays = Vec::with_capacity(relay_count);
        for _ in 0..relay_count {
            let url = reader.text()?;
            let canonical = crate::relay::parse_relay_url(&url)
                .map_err(|_| Error::CorruptRecord)?
                .to_string();
            if relays
                .iter()
                .any(|existing: &String| existing == &canonical)
            {
                return Err(Error::CorruptRecord);
            }
            relays.push(canonical);
        }
        let addr_count = reader.byte()? as usize;
        if addr_count > MAX_DIRECT_ADDRS {
            return Err(Error::CorruptRecord);
        }
        let mut direct = Vec::with_capacity(addr_count);
        for _ in 0..addr_count {
            let addr = read_addr(reader)?;
            if direct.contains(&addr) {
                return Err(Error::CorruptRecord);
            }
            direct.push(addr);
        }
        let bits = if version >= 3 {
            reader.u16()?
        } else {
            u16::from(reader.byte()?)
        };
        if bits & !(if version >= 3 { 1023 } else { 7 }) != 0 {
            return Err(Error::CorruptRecord);
        }
        let grants = grants_from_bits((bits & 39) as u8)?;
        if trust.get(&endpoint_id).is_some() {
            return Err(Error::CorruptRecord);
        }
        trust
            .pair(Device {
                endpoint_id,
                display_id,
                relays,
                direct,
                grants,
                terminal: version >= 3 && bits & 8 != 0,
                camera: version >= 3 && bits & 16 != 0,
                system_audio: version >= 3 && bits & 64 != 0,
                microphone: version >= 3 && bits & 128 != 0,
                recording: version >= 3 && bits & 256 != 0,
                system_actions: version >= 3 && bits & 512 != 0,
            })
            .map_err(|_| Error::CorruptRecord)?;
    }
    if version >= 2 {
        read_names(reader, &mut trust)?;
    }
    Ok(trust)
}

fn read_names(reader: &mut Reader<'_>, trust: &mut Trust) -> Result<(), Error> {
    read_name_map(reader, trust, true)?;
    read_name_map(reader, trust, false)?;
    Ok(())
}

fn read_name_map(reader: &mut Reader<'_>, trust: &mut Trust, reported: bool) -> Result<(), Error> {
    let count = u16::from_be_bytes(reader.array()?) as usize;
    if count > MAX_DEVICES {
        return Err(Error::CorruptRecord);
    }
    for _ in 0..count {
        let endpoint_id = reader.array::<32>()?;
        let len = u16::from_be_bytes(reader.array()?) as usize;
        if len > crate::hostname::MAX_HOSTNAME_BYTES {
            return Err(Error::CorruptRecord);
        }
        let slice = reader.take(len)?;
        let text = std::str::from_utf8(slice).map_err(|_| Error::CorruptRecord)?;
        match crate::hostname::clean_hostname(text) {
            Some(name) if name == text => {}
            _ => return Err(Error::CorruptRecord),
        }
        // A name without its device is inconsistent, not forward-compatible
        // data: our writer never emits orphans.
        if trust.get(&endpoint_id).is_none() {
            return Err(Error::CorruptRecord);
        }
        let present = if reported {
            trust.hostname_of(&endpoint_id).is_some()
        } else {
            trust.pet_name_of(&endpoint_id).is_some()
        };
        if present {
            return Err(Error::CorruptRecord);
        }
        if reported {
            trust
                .set_hostname(&endpoint_id, text.to_string())
                .map_err(|_| Error::CorruptRecord)?;
        } else {
            trust
                .set_pet_name(&endpoint_id, text.to_string())
                .map_err(|_| Error::CorruptRecord)?;
        }
    }
    Ok(())
}

fn read_replay(reader: &mut Reader<'_>) -> Result<ReplayCache, Error> {
    let count = u16::from_be_bytes(reader.array()?) as usize;
    if count > invitation::MAX_REPLAY {
        return Err(Error::CorruptRecord);
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let nonce = reader.array::<16>()?;
        let expires_at_ms = u64::from_be_bytes(reader.array()?);
        if entries.iter().any(|(seen, _)| seen == &nonce) {
            return Err(Error::CorruptRecord);
        }
        entries.push((nonce, expires_at_ms));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidSetting("system clock is before the unix epoch"))?
        .as_millis();
    let now =
        u64::try_from(now).map_err(|_| Error::InvalidSetting("system clock is out of range"))?;
    entries.retain(|(_, expires_at_ms)| *expires_at_ms > now);
    ReplayCache::from_entries(entries)
}

fn read_addr(reader: &mut Reader<'_>) -> Result<SocketAddr, Error> {
    let addr = match reader.byte()? {
        4 => {
            let octets = reader.array::<4>()?;
            let port = u16::from_be_bytes(reader.array()?);
            SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port)
        }
        6 => {
            let octets = reader.array::<16>()?;
            let port = u16::from_be_bytes(reader.array()?);
            SocketAddr::new(IpAddr::V6(Ipv6Addr::from(octets)), port)
        }
        _ => return Err(Error::CorruptRecord),
    };
    if addr.ip().is_unspecified() || addr.port() == 0 {
        return Err(Error::CorruptRecord);
    }
    Ok(addr)
}

fn grant_bits(grants: Grants) -> u8 {
    let mut bits = 0;
    if grants.view {
        bits |= 32;
    }
    if grants.control {
        bits |= 1;
    }
    if grants.clipboard {
        bits |= 2;
    }
    if grants.transfer {
        bits |= 4;
    }
    bits
}

fn grants_from_bits(bits: u8) -> Result<Grants, Error> {
    if bits & !0b100111 != 0 {
        return Err(Error::CorruptRecord);
    }
    Ok(Grants {
        view: bits & 32 != 0,
        control: bits & 1 != 0,
        clipboard: bits & 2 != 0,
        transfer: bits & 4 != 0,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl Reader<'_> {
    fn finish(&self) -> Result<(), Error> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(Error::CorruptRecord)
        }
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let byte = *self.bytes.get(self.cursor).ok_or(Error::CorruptRecord)?;
        self.cursor += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, Error> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_be_bytes(self.array()?))
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let slice = self.take(N)?;
        let mut array = [0; N];
        array.copy_from_slice(slice);
        Ok(array)
    }

    fn take(&mut self, len: usize) -> Result<&[u8], Error> {
        let end = self.cursor.checked_add(len).ok_or(Error::CorruptRecord)?;
        if end > self.bytes.len() {
            return Err(Error::CorruptRecord);
        }
        let slice = &self.bytes[self.cursor..end];
        self.cursor = end;
        Ok(slice)
    }

    fn text(&mut self) -> Result<String, Error> {
        let len = self.u16()? as usize;
        if len > MAX_TEXT {
            return Err(Error::CorruptRecord);
        }
        let slice = self.take(len)?;
        let text = std::str::from_utf8(slice).map_err(|_| Error::CorruptRecord)?;
        if text
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(Error::CorruptRecord);
        }
        Ok(text.to_owned())
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[cfg(test)]
pub(crate) fn frame(payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + payload.len() + 4);
    body.extend_from_slice(MAGIC);
    // Hand-built malformed-record fixtures use the original v1 payload shape.
    body.push(1);
    body.extend_from_slice(payload);
    let checksum = crc32(&body);
    body.extend_from_slice(&checksum.to_be_bytes());
    body
}

#[cfg(test)]
pub(crate) fn checksum(bytes: &[u8]) -> u32 {
    crc32(bytes)
}
