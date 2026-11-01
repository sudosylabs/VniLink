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
//!
//! The relay list is what [`crate::RelaySettings::relay_urls`] reports: empty
//! while the installation uses the public relays, the configured home relay
//! URLs otherwise. Direct sockets are the endpoint's current ones. A frame
//! with more relays or sockets than the invitation allows, a repeated relay
//! or an unparsable URL is [`Error::Malformed`]. A failed save in the
//! receiver keeps the previous stored list.
//!
//! A peer chooses what its announcement says, and this computer later dials
//! it. [`decode`] therefore keeps only what a remote computer can own: an
//! HTTPS relay on a public name or address, and a direct socket that is not
//! a multicast or broadcast group. Anything else is dropped from the
//! announcement; the rest is kept. A loopback socket stays, because a peer
//! on this same computer is a legitimate peer.

use std::net::{IpAddr, SocketAddr};

use crate::codec::{read_array, read_sockaddr, read_u8, write_sockaddr};
use crate::error::Error;
use crate::invitation::{self, MAX_DIRECT_ADDRS};
use crate::relay::{self, MAX_CUSTOM_RELAYS};
use crate::trust::Device;

/// First byte of the announcement on the control stream.
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

/// Replaces dial addresses. Sharing consent and the endpoint id stay as they were.
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
    let announcement = canonical(&relays, &direct)?;
    Ok(reachable(announcement))
}

/// What a remote peer may ask this computer to dial.
fn reachable(announcement: Announcement) -> Announcement {
    Announcement {
        relays: announcement
            .relays
            .into_iter()
            .filter(|relay| relay_is_remote(relay))
            .collect(),
        direct: announcement
            .direct
            .into_iter()
            .filter(|addr| !is_group_address(addr.ip()))
            .collect(),
    }
}

fn relay_is_remote(relay: &str) -> bool {
    let Ok(url) = relay::parse_relay_url(relay) else {
        return false;
    };
    if url.scheme() != "https" {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) => is_global_unicast(ip),
        Err(_) => !host.eq_ignore_ascii_case("localhost") && host.contains('.'),
    }
}

fn is_group_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_multicast() || v4.is_broadcast(),
        IpAddr::V6(v6) => v6.is_multicast(),
    }
}

/// A relay on an IP literal must be reachable from outside its own network.
fn is_global_unicast(ip: IpAddr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || is_group_address(ip) {
        return false;
    }
    match ip {
        IpAddr::V4(v4) => !(v4.is_private() || v4.is_link_local()),
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            let unique_local = first & 0xfe00 == 0xfc00;
            let link_local = first & 0xffc0 == 0xfe80;
            !(unique_local || link_local)
        }
    }
}

/// How many announcements a link may have saved in a burst, and how long
/// each one then takes to come back. Each saved announcement rewrites the
/// installation record, so a peer cannot choose the write rate.
const ANNOUNCEMENT_BURST: u32 = 4;
const ANNOUNCEMENT_REFILL: std::time::Duration = std::time::Duration::from_secs(15);

/// Admits the address announcements of one link. A refused announcement is
/// dropped: the peer sends its addresses again whenever they change, and on
/// its next connection.
pub(crate) struct AnnouncementBudget {
    left: u32,
    since: std::time::Instant,
}

impl AnnouncementBudget {
    pub(crate) fn new(now: std::time::Instant) -> Self {
        Self {
            left: ANNOUNCEMENT_BURST,
            since: now,
        }
    }

    pub(crate) fn admit(&mut self, now: std::time::Instant) -> bool {
        let earned = (now.saturating_duration_since(self.since).as_millis()
            / ANNOUNCEMENT_REFILL.as_millis()) as u32;
        if earned > 0 {
            self.left = (self.left + earned).min(ANNOUNCEMENT_BURST);
            self.since += ANNOUNCEMENT_REFILL * earned;
        }
        if self.left == 0 {
            return false;
        }
        self.left -= 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ControlMessage;

    #[test]
    fn a_link_saves_a_burst_of_announcements_and_then_one_per_refill() {
        let start = std::time::Instant::now();
        let mut budget = AnnouncementBudget::new(start);
        let admitted = (0..10).filter(|_| budget.admit(start)).count();
        assert_eq!(admitted as u32, ANNOUNCEMENT_BURST);
        assert!(!budget.admit(start + ANNOUNCEMENT_REFILL / 2));
        assert!(budget.admit(start + ANNOUNCEMENT_REFILL));
        assert!(!budget.admit(start + ANNOUNCEMENT_REFILL));
        let later = start + ANNOUNCEMENT_REFILL * 100;
        let admitted = (0..10).filter(|_| budget.admit(later)).count();
        assert_eq!(
            admitted as u32, ANNOUNCEMENT_BURST,
            "idle time does not bank more than a burst"
        );
    }

    #[test]
    fn a_peer_cannot_make_this_computer_dial_its_own_services() {
        let relays = vec![
            "http://127.0.0.1:9".to_string(),
            "https://localhost".to_string(),
            "https://10.0.0.5".to_string(),
            "https://[::1]".to_string(),
            "https://intranet".to_string(),
            "https://relay.example.com".to_string(),
        ];
        let direct = vec![
            "127.0.0.1:9".parse().unwrap(),
            "224.0.0.1:9".parse().unwrap(),
            "255.255.255.255:9".parse().unwrap(),
            "[ff02::1]:9".parse().unwrap(),
            "203.0.113.7:9".parse().unwrap(),
        ];
        let sent = canonical(&relays, &direct).unwrap();
        let received = decode(&encode(&sent.relays, &sent.direct)).unwrap();
        assert_eq!(
            received.relays,
            vec!["https://relay.example.com/".to_string()]
        );
        assert_eq!(
            received.direct,
            vec![
                "127.0.0.1:9".parse().unwrap(),
                "203.0.113.7:9".parse().unwrap()
            ]
        );
    }

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
