// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Routing a connection's Desktop datagrams to the desktop they stamp.

use super::*;

pub(super) fn epoch(stamp: SessionStamp) -> [u8; PREFIX] {
    let mut bytes = [0; PREFIX];
    bytes[..4].copy_from_slice(MAGIC);
    bytes[4..12].copy_from_slice(&stamp.id.to_be_bytes());
    bytes[12..20].copy_from_slice(&stamp.generation.to_be_bytes());
    bytes
}

/// Splits the 20-byte owner prefix (magic + stamp) from a datagram body.
/// Unprefixed traffic belongs to no owner and is dropped.
pub(super) fn split_datagram(bytes: &[u8]) -> Option<(SessionStamp, &[u8])> {
    if bytes.len() <= PREFIX || bytes[..4] != *MAGIC {
        return None;
    }
    let stamp = SessionStamp::new(
        u64::from_be_bytes(bytes[4..12].try_into().ok()?),
        u64::from_be_bytes(bytes[12..20].try_into().ok()?),
    );
    if stamp.id == 0 {
        return None;
    }
    Some((stamp, &bytes[PREFIX..]))
}

/// Starts the connection's sole desktop-datagram reader, once per
/// (connection, generation). Per-owner readers would split fragments
/// between siblings; the router reassembles per stamp and feeds each
/// owner's inbox. The task exits when the connection closes or the
/// session ends, and releases its key so a later generation can route.
pub(super) fn ensure_datagram_router(
    shared: &Arc<Shared>,
    link: Connection,
    connection_id: u64,
    generation: u64,
) {
    if !lock(&shared.desktop_routers).insert((connection_id, generation)) {
        return;
    }
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        route_desktop_datagrams(&shared, &link, connection_id, generation).await;
        lock(&shared.desktop_routers).remove(&(connection_id, generation));
    });
}

/// How often the router checks its connection is still the live one; between
/// checks a closed owner is noticed by its own retired flag.
pub(super) const ROUTER_LIVENESS: Duration = Duration::from_millis(100);

/// Datagrams taken from the connection at once.
pub(super) const ROUTER_BATCH: usize = 32;

pub(super) async fn route_desktop_datagrams(
    shared: &Arc<Shared>,
    link: &Connection,
    connection_id: u64,
    generation: u64,
) {
    // Each owner's runtime, found once under the state lock and kept until
    // it retires, so a datagram takes no shared lock.
    let mut owners: std::collections::HashMap<(u64, u64), Arc<DesktopRuntime>> =
        std::collections::HashMap::new();
    let mut checked: Option<std::time::Instant> = None;
    let mut batch = vec![bytes::Bytes::new(); ROUTER_BATCH];
    loop {
        if checked.is_none_or(|at| at.elapsed() >= ROUTER_LIVENESS) {
            let state = lock(&shared.state);
            let live = !state.shut_down
                && state.outgoing.get(&connection_id).is_some_and(|session| {
                    session.generation == generation && !finished(&session.phase)
                });
            if !live {
                return;
            }
            checked = Some(std::time::Instant::now());
        }
        let count = match link.read_many_datagrams(&mut batch).await {
            Ok(count) => count,
            Err(_) => return,
        };
        for bytes in &batch[..count] {
            let Some((stamp, fragment)) = split_datagram(bytes) else {
                continue;
            };
            let key = (stamp.id, stamp.generation);
            let cached = owners
                .get(&key)
                .filter(|desktop| !desktop.retired.load(std::sync::atomic::Ordering::Acquire))
                .cloned();
            let target = cached.or_else(|| {
                let state = lock(&shared.state);
                state.services.values().find_map(|record| {
                    record
                        .desktop
                        .as_ref()
                        .filter(|desktop| {
                            record.info.connection_id == connection_id
                                && record.info.generation == generation
                                && !record.info.phase.finished()
                                && desktop.stamp == stamp
                        })
                        .map(Arc::clone)
                })
            });
            let Some(desktop) = target else {
                owners.remove(&key);
                continue;
            };
            // One cached owner per stamp, which reassembles its own
            // pictures; the map cannot outgrow the owner bound plus one
            // transient entry for a closing sibling.
            if owners.len() > MAX_DESKTOP_OWNERS_PER_CONNECTION + 1 {
                owners.clear();
            }
            owners.insert(key, Arc::clone(&desktop));
            if dropped(shared, DropPoint::Datagram) {
                continue;
            }
            let arrival = lock(&desktop.pictures).datagram(fragment, std::time::Instant::now());
            desktop.report.fragments(arrival.fragments);
            if arrival.body.is_some() {
                desktop.report.frame_completed();
            }
            if arrival.unfinished.any() {
                desktop.counters.lost(crate::video::Losses {
                    expired: arrival.unfinished.expired,
                    evicted: arrival.unfinished.evicted,
                    ..Default::default()
                });
            }
            if let Some(body) = arrival.body {
                if let Ok(body) = crate::video::decode_video_body(&body) {
                    if !body
                        .frame
                        .belongs_to(desktop.info.codec, desktop.info.stills)
                    {
                        continue;
                    }
                    desktop.receive_video(shared, body);
                }
            }
        }
    }
}
