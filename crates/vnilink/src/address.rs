// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Relay URLs and direct addresses a peer can dial.
//!
//! After a session is accepted, each computer sends one announcement on the
//! authenticated control stream. The announcement is that computer's own home
//! relay URLs and its current direct sockets. It carries no relay credential
//! and no grants. The receiver stores it on that peer only, because the Iroh
//! connection already names the sender. A later reconnect dials this stored
//! list instead of the addresses from the original invitation.

use std::net::SocketAddr;

use crate::codec::{read_array, read_sockaddr, read_u8, write_sockaddr};
use crate::error::Error;
use crate::invitation::{self, MAX_DIRECT_ADDRS};
use crate::relay::{self, MAX_CUSTOM_RELAYS};
use crate::trust::Device;

pub(crate) const TAG: u8 = 6;

/// Largest encoded announcement. Eight relay URLs at the relay module's
/// length cap, plus eight direct sockets, plus the small header.
pub(crate) const MAX_FRAME: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Announcement {
    pub relays: Vec<String>,
    pub direct: Vec<SocketAddr>,
}

pub(crate) fn canonical(relays: &[String], direct: &[SocketAddr]) -> Result<Announcement, Error> {
    if relays.len() > MAX_CUSTOM_RELAYS || direct.len() > MAX_DIRECT_ADDRS {
        return Err(Error::Malformed);
    }
    let mut canonical_relays = Vec::with_capacity(relays.len());
    for relay in relays {
        let parsed = relay::parse_relay_url(relay).map_err(|_| Error::Malformed)?;
        let text = parsed.to_string();
        if canonical_relays
            .iter()
            .any(|existing: &String| existing == &text)
        {
            return Err(Error::Malformed);
        }
        canonical_relays.push(text);
    }
    Ok(Announcement {
        relays: canonical_relays,
        direct: invitation::canonicalize_addrs(direct).map_err(|_| Error::Malformed)?,
    })
}

/// Replaces dial addresses. Grants and the endpoint id stay as they were.
pub(crate) fn install(device: &mut Device, announcement: &Announcement) -> Result<(), Error> {
    let announcement = canonical(&announcement.relays, &announcement.direct)?;
    device.relays = announcement.relays;
    device.direct = announcement.direct;
    Ok(())
}

pub(crate) fn unchanged(device: &Device, announcement: &Announcement) -> bool {
    device.relays == announcement.relays && device.direct == announcement.direct
}

pub(crate) fn encode(relays: &[String], direct: &[SocketAddr]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(TAG);
    bytes.push(relays.len() as u8);
    for relay in relays {
        let length = u16::try_from(relay.len()).unwrap_or(u16::MAX);
        bytes.extend_from_slice(&length.to_be_bytes());
        bytes.extend_from_slice(relay.as_bytes());
    }
    bytes.push(direct.len() as u8);
    for addr in direct {
        write_sockaddr(&mut bytes, addr);
    }
    bytes
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Announcement, Error> {
    if bytes.len() < 3 || bytes.len() > MAX_FRAME || bytes[0] != TAG {
        return Err(Error::Malformed);
    }
    let mut cursor = 1;
    let relay_count = read_u8(bytes, &mut cursor).ok_or(Error::Malformed)?;
    if relay_count as usize > MAX_CUSTOM_RELAYS {
        return Err(Error::Malformed);
    }
    let mut relays = Vec::with_capacity(relay_count as usize);
    for _ in 0..relay_count {
        let length =
            u16::from_be_bytes(read_array::<2>(bytes, &mut cursor).ok_or(Error::Malformed)?)
                as usize;
        let end = cursor.checked_add(length).ok_or(Error::Malformed)?;
        if end > bytes.len() {
            return Err(Error::Malformed);
        }
        let text = std::str::from_utf8(&bytes[cursor..end]).map_err(|_| Error::Malformed)?;
        relays.push(text.to_owned());
        cursor = end;
    }
    let addr_count = read_u8(bytes, &mut cursor).ok_or(Error::Malformed)?;
    if addr_count as usize > MAX_DIRECT_ADDRS {
        return Err(Error::Malformed);
    }
    let mut direct = Vec::with_capacity(addr_count as usize);
    for _ in 0..addr_count {
        direct.push(read_sockaddr(bytes, &mut cursor).ok_or(Error::Malformed)?);
    }
    if cursor != bytes.len() {
        return Err(Error::Malformed);
    }
    canonical(&relays, &direct)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ControlMessage;

    #[test]
    fn an_announcement_round_trips_and_keeps_grants() {
        let relays = vec!["https://relay.example.com".to_string()];
        let direct = vec!["127.0.0.1:9".parse().unwrap(), "[::1]:9".parse().unwrap()];
        let announcement = canonical(&relays, &direct).unwrap();
        let bytes = encode(&announcement.relays, &announcement.direct);
        assert_eq!(decode(&bytes).unwrap(), announcement);
        let message = ControlMessage::Addresses {
            relays: announcement.relays.clone(),
            direct: announcement.direct.clone(),
        };
        assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
        assert_eq!(
            ControlMessage::decode(&ControlMessage::Close.encode()).unwrap(),
            ControlMessage::Close
        );

        let mut device = Device {
            endpoint_id: [4; 32],
            display_id: "peer".to_string(),
            relays: vec!["https://old.example.com".to_string()],
            direct: vec!["198.51.100.8:8".parse().unwrap()],
        };
        install(&mut device, &announcement).unwrap();
        assert_eq!(device.endpoint_id, [4; 32]);
        assert_eq!(device.relays, announcement.relays);
        assert!(!device.direct.contains(&"198.51.100.8:8".parse().unwrap()));

        let mut too_many = vec!["203.0.113.1:1".parse().unwrap()];
        for port in 2..=MAX_DIRECT_ADDRS as u16 + 1 {
            too_many.push(SocketAddr::from(([203, 0, 113, 2], port)));
        }
        assert!(matches!(canonical(&[], &too_many), Err(Error::Malformed)));
        assert!(matches!(
            decode(b"\x06\x01\x00\x05http:extra"),
            Err(Error::Malformed)
        ));
        let _ = Announcement {
            relays: Vec::new(),
            direct: Vec::new(),
        };
    }
}
