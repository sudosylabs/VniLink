// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Pointer and scroll samples on unreliable datagrams.
//!
//! A viewer sends where its pointer is, and each scroll step, as one
//! datagram each ([`crate::InputEvent::sampled`]); keys, buttons and typed
//! text stay on the desktop's stream, reliable and in order. A datagram is
//! `magic | seq u32 | the stream's own command body`, so the host admits it
//! through the same lease checks as a stream command.
//!
//! The network may lose, reorder or delay a datagram, and the host takes
//! that as follows:
//! * A pointer position older than one already applied is dropped: the
//!   host applies where the pointer is now, not where it passed. A lost
//!   one is replaced by the next.
//! * A scroll step is applied whenever it arrives. A lost step scrolls a
//!   little less; steps add, so order does not matter.
//! * Before each button, after a display selection, and once the pointer
//!   has rested for [`super::desktop_stream`]'s settle time, the viewer
//!   says its newest position on the stream. A click lands where the
//!   pointer was, a pointer never waits on a display not yet selected, and
//!   a lost last sample is made up. Such a position may be applied after a
//!   newer datagram's; the next sample, or the rest, corrects it.
//!
//! One reader per incoming connection owns `read_datagram`, as the viewer's
//! video router does for pictures.
// trace: records every ignored failure

use std::collections::HashMap;
use std::sync::Mutex;

use bytes::Bytes;
use iroh::endpoint::Connection;

use super::desktop_stream::{apply_host_batch, DesktopRuntime, HostCommand, COMMAND};
use super::*;
use crate::protocol::{ControlMessage, SessionStamp};

const MAGIC: &[u8; 4] = b"INP1";
const HEADER: usize = MAGIC.len() + 4;
/// Datagrams taken from the connection at once.
const BATCH: usize = 32;
/// How often the router checks its connection is still the live one.
const LIVENESS: Duration = Duration::from_millis(100);

/// The datagram for one command `body` (as written on the stream), numbered
/// `seq`.
pub(super) fn encode(seq: u32, body: &[u8]) -> Bytes {
    let mut bytes = Vec::with_capacity(HEADER + body.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&seq.to_be_bytes());
    bytes.extend_from_slice(body);
    Bytes::from(bytes)
}

fn decode(bytes: &[u8]) -> Option<(u32, &[u8])> {
    if bytes.len() <= HEADER || bytes[..MAGIC.len()] != *MAGIC || bytes[HEADER] != COMMAND {
        return None;
    }
    Some((
        u32::from_be_bytes(bytes[MAGIC.len()..HEADER].try_into().ok()?),
        &bytes[HEADER..],
    ))
}

/// Whether `seq` follows `last` in wrapping order.
fn newer(seq: u32, last: u32) -> bool {
    seq.wrapping_sub(last) as i32 > 0
}

/// Which pointer position a desktop took last, so that no datagram older
/// than it is applied after it.
#[derive(Default)]
pub(super) struct PointerOrder(Mutex<Option<u32>>);

impl PointerOrder {
    /// Whether a position sent as a datagram numbered `seq` is newer than
    /// any taken; it is then the newest.
    fn datagram(&self, seq: u32) -> bool {
        let mut last = lock(&self.0);
        let fresh = last.is_none_or(|last| newer(seq, last));
        if fresh {
            *last = Some(seq);
        }
        fresh
    }
}

/// One datagram, as the command it carries and the desktop it is for.
fn parse(bytes: &[u8]) -> Option<(SessionStamp, u32, HostCommand)> {
    let (seq, body) = decode(bytes)?;
    let command = HostCommand::new(body.to_vec());
    let Some(Ok(ControlMessage::Input { stamp, event })) = &command.command else {
        return None;
    };
    // The datagram path may not carry a key or button.
    let (stamp, sampled) = (*stamp, event.sampled());
    sampled.then_some((stamp, seq, command))
}

/// What a desktop applies of the samples one read brought: the newest
/// pointer position if it is newer than any applied, and every scroll step,
/// in arrival order.
fn newest(order: &PointerOrder, samples: Vec<(u32, HostCommand)>) -> Vec<HostCommand> {
    let pointer = |command: &HostCommand| {
        matches!(
            command.command,
            Some(Ok(ControlMessage::Input {
                event: crate::InputEvent::Pointer { .. },
                ..
            }))
        )
    };
    let mut commands: Vec<HostCommand> = Vec::new();
    let mut at = None;
    for (seq, command) in samples {
        if pointer(&command) {
            if at.is_some_and(|(held, _)| !newer(seq, held)) || !order.datagram(seq) {
                continue;
            }
            // The newest position replaces an older one still waiting.
            commands.retain(|waiting| !pointer(waiting));
            at = Some((seq, ()));
        }
        commands.push(command);
    }
    commands
}

/// Starts the connection's sole input-datagram reader, once per (connection,
/// generation). It exits when the connection closes or its session ends.
pub(super) fn ensure_router(
    shared: &Arc<Shared>,
    link: Connection,
    connection_id: u64,
    generation: u64,
) {
    if !lock(&shared.input_routers).insert((connection_id, generation)) {
        return;
    }
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        route(&shared, &link, connection_id, generation).await;
        lock(&shared.input_routers).remove(&(connection_id, generation));
    });
}

async fn route(shared: &Arc<Shared>, link: &Connection, connection_id: u64, generation: u64) {
    let mut owners: HashMap<u64, Arc<DesktopRuntime>> = HashMap::new();
    let mut checked: Option<std::time::Instant> = None;
    let mut batch = vec![Bytes::new(); BATCH];
    loop {
        if checked.is_none_or(|at| at.elapsed() >= LIVENESS) {
            let state = lock(&shared.state);
            let live = !state.shut_down
                && state.incoming.get(&connection_id).is_some_and(|session| {
                    session.generation == generation && !finished(&session.phase)
                });
            if !live {
                return;
            }
            owners.retain(|_, desktop| !desktop.retired.load(std::sync::atomic::Ordering::Acquire));
            checked = Some(std::time::Instant::now());
        }
        let count = match link.read_many_datagrams(&mut batch).await {
            Ok(count) => count,
            Err(_) => return,
        };
        let mut arrived: Vec<(SessionStamp, Vec<(u32, HostCommand)>)> = Vec::new();
        for bytes in &batch[..count] {
            let Some((stamp, seq, command)) = parse(bytes) else {
                continue;
            };
            match arrived.iter_mut().find(|(owner, _)| *owner == stamp) {
                Some((_, samples)) => samples.push((seq, command)),
                None => arrived.push((stamp, vec![(seq, command)])),
            }
        }
        for (stamp, samples) in arrived {
            let desktop = owners
                .get(&stamp.id)
                .filter(|desktop| desktop.stamp == stamp)
                .cloned()
                .or_else(|| {
                    let state = lock(&shared.state);
                    state.hosted_services.values().find_map(|record| {
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
            let Some(desktop) = desktop else { continue };
            owners.insert(stamp.id, Arc::clone(&desktop));
            let commands = newest(&desktop.pointer_order, samples);
            // The platform is called from a blocking thread, once for the
            // read. A sample refused for its lease or its target is the
            // host's to record, not the stream's to end.
            let shared = Arc::clone(shared);
            let applied = tokio::task::spawn_blocking(move || {
                let mut refused = None;
                for command in commands {
                    if let Err(error) = apply_host_batch(&shared, &desktop, vec![command]) {
                        refused = Some(error);
                    }
                }
                refused.map_or(Ok(()), Err)
            })
            .await;
            if let Ok(result) = applied {
                crate::ignored!(
                    crate::trace::INPUT,
                    "a sample for a lease that has changed is dropped, and the next one asks again",
                    result
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InputEvent;

    const EPOCH: u64 = 7;

    fn sample(stamp: SessionStamp, seq: u32, event: InputEvent) -> Bytes {
        let mut body = vec![COMMAND];
        body.extend_from_slice(&EPOCH.to_be_bytes());
        body.extend_from_slice(&ControlMessage::Input { stamp, event }.encode());
        encode(seq, &body)
    }

    fn moved(x: i32) -> InputEvent {
        InputEvent::pointer(x, 0, 1, 1)
    }

    /// What a desktop is told to apply, as the events.
    fn events(commands: Vec<HostCommand>) -> Vec<InputEvent> {
        commands
            .into_iter()
            .filter_map(|command| match command.command {
                Some(Ok(ControlMessage::Input { event, .. })) => Some(event),
                _ => None,
            })
            .collect()
    }

    /// What a desktop is told to apply of one read, as events.
    fn read(order: &PointerOrder, datagrams: &[Bytes]) -> Vec<InputEvent> {
        let samples = datagrams
            .iter()
            .filter_map(|bytes| parse(bytes))
            .map(|(_, seq, command)| (seq, command))
            .collect();
        events(newest(order, samples))
    }

    /// A pointer position older than one applied is dropped, one read's
    /// positions collapse to the newest, scroll steps all apply in arrival
    /// order, and what is not a sample (a key) is refused.
    #[test]
    fn a_stale_pointer_is_dropped_and_scroll_steps_all_apply() {
        let stamp = SessionStamp::new(3, 1);
        let order = PointerOrder::default();
        let first = read(
            &order,
            &[
                sample(stamp, 5, moved(50)),
                sample(stamp, 4, moved(40)),
                sample(stamp, 6, InputEvent::wheel(0, 1)),
                sample(stamp, 7, moved(70)),
                sample(stamp, 8, InputEvent::key_down(crate::usage::A)),
                sample(stamp, 9, InputEvent::wheel(0, 2)),
            ],
        );
        let second = read(
            &order,
            &[sample(stamp, 3, moved(30)), sample(stamp, 10, moved(100))],
        );
        assert_eq!(
            first,
            [InputEvent::wheel(0, 1), moved(70), InputEvent::wheel(0, 2)],
            "the older position is dropped, the newest of the read stays where it arrived, the key is refused"
        );
        assert_eq!(second, [moved(100)], "30 is stale, 100 is new");
    }

    /// The sequence wraps: a position just after the wrap is newer than one
    /// just before it.
    #[test]
    fn pointer_order_survives_the_sequence_wrapping() {
        let stamp = SessionStamp::new(3, 1);
        let order = PointerOrder::default();
        let applied = read(
            &order,
            &[
                sample(stamp, u32::MAX, moved(1)),
                sample(stamp, 0, moved(2)),
                sample(stamp, u32::MAX - 1, moved(0)),
            ],
        );
        assert_eq!(applied, [moved(2)]);
    }
}
