//! Host display topology and the pointer fence.
//!
//! The platform publishes one snapshot. A pointer names a display and the
//! revision it was mapped with. A stale revision or a display that is no
//! longer in the snapshot is not applied, and it is not retargeted onto a
//! remaining display. One desktop stream shows one selected display; another
//! display is another desktop owner.
// trace: records every ignored failure

use super::*;

use crate::input::{
    display_for_point, validate_topology, DisplayInfo, InputEvent, MAX_DESKTOP_ORIGIN,
};
use crate::protocol::{ControlMessage, SessionStamp};

/// Last published snapshot. `revision` 0 means the host has not published one.
/// An empty display list with a positive revision means every display is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologySnapshot {
    pub revision: u64,
    pub displays: Vec<DisplayInfo>,
}

impl Core {
    /// Replace the host topology.
    ///
    /// `revision` must be greater than the published revision when the geometry
    /// changes. An identical geometry keeps its revision. Display records are
    /// stamped with `revision`, so callers cannot mix revisions in one snapshot.
    /// An empty list removes every display at that revision.
    pub fn set_topology(&self, revision: u64, displays: &[DisplayInfo]) -> Result<(), Error> {
        let _engine = self.engine();
        if revision == 0 {
            return Err(Error::InvalidSetting("a display revision is invalid"));
        }
        let stamped = displays
            .iter()
            .map(|display| DisplayInfo {
                revision,
                ..*display
            })
            .collect::<Vec<_>>();
        if !stamped.is_empty() {
            validate_topology(&stamped)?;
        }
        // Keep reconciliation and publication in the same state transaction.
        // Otherwise a delayed publisher can replace a newer topology after
        // both publishers passed the revision check against the old snapshot.
        // State precedes topology in the input/selection paths as well.
        let state = self.alive_state()?;
        let previous = lock(&self.shared.topology).clone();
        if geometry(&previous.displays) == geometry(&stamped) {
            return Ok(());
        }
        if revision <= previous.revision {
            return Err(Error::InvalidSetting("the display revision must increase"));
        }
        for record in state.hosted_services.values() {
            let Some(desktop) = &record.desktop else {
                continue;
            };
            let mut selected = lock(&desktop.selected);
            let mut fault = lock(&desktop.display_fault);
            if reconcile(&mut selected, &mut fault, &previous.displays, &stamped) {
                desktop.keyframe.request();
            }
        }
        *lock(&self.shared.topology) = TopologySnapshot {
            revision,
            displays: stamped,
        };
        crate::ignored!(
            crate::trace::TOPOLOGY,
            "no viewer watches the topology now",
            self.shared.topology_watch.send(revision)
        );
        self.shared
            .events
            .publish(CoreEvent::TopologyChanged { revision });
        Ok(())
    }

    /// Fail only Desktop owners routed to an unavailable capture source.
    /// Selection and failure are one state transaction; a concurrent reselection
    /// cannot close a healthy owner. Neutral parents and Files are preserved.
    pub fn fail_desktop_source(&self, display_id: u64, message: &str) -> Result<(), Error> {
        let _engine = self.engine();
        let mut state = self.alive_state()?;
        let routing = super::video::video_routing_locked(&self.shared, &state);
        let routes_to_source = |selected: &[u64]| routing.serves_selection(selected, display_id);
        let affected: Vec<_> = state
            .hosted_services
            .values()
            .filter(|record| {
                if record.info.kind != crate::ServiceKind::Desktop || record.info.phase.finished() {
                    return false;
                }
                record
                    .desktop
                    .as_ref()
                    .is_some_and(|desktop| routes_to_source(&lock(&desktop.selected)))
            })
            .map(|record| (record.info.id, record.info.generation))
            .collect();
        for (id, generation) in affected {
            let record = state
                .hosted_services
                .get_mut(&id)
                .expect("selected under state");
            if record.change(generation, crate::ServicePhase::Failed(message.to_owned())) {
                self.shared.events.publish(CoreEvent::HostedServiceChanged {
                    service: record.info.clone(),
                });
            }
        }
        Ok(())
    }

    /// The published snapshot. Revision 0 and an empty list means none yet.
    pub fn topology(&self) -> Result<TopologySnapshot, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        Ok(lock(&self.shared.topology).clone())
    }

    /// Topology the host most recently sent to this controlling session.
    pub fn session_topology(
        &self,
        session_id: u64,
        generation: u64,
    ) -> Result<Vec<DisplayInfo>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let session = state.outgoing.get(&session_id).ok_or(Error::NoSession)?;
        if session.generation != generation {
            return Err(Error::StaleSession);
        }
        Ok(session.peer_topology.clone())
    }

    /// Displays a capture supervisor should open. One id per desktop owner,
    /// the display that owner selected. Unselected owners are absent.
    pub fn capture_displays(&self) -> Result<Vec<u64>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let mut ids = Vec::new();
        for record in state.hosted_services.values() {
            if record.info.kind != crate::ServiceKind::Desktop || *record.stop.borrow() {
                continue;
            }
            let Some(desktop) = &record.desktop else {
                continue;
            };
            if let Some(id) = lock(&desktop.selected).first().copied() {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    pub fn hosted_display_selection(&self, id: u64, generation: u64) -> Result<Vec<u64>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let desktop = super::desktop_stream::runtime(&state, id, generation, true)?;
        let selected = lock(&desktop.selected).clone();
        Ok(selected)
    }

    pub fn hosted_display_fault(
        &self,
        id: u64,
        generation: u64,
    ) -> Result<Option<&'static str>, Error> {
        let _engine = self.engine();
        let state = self.alive_state()?;
        let desktop = super::desktop_stream::runtime(&state, id, generation, true)?;
        let fault = *lock(&desktop.display_fault);
        Ok(fault)
    }
}

pub(super) fn normalize_selection(ids: &[u64]) -> Result<Vec<u64>, Error> {
    if ids.len() != 1 {
        return Err(Error::InvalidSetting("a desktop stream shows one display"));
    }
    if ids[0] == 0 {
        return Err(Error::InvalidSetting("a display id is invalid"));
    }
    Ok(vec![ids[0]])
}

pub(super) fn apply_selection(displays: &[DisplayInfo], ids: &[u64]) -> Result<Vec<u64>, Error> {
    let ids = normalize_selection(ids)?;
    if displays.is_empty() {
        return Err(Error::InputFailed("the host has no display topology"));
    }
    if displays.iter().all(|display| display.id != ids[0]) {
        return Err(Error::InputFailed(
            "the display is not in the current topology",
        ));
    }
    Ok(ids)
}

/// Drop the pointer when its revision or display is no longer the one the
/// viewer selected. Keys and buttons are not geometry.
pub(super) fn accept_pointer(
    shared: &Shared,
    event: InputEvent,
    selected: &[u64],
) -> Result<(), Error> {
    let InputEvent::Pointer {
        x,
        y,
        display,
        revision,
    } = event
    else {
        return Ok(());
    };
    if display == 0
        || revision == 0
        || x.unsigned_abs() > u32::try_from(MAX_DESKTOP_ORIGIN).unwrap_or(u32::MAX)
        || y.unsigned_abs() > u32::try_from(MAX_DESKTOP_ORIGIN).unwrap_or(u32::MAX)
    {
        return Err(Error::InputFailed("the pointer is outside the desktop"));
    }
    let snapshot = lock(&shared.topology);
    if snapshot.revision == 0 {
        return Err(Error::InputFailed("the host has no display topology"));
    }
    if revision != snapshot.revision {
        return Err(Error::InputFailed(
            "the pointer uses an outdated display topology",
        ));
    }
    if snapshot.displays.iter().all(|item| item.id != display) {
        return Err(Error::InputFailed(
            "the display is not in the current topology",
        ));
    }
    match display_for_point(x, y, &snapshot.displays) {
        Some(found) if found.id == display => {}
        _ => {
            return Err(Error::InputFailed(
                "the pointer is not on the named display",
            ))
        }
    }
    if selected.is_empty() {
        return Err(Error::InputFailed("no display is selected"));
    }
    if !selected.contains(&display) {
        return Err(Error::InputFailed("that display is not selected"));
    }
    Ok(())
}

/// Topology received on a desktop service stream. The service generation matches
/// the parent connection. The desktop stamp is not the parent wire stamp.
pub(super) fn store_service_topology(
    shared: &Shared,
    connection_id: u64,
    generation: u64,
    revision: u64,
    displays: Vec<DisplayInfo>,
) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.get_mut(&connection_id) else {
        return;
    };
    if session.generation != generation || session.phase != crate::session::SessionPhase::Active {
        return;
    }
    session.peer_topology = displays;
    drop(state);
    shared
        .events
        .publish(CoreEvent::TopologyChanged { revision });
}

pub(super) fn store_peer_topology(
    shared: &Shared,
    local_id: u64,
    stamp: SessionStamp,
    revision: u64,
    displays: Vec<DisplayInfo>,
) {
    let mut state = lock(&shared.state);
    let Some(session) = state.outgoing.get_mut(&local_id) else {
        return;
    };
    if session.wire_stamp != stamp || session.phase != crate::session::SessionPhase::Active {
        return;
    }
    session.peer_topology = displays;
    drop(state);
    shared
        .events
        .publish(CoreEvent::TopologyChanged { revision });
}

pub(super) async fn write_topology(
    shared: &Shared,
    send: &mut iroh::endpoint::SendStream,
    stamp: SessionStamp,
) -> Result<(), Error> {
    let snapshot = lock(&shared.topology).clone();
    if snapshot.revision == 0 {
        return Ok(());
    }
    super::wire::write_frame(
        send,
        &ControlMessage::Topology {
            stamp,
            revision: snapshot.revision,
            displays: snapshot.displays,
        },
    )
    .await
}

fn reconcile(
    selected: &mut Vec<u64>,
    fault: &mut Option<&'static str>,
    previous: &[DisplayInfo],
    next: &[DisplayInfo],
) -> bool {
    if selected.is_empty() {
        return false;
    }
    let mut keyframe = false;
    let before = selected.len();
    selected.retain(|id| next.iter().any(|display| display.id == *id));
    if selected.len() != before {
        *fault = Some(if selected.is_empty() {
            "the selected display is gone"
        } else {
            "a selected display is gone"
        });
        keyframe = true;
    }
    for id in selected.iter() {
        let old = previous.iter().find(|display| display.id == *id);
        let new = next.iter().find(|display| display.id == *id);
        if let (Some(old), Some(new)) = (old, new) {
            if old.origin_x != new.origin_x
                || old.origin_y != new.origin_y
                || old.width != new.width
                || old.height != new.height
                || old.rotation_deg != new.rotation_deg
            {
                keyframe = true;
            }
        }
    }
    keyframe
}

fn geometry(displays: &[DisplayInfo]) -> Vec<(u64, i32, i32, u16, u16, u16, u16)> {
    let mut items = displays
        .iter()
        .map(|display| {
            (
                display.id,
                display.origin_x,
                display.origin_y,
                display.width,
                display.height,
                display.rotation_deg,
                display.scale_percent,
            )
        })
        .collect::<Vec<_>>();
    items.sort_unstable();
    items
}
