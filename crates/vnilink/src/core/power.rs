//! Aggregate OS leases with independently counted Desktop consumers.
use super::*;
use crate::power::*;
use crate::{BackendStartup, CloseCause, ServiceKind, ServicePhase};

pub(super) struct Factory {
    pub(super) revision: u64,
    pub(super) adapter: Option<Arc<dyn HostPowerFactory>>,
}
struct Slot {
    revision: u64,
    inhibitor: Option<Box<dyn PowerInhibitor>>,
    pending: Option<(
        BackendStartup,
        crate::service::StartupGuard,
        tokio::task::JoinHandle<Result<Box<dyn PowerInhibitor>, Error>>,
    )>,
    failure: Option<String>,
}
impl Slot {
    fn new(revision: u64) -> Self {
        Self {
            revision,
            inhibitor: None,
            pending: None,
            failure: None,
        }
    }
    fn clear(&mut self) {
        self.pending = None;
        self.inhibitor = None;
        self.failure = None;
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        self.clear();
    }
}

impl Core {
    pub fn power_policy(&self) -> Result<PowerPolicy, Error> {
        let _engine = self.engine();
        Ok(self.alive_state()?.power_policy)
    }
    /// Saved first; the resource actor reconciles live owners on its next tick.
    /// An OS rejection appears as Failed/Unavailable, never a false Active ACK.
    pub fn set_power_policy(&self, policy: PowerPolicy) -> Result<(), Error> {
        let _engine = self.engine();
        policy.validate()?;
        let mut state = self.alive_state()?;
        let previous = state.power_policy;
        state.power_policy = policy;
        if let Err(error) = persist(&self.shared, &mut state) {
            state.power_policy = previous;
            return Err(error);
        }
        Ok(())
    }
    pub fn set_power_factory(
        &self,
        factory: Option<Arc<dyn HostPowerFactory>>,
    ) -> Result<(), Error> {
        let _state = self.alive_state()?;
        let mut target = lock(&self.shared.power_factory);
        target.revision = target.revision.checked_add(1).ok_or(Error::Busy)?;
        target.adapter = factory;
        Ok(())
    }
    pub fn power_status(&self) -> Result<Vec<PowerStatus>, Error> {
        let _engine = self.engine();
        self.ensure_alive()?;
        Ok(lock(&self.shared.power_status).to_vec())
    }
}

/// Control was used: a controller's input was applied here, or a picture of
/// this computer reached a controlling viewer. Only used control locks this
/// computer when it ends; see [`LockAfterControl`].
pub(super) fn note_control_used(shared: &Shared) {
    shared.control_used.store(true, Ordering::Release);
}

pub(super) fn start(shared: Weak<Shared>) {
    tokio::spawn(async move {
        let mut slots = [Slot::new(0), Slot::new(0)];
        let mut ticker = tokio::time::interval(Duration::from_millis(25));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let Some(core) = shared.upgrade() else {
                break;
            };
            let (consumers, revision, factory, lock_fire) = {
                let mut state = lock(&core.state);
                if state.shut_down {
                    break;
                }
                super::services::synchronize(&mut state, &core.events);
                let now = std::time::Instant::now();
                let policy = state.power_policy;
                let mut expired = expire(&mut state.services, policy.outgoing_idle_secs, now);
                expired.extend(expire(
                    &mut state.hosted_services,
                    policy.incoming_idle_secs,
                    now,
                ));
                for service in expired {
                    core.events.publish(CoreEvent::ServiceChanged { service });
                }
                let mut consumers = [Vec::new(), Vec::new()];
                for (hosted, records) in [(false, &state.services), (true, &state.hosted_services)]
                {
                    for record in records.values() {
                        if record.info.kind != ServiceKind::Desktop
                            || record.info.phase != ServicePhase::Active
                            || record.desktop.is_none()
                            || *record.stop.borrow()
                        {
                            continue;
                        }
                        for kind in PowerKind::ALL {
                            if state.power_policy.wants(hosted, kind) {
                                consumers[kind.index()].push(record.stop.subscribe());
                            }
                        }
                    }
                }
                let factory = lock(&core.power_factory);
                // Lock-after-control decision: any live remote control, and
                // any live parent session to tell explicit close from
                // network loss. Sampled after synchronize/expire on the
                // same tick. A use noted since the last tick arms it even
                // when control already ended within the tick.
                let controlling = state.input_holder().is_some();
                let parent_live = state
                    .incoming
                    .values()
                    .any(|s| s.phase == SessionPhase::Active);
                let lock_enabled = state.power_policy.lock_after_control;
                let used = core.control_used.swap(false, Ordering::AcqRel);
                let lock_fire =
                    state
                        .lock_watch
                        .tick(now, lock_enabled, controlling, used, parent_live)
                        == LockDecision::Fire;
                (
                    consumers,
                    factory.revision,
                    factory.adapter.clone(),
                    lock_fire,
                )
            };
            if lock_fire {
                // State lock is released; the adapter slot is taken alone.
                // The OS call runs on a blocking worker, never here.
                let actions = lock(&core.system_actions).clone();
                let events = core.events.clone();
                tokio::task::spawn_blocking(move || {
                    let locked = actions
                        .as_ref()
                        .is_some_and(|actions| actions.lock_workstation().is_ok());
                    events.publish(CoreEvent::HostLocked { locked });
                });
            }
            for (kind, watchers) in PowerKind::ALL.into_iter().zip(consumers) {
                let slot = &mut slots[kind.index()];
                let count = watchers.len() as u32;
                if count == 0 || slot.revision != revision {
                    slot.clear();
                    slot.revision = revision;
                }
                let cancellation = slot
                    .pending
                    .as_ref()
                    .and_then(|(startup, _, _)| startup.check().err());
                if let Some(error) = cancellation {
                    slot.clear();
                    if matches!(error, Error::Timeout) {
                        slot.failure = Some("native power startup timed out".into());
                    }
                }
                if slot
                    .pending
                    .as_ref()
                    .is_some_and(|(_, _, task)| task.is_finished())
                {
                    let (startup, _guard, task) = slot.pending.take().unwrap();
                    match task.await {
                        Ok(Ok(inhibitor)) if !startup.is_cancelled() && count != 0 => {
                            slot.inhibitor = Some(inhibitor)
                        }
                        Ok(Ok(_)) => {} // Drop a late native success immediately.
                        Ok(Err(error)) => slot.failure = Some(error.to_string()),
                        Err(_) => slot.failure = Some("native power worker ended".into()),
                    }
                }
                let phase = if count == 0 {
                    PowerPhase::Idle
                } else if factory.is_none() {
                    PowerPhase::Unavailable
                } else if slot.inhibitor.is_some() {
                    PowerPhase::Active
                } else if let Some(error) = &slot.failure {
                    PowerPhase::Failed(error.clone())
                } else {
                    if slot.pending.is_none() {
                        let startup = BackendStartup::for_consumers(
                            Arc::new(AtomicBool::new(false)),
                            watchers,
                            Duration::from_secs(5),
                        );
                        let opening = startup.clone();
                        let factory = factory.clone().unwrap();
                        let task = tokio::task::spawn_blocking(move || {
                            opening.check()?;
                            let acquired = factory.acquire(kind, &opening)?;
                            opening.check()?;
                            Ok(acquired)
                        });
                        let guard = startup.guard();
                        slot.pending = Some((startup, guard, task));
                    }
                    PowerPhase::Starting
                };
                lock(&core.power_status)[kind.index()] = PowerStatus {
                    kind,
                    owners: count,
                    phase,
                };
            }
        }
        // Dropping slots cancels delayed acquisition and releases native handles.
    });
}

fn expire(
    records: &mut std::collections::HashMap<u64, super::services::ServiceRecord>,
    limit: u32,
    now: std::time::Instant,
) -> Vec<crate::ServiceInfo> {
    let mut closed = Vec::new();
    for record in records.values_mut() {
        if record.info.kind == ServiceKind::Desktop
            && record.info.phase == ServicePhase::Active
            && record
                .desktop
                .as_ref()
                .is_some_and(|desktop| lock(&desktop.activity).expired(now, limit))
            && record.change(
                record.info.generation,
                ServicePhase::Closed(CloseCause::IdleExpired),
            )
        {
            closed.push(record.info.clone());
        }
    }
    closed
}
