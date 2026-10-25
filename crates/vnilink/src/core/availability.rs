//! Installation-level capability availability. Pure combination lives in
//! [`crate::availability`]; this wires the facts the core actually tracks:
//! installed backends, published backend state, durable policies, peer
//! grants and live counts. OS denial, device presence and backend failure
//! come only from the registry: adapters publish what they observe and
//! serve loops record factory failures they hit. Unpublished stays
//! unknown, reported as no blocker, and operations still fail per call
//! with their own reasons.

use super::*;
use crate::availability::{evaluate, Availability, AvailabilityInputs, BackendState, Capability};
use crate::session::Grant;
use crate::ServiceKind;

impl Core {
    /// Publishes last-known backend facts for one capability, replacing any
    /// earlier entry. Native adapters report OS consent and device presence
    /// they can see; unknown stays unpublished rather than guessed.
    pub fn report_backend_state(
        &self,
        capability: Capability,
        state: BackendState,
    ) -> Result<(), Error> {
        self.ensure_alive()?;
        let mut state = state;
        if let Some(failed) = state.failed.take() {
            state.failed = Some(crate::availability::truncate(failed));
        }
        lock(&self.shared.backend_state).insert(capability, state);
        Ok(())
    }

    /// Updates OS consent without clearing a separately observed device or backend failure.
    pub fn report_os_permission(&self, capability: Capability, denied: bool) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        lock(&self.shared.backend_state)
            .entry(capability)
            .or_default()
            .os_denied = denied;
        Ok(())
    }
    pub fn report_backend_failure(
        &self,
        capability: Capability,
        failure: Option<String>,
    ) -> Result<(), Error> {
        self.ensure_alive()?;
        lock(&self.shared.backend_state)
            .entry(capability)
            .or_default()
            .failed = failure.map(crate::availability::truncate);
        Ok(())
    }

    /// Drops any published backend facts for one capability. The query
    /// returns to unknown (no blocker) for OS, device and failure facts.
    pub fn clear_backend_state(&self, capability: Capability) -> Result<(), Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        lock(&self.shared.backend_state).remove(&capability);
        Ok(())
    }
}

/// Records a factory failure a serve loop hit, preserving any adapter
/// OS/device facts already published for the capability.
pub(super) fn note_backend_failure(shared: &Shared, capability: Capability, detail: String) {
    lock(&shared.backend_state)
        .entry(capability)
        .or_default()
        .failed
        .replace(crate::availability::truncate(detail));
}

/// Clears only the failure detail after a fresh success, preserving any
/// adapter OS/device facts already published for the capability.
pub(super) fn note_backend_recovered(shared: &Shared, capability: Capability) {
    if let Some(entry) = lock(&shared.backend_state).get_mut(&capability) {
        entry.failed = None;
    }
}

impl Core {
    /// Effective availability of one capability for one peer, or for this
    /// installation when `endpoint_id` is `None` (grant checks skipped).
    /// Counts are a snapshot, not a reservation; opening the capability
    /// re-checks everything.
    pub fn capability_availability(
        &self,
        endpoint_id: Option<&str>,
        capability: Capability,
    ) -> Result<Availability, Error> {
        let peer = match endpoint_id {
            Some(id) => Some(parse_endpoint_id(id)?),
            None => None,
        };
        let state = self.alive_state()?;
        evaluate_capability(&self.shared, &state, peer, capability)
    }
}

/// [`Core::capability_availability`] for callers that hold the state lock,
/// such as the host reporting to a peer over its link.
pub(super) fn evaluate_capability(
    shared: &Shared,
    state: &State,
    peer: Option<[u8; 32]>,
    capability: Capability,
) -> Result<Availability, Error> {
    if let Some(peer) = peer {
        if state.trust.get(&peer).is_none() {
            return Err(Error::NotPaired);
        }
    }
    let granted =
        |grant: Grant| -> bool { peer.is_none_or(|peer| state.trust.granted(&peer, grant)) };
    let service_granted = |kind: ServiceKind| -> bool {
        peer.is_none_or(|peer| state.trust.service_granted(&peer, kind))
    };
    let view_ready = lock(&shared.video_caps).can_encode()
        && monitor_size(&shared) != (0, 0)
        && lock(&shared.capture).is_some();
    let camera_turned_off = shared
        .camera_turned_off
        .load(std::sync::atomic::Ordering::Acquire);
    let implemented = match capability {
        Capability::DesktopView => view_ready,
        Capability::Clipboard => {
            view_ready
                && shared
                    .clipboard_available
                    .load(std::sync::atomic::Ordering::Acquire)
        }
        Capability::DesktopControl => view_ready && lock(&shared.desktop_info).is_some(),
        Capability::Files | Capability::Recording => true,
        Capability::Terminal => lock(&shared.terminal_factory).is_some(),
        Capability::Camera => lock(&shared.camera_factory).is_some() || camera_turned_off,
        Capability::SystemAudio | Capability::Microphone => {
            let factory = lock(&shared.audio_factory);
            factory.as_ref().is_some_and(|factory| {
                factory.supports(match capability {
                    Capability::SystemAudio => crate::audio::AudioKind::System,
                    _ => crate::audio::AudioKind::Microphone,
                })
            })
        }
        Capability::SystemLock
        | Capability::SystemRestart
        | Capability::SystemShutdown
        | Capability::SystemPrivacy => {
            lock(&shared.system_actions)
                .as_ref()
                .is_some_and(|actions| {
                    let support = actions.support();
                    // Each action stands on its own native bit: an
                    // installed backend that cannot restart must read
                    // missing-implementation, not available. Privacy
                    // aggregates two facilities; either one behind it
                    // counts as implemented.
                    match capability {
                        Capability::SystemLock => support.lock,
                        Capability::SystemRestart => support.restart,
                        Capability::SystemShutdown => support.shutdown,
                        Capability::SystemPrivacy => support.privacy || support.input_block,
                        _ => false,
                    }
                })
        }
    };
    let policy_allowed = match capability {
        Capability::SystemLock => state
            .system_policy
            .allows(crate::system::SystemAction::Lock),
        Capability::SystemRestart => state
            .system_policy
            .allows(crate::system::SystemAction::Restart),
        Capability::SystemShutdown => state
            .system_policy
            .allows(crate::system::SystemAction::Shutdown),
        Capability::SystemPrivacy => state.system_policy.allow_privacy,
        Capability::Camera => !camera_turned_off,
        _ => true,
    };
    let peer_granted = match capability {
        Capability::DesktopView => granted(Grant::View),
        Capability::DesktopControl => granted(Grant::Control),
        Capability::Clipboard => granted(Grant::Clipboard),
        Capability::Files => service_granted(ServiceKind::Files),
        Capability::Terminal => service_granted(ServiceKind::Terminal),
        Capability::Camera => service_granted(ServiceKind::Camera),
        Capability::SystemAudio => service_granted(ServiceKind::SystemAudio),
        Capability::Microphone => service_granted(ServiceKind::Microphone),
        Capability::SystemLock
        | Capability::SystemRestart
        | Capability::SystemShutdown
        | Capability::SystemPrivacy => service_granted(ServiceKind::System),
        Capability::Recording => peer.is_none_or(|peer| {
            state
                .trust
                .service_permissions(&peer)
                .is_ok_and(|permissions| permissions.recording)
        }),
    };
    let services_used = u32::try_from(state.services.len()).unwrap_or(u32::MAX);
    let capacity = match capability {
        Capability::Terminal => Some((
            u32::try_from(lock(&shared.terminal_sessions).len()).unwrap_or(u32::MAX),
            u32::from(state.terminal_policy.max_sessions),
        )),
        Capability::Recording => None,
        _ => Some((services_used, crate::MAX_SERVICE_RECORDS as u32)),
    };
    let backend = lock(&shared.backend_state)
        .get(&capability)
        .cloned()
        .unwrap_or_default();
    Ok(evaluate(
        capability,
        &AvailabilityInputs {
            implemented,
            os_denied: backend.os_denied,
            device_missing: backend.device_missing,
            policy_allowed,
            peer_granted,
            capacity,
            backend_failed: backend.failed,
        },
    ))
}

/// The coarse report a peer may see about every capability on this computer.
pub(super) fn remote_report(
    shared: &Shared,
    peer: &[u8; 32],
) -> Vec<(Capability, crate::RemoteStatus)> {
    let state = lock(&shared.state);
    Capability::ALL
        .iter()
        .filter_map(|capability| {
            evaluate_capability(shared, &state, Some(*peer), *capability)
                .ok()
                .map(|availability| (*capability, availability.remote_status()))
        })
        .collect()
}
