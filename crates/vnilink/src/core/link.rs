//! The live link and the address refresh both roles share.
//!
//! The host and the controller each store the connection on the session and
//! exchange home relay URLs and direct addresses on the authenticated control
//! stream. The receiver keeps that list for that peer only; a later dial,
//! including a reconnect after a dropped link, uses the stored list.

use super::*;

use super::wire::write_frame;

use std::net::SocketAddr;

use iroh::{Endpoint, EndpointId, TransportAddr};

use crate::address::Announcement;
use crate::error::Error;
use crate::events::CoreEvent;
use crate::invitation::{self};
use crate::protocol::ControlMessage;

/// The relay URLs and direct sockets the endpoint currently sees for `peer`.
pub(super) async fn peer_addresses(
    endpoint: &Endpoint,
    peer: EndpointId,
) -> (Vec<String>, Vec<SocketAddr>) {
    let Some(info) = endpoint.remote_info(peer).await else {
        return (Vec::new(), Vec::new());
    };
    let mut relays = Vec::new();
    let mut direct = Vec::new();
    for addr in info.addrs() {
        match addr.addr() {
            TransportAddr::Relay(url) => {
                if relays.len() >= crate::relay::MAX_CUSTOM_RELAYS {
                    continue;
                }
                let Ok(parsed) = crate::relay::parse_relay_url(url.as_str()) else {
                    continue;
                };
                let text = parsed.to_string();
                if !relays.contains(&text) {
                    relays.push(text);
                }
            }
            TransportAddr::Ip(socket) => direct.push(*socket),
            _ => {}
        }
    }
    let direct = invitation::canonicalize_addrs(&direct).unwrap_or_default();
    (relays, direct)
}

pub(super) fn remember_link(shared: &Shared, incoming: bool, link: iroh::endpoint::Connection) {
    let mut state = lock(&shared.state);
    let session = if incoming {
        state.incoming.as_mut()
    } else {
        state.outgoing.as_mut()
    };
    if let Some(session) = session {
        session.link = Some(link);
    }
}

pub(super) fn remember_peer_addresses(
    shared: &Shared,
    peer_key: &[u8; 32],
    announcement: Announcement,
) -> Result<(), Error> {
    let mut state = lock(&shared.state);
    let Some(mut device) = state.trust.get(peer_key).cloned() else {
        return Ok(());
    };
    if crate::address::unchanged(&device, &announcement) {
        return Ok(());
    }
    let previous = device.clone();
    crate::address::install(&mut device, &announcement)?;
    let endpoint_id = device.display_id.clone();
    state.trust.pair(device)?;
    if let Err(error) = persist(shared, &state) {
        let _ = state.trust.pair(previous);
        return Err(error);
    }
    drop(state);
    shared
        .events
        .publish(CoreEvent::AddressesUpdated { endpoint_id });
    Ok(())
}

pub(super) async fn write_addresses(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
) -> Result<(), Error> {
    let relays = shared.relay.relay_urls()?;
    let direct = invitation::canonicalize_addrs(&direct_addresses(&shared.endpoint))?;
    let announcement = crate::address::canonical(&relays, &direct)?;
    write_frame(
        send,
        &ControlMessage::Addresses {
            relays: announcement.relays,
            direct: announcement.direct,
        },
    )
    .await
}
