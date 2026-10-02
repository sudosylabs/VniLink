//! Opaque installation record.
//!
//! The bytes hold the installation secret, paired devices, grants, used
//! invitation nonces, and this installation's relay settings. The host stores
//! them. This module does not open a credential store.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{SystemTime, UNIX_EPOCH};

use iroh::{PublicKey, SecretKey};

use crate::error::Error;
use crate::invitation::{self, ReplayCache, MAX_DIRECT_ADDRS};
use crate::relay::{CustomRelay, RelaySettings, MAX_CUSTOM_RELAYS};
use crate::session::Grants;
use crate::trust::{Device, Trust, MAX_DEVICES};

const MAGIC: &[u8; 4] = b"VNLK";
const VERSION: u8 = 1;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_TEXT: usize = 512;

/// Where the host keeps one installation record.
///
/// `load` returns [`None`] when nothing has been saved. `save` replaces the
/// previous bytes. Both run on the caller's thread while a change is applied,
/// so they must return without waiting on the network. Do not log the bytes:
/// they contain the installation secret and any relay credential.
pub trait InstallationStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, Error>;
    fn save(&self, record: &[u8]) -> Result<(), Error>;
}

/// Process-local record. Tests use this. It is not a platform credential store,
/// and dropping the last clone drops the bytes. [`Debug`] prints no record.
#[derive(Clone)]
pub struct MemoryStore {
    inner: std::sync::Arc<std::sync::Mutex<MemoryInner>>,
}

struct MemoryInner {
    record: Option<Vec<u8>>,
    load_fault: Option<StoreFault>,
    save_fault: Option<StoreFault>,
}

#[derive(Clone, Copy)]
enum StoreFault {
    Locked,
    Unavailable,
}

impl From<StoreFault> for Error {
    fn from(fault: StoreFault) -> Self {
        match fault {
            StoreFault::Locked => Error::StoreLocked,
            StoreFault::Unavailable => Error::StoreUnavailable,
        }
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MemoryStore")
    }
}

fn lock(store: &MemoryStore) -> std::sync::MutexGuard<'_, MemoryInner> {
    store
        .inner
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

impl MemoryStore {
    pub fn empty() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(MemoryInner {
                record: None,
                load_fault: None,
                save_fault: None,
            })),
        }
    }

    pub(crate) fn containing(bytes: Vec<u8>) -> Self {
        let store = Self::empty();
        lock(&store).record = Some(bytes);
        store
    }

    pub(crate) fn locked() -> Self {
        let store = Self::empty();
        lock(&store).load_fault = Some(StoreFault::Locked);
        store
    }

    pub(crate) fn unavailable() -> Self {
        let store = Self::empty();
        lock(&store).load_fault = Some(StoreFault::Unavailable);
        store
    }

    /// The next [`InstallationStore::save`] returns `error` once.
    pub(crate) fn fail_next_save(&self, error: Error) {
        let fault = match error {
            Error::StoreLocked => StoreFault::Locked,
            Error::StoreUnavailable => StoreFault::Unavailable,
            _ => panic!("the injected save fault must be a store error"),
        };
        lock(self).save_fault = Some(fault);
    }

    pub(crate) fn record(&self) -> Option<Vec<u8>> {
        lock(self).record.clone()
    }
}

impl InstallationStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, Error> {
        let inner = lock(self);
        if let Some(fault) = inner.load_fault {
            return Err(fault.into());
        }
        Ok(inner.record.clone())
    }

    fn save(&self, record: &[u8]) -> Result<(), Error> {
        let mut inner = lock(self);
        if let Some(fault) = inner.save_fault.take() {
            return Err(fault.into());
        }
        inner.record = Some(record.to_vec());
        Ok(())
    }
}

pub(crate) struct Record {
    pub secret: [u8; 32],
    pub relay: RelaySettings,
    pub trust: Trust,
    pub replay: ReplayCache,
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
) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    body.extend_from_slice(MAGIC);
    body.push(VERSION);
    body.extend_from_slice(secret);
    write_relay(&mut body, relay)?;
    write_devices(&mut body, trust)?;
    write_replay(&mut body, replay)?;
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
        || signed[4] != VERSION
    {
        return Err(Error::CorruptRecord);
    }
    let mut reader = Reader {
        bytes: &signed[5..],
        cursor: 0,
    };
    let secret = reader.array::<32>()?;
    let relay = read_relay(&mut reader)?;
    let trust = read_devices(&mut reader, &secret)?;
    let replay = read_replay(&mut reader)?;
    reader.finish()?;
    Ok(Record {
        secret,
        relay,
        trust,
        replay,
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
        out.push(grant_bits(device.grants));
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

fn read_devices(reader: &mut Reader<'_>, secret: &[u8; 32]) -> Result<Trust, Error> {
    let count = reader.u16()? as usize;
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
        let grants = grants_from_bits(reader.byte()?)?;
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
            })
            .map_err(|_| Error::CorruptRecord)?;
    }
    Ok(trust)
}

fn read_replay(reader: &mut Reader<'_>) -> Result<ReplayCache, Error> {
    let count = reader.u16()? as usize;
    if count > invitation::MAX_REPLAY {
        return Err(Error::CorruptRecord);
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let nonce = reader.array::<16>()?;
        let expires_at_ms = reader.u64()?;
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
            let port = reader.u16()?;
            SocketAddr::new(IpAddr::V4(Ipv4Addr::from(octets)), port)
        }
        6 => {
            let octets = reader.array::<16>()?;
            let port = reader.u16()?;
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
    if bits & !0b111 != 0 {
        return Err(Error::CorruptRecord);
    }
    Ok(Grants {
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
    body.push(VERSION);
    body.extend_from_slice(payload);
    let checksum = crc32(&body);
    body.extend_from_slice(&checksum.to_be_bytes());
    body
}

#[cfg(test)]
pub(crate) fn checksum(bytes: &[u8]) -> u32 {
    crc32(bytes)
}
