//! Live diagnostic snapshot. Reads state under one short lock; counts and
//! labels only, never payload bytes. Failure messages are reduced to fixed
//! labels so a snapshot cannot smuggle paths or addresses.

use super::*;
use crate::diagnostics::{
    build_snapshot, fingerprint, monotonic_ms, ConnectionSummary, DiagnosticSnapshot,
    PipelineSummary, QueueDepths, ResourceCounts, ServiceSummary, SnapshotInputs,
};
use crate::{ServiceKind, ServicePhase, SessionPhase, SessionRole};

fn session_phase(phase: &SessionPhase) -> &'static str {
    match phase {
        SessionPhase::Connecting => "connecting",
        SessionPhase::Authenticated => "authenticated",
        SessionPhase::Authorized => "authorized",
        SessionPhase::Negotiating => "negotiating",
        SessionPhase::Active => "active",
        SessionPhase::Reconnecting => "reconnecting",
        SessionPhase::Stopping => "stopping",
        SessionPhase::Closed => "closed",
        SessionPhase::Failed(_) => "failed",
    }
}

fn service_phase(phase: &ServicePhase) -> &'static str {
    match phase {
        ServicePhase::Waiting => "waiting",
        ServicePhase::Active => "active",
        ServicePhase::Suspended => "suspended",
        ServicePhase::Closed(_) => "closed",
        ServicePhase::Failed(_) => "failed",
    }
}

fn service_kind(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Desktop => "desktop",
        ServiceKind::Files => "files",
        ServiceKind::Terminal => "terminal",
        ServiceKind::Camera => "camera",
        ServiceKind::SystemAudio => "system-audio",
        ServiceKind::Microphone => "microphone",
        ServiceKind::System => "system",
        ServiceKind::ScreenSnapshot => "screen-snapshot",
    }
}

impl Core {
    /// Point-in-time diagnostic snapshot: stages, phases, counts and queue
    /// depths. Contains no screen, clipboard, terminal, token, secret or
    /// path payload. Export with
    /// [`export_json`](crate::diagnostics::export_json).
    pub fn diagnostic_snapshot(&self) -> Result<DiagnosticSnapshot, Error> {
        // Jobs come before state in the lock order (`interrupt_orphaned`),
        // so they are counted before the state lock is taken.
        let active_jobs = lock(&self.shared.jobs)
            .values()
            .filter(|record| super::jobs::ACTIVE_STATES.contains(&record.state))
            .count() as u32;
        let state = self.alive_state()?;
        let mut connections = Vec::with_capacity(state.incoming.len() + state.outgoing.len());
        for session in state.incoming.values().chain(state.outgoing.values()) {
            let services = state
                .services
                .values()
                .filter(|record| record.info.connection_id == session.id)
                .count()
                + state
                    .hosted_services
                    .values()
                    .filter(|record| record.info.connection_id == session.id)
                    .count();
            connections.push(ConnectionSummary {
                id: session.id,
                role: match session.role {
                    SessionRole::Controller => "controller",
                    SessionRole::Host => "host",
                }
                .into(),
                phase: session_phase(&session.phase).into(),
                generation: session.generation,
                peer: fingerprint(&session.peer_key),
                services: u32::try_from(services).unwrap_or(u32::MAX),
                path: "none".into(),
                rtt_ms: None,
                tx_bytes: 0,
                rx_bytes: 0,
            });
        }
        connections.sort_by_key(|summary| summary.id);
        let mut services = Vec::with_capacity(state.services.len() + state.hosted_services.len());
        let mut runtimes = Vec::new();
        for record in state.services.values() {
            services.push(ServiceSummary {
                id: record.info.id,
                connection_id: record.info.connection_id,
                kind: service_kind(record.info.kind).into(),
                phase: service_phase(&record.info.phase).into(),
                hosted: false,
            });
            if let Some(desktop) = record.desktop.clone() {
                runtimes.push((record.info.id, false, desktop));
            }
        }
        for record in state.hosted_services.values() {
            services.push(ServiceSummary {
                id: record.info.id,
                connection_id: record.info.connection_id,
                kind: service_kind(record.info.kind).into(),
                phase: service_phase(&record.info.phase).into(),
                hosted: true,
            });
            if let Some(desktop) = record.desktop.clone() {
                runtimes.push((record.info.id, true, desktop));
            }
        }
        services.sort_by_key(|summary| (summary.connection_id, summary.id));
        let input_queued: usize = state
            .outgoing
            .values()
            .filter_map(|session| session.outbox.as_ref())
            .map(|outbox| outbox.depth())
            .sum();
        let resources = ResourceCounts {
            incoming_connections: state.incoming.len() as u32,
            outgoing_connections: state.outgoing.len() as u32,
            controller_services: state.services.len() as u32,
            hosted_services: state.hosted_services.len() as u32,
            terminal_sessions: lock(&self.shared.terminal_sessions).len() as u32,
            retained_operations: state.system_operations.len() as u32,
            privacy_held: state.privacy.is_some(),
            active_jobs,
            paired_devices: state.trust.devices().count() as u32,
            control_connection: state.input_holder(),
        };
        let queues = QueueDepths {
            input_queued: u32::try_from(input_queued).unwrap_or(u32::MAX),
            terminal_startups_pending: state.terminal_pending.values().sum::<usize>() as u32,
            files_serving: lock(&self.shared.files_serving).len() as u32,
        };
        let hosting = state.hosting;
        drop(state);
        // Transport paths and pipeline telemetry sample live handles outside
        // the state lock; each entry is internally consistent.
        for summary in &mut connections {
            let paths = self.session_paths(summary.id).unwrap_or_default();
            let selected = paths.iter().find(|path| path.selected);
            summary.path = match selected {
                Some(path) if path.relay => "relay",
                Some(_) => "direct",
                None => "none",
            }
            .into();
            summary.rtt_ms = selected.map(|path| path.rtt_ms);
            summary.tx_bytes = paths.iter().map(|path| path.tx_bytes).sum();
            summary.rx_bytes = paths.iter().map(|path| path.rx_bytes).sum();
        }
        let now = std::time::Instant::now();
        let mut pipelines: Vec<PipelineSummary> = runtimes
            .iter()
            .map(|(id, hosted, desktop)| desktop.pipeline_summary(*id, *hosted, now))
            .collect();
        pipelines.sort_by_key(|summary| (summary.hosted, summary.service_id));
        let snapshot = build_snapshot(SnapshotInputs {
            monotonic_ms: monotonic_ms(),
            hosting,
            connections,
            services,
            pipelines,
            resources,
            queues,
            transfers: self.shared.transfer_counters.summary(),
        });
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A snapshot waiting for the jobs lock holds no state lock, so a job
    /// poll that holds jobs and then takes state cannot deadlock with it.
    #[test]
    fn a_snapshot_waits_for_jobs_without_holding_state() {
        let core = crate::tests::support::start_direct();
        let jobs = lock(&core.shared.jobs);
        std::thread::scope(|scope| {
            let snapshot = scope.spawn(|| core.diagnostic_snapshot().map(|_| ()));
            std::thread::sleep(std::time::Duration::from_millis(100));
            let state_free = core.shared.state.try_lock().is_ok();
            drop(jobs);
            snapshot.join().unwrap().unwrap();
            assert!(
                state_free,
                "the snapshot holds state while it waits for jobs"
            );
        });
    }
}
