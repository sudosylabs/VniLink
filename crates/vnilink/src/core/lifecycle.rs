//! Open, shutdown, identity, and relay.

use super::*;

use super::host::{accept_loop, bind_endpoint};

use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use iroh::{RelayMode, SecretKey};

use crate::error::Error;
use crate::events::{EventHub, EventSubscription};
use crate::installation::{self, InstallationStore};
use crate::invitation::ReplayCache;
use crate::keyboard::{self, KeyboardMode};
use crate::protocol::Capabilities;
use crate::relay::RelaySettings;
use crate::trust::Trust;

impl Core {
    pub fn start(config: CoreConfig) -> Result<Self, Error> {
        let relay_mode = config.relay.to_relay_mode()?;
        Self::boot(config, relay_mode)
    }

    /// Loads the installation record and binds an endpoint.
    ///
    /// An empty store creates an identity, saves it, and uses
    /// [`OpenConfig::initial_relay`]. The next open restores that identity,
    /// the paired devices, their grants, used invitations, and the relay
    /// setting. The host implements [`InstallationStore`]. The production
    /// adapter keeps the record in an encrypted database and the key in the
    /// platform credential store. [`crate::MemoryStore`] keeps the bytes only
    /// in this process.
    ///
    /// A locked, unavailable, or invalid store leaves any existing bytes
    /// where they are. The saved relay is the one that is bound, including
    /// the public n0 relays when that is the saved setting.
    pub fn open(store: Arc<dyn InstallationStore>, config: OpenConfig) -> Result<Self, Error> {
        Self::open_with(store, config, false)
    }

    #[cfg(any(test, feature = "test-probes"))]
    pub fn open_direct(
        store: Arc<dyn InstallationStore>,
        initial_relay: RelaySettings,
    ) -> Result<Self, Error> {
        Self::open_with(
            store,
            OpenConfig {
                listen: Listen::Loopback,
                event_capacity: 64,
                initial_relay,
            },
            true,
        )
    }

    /// `direct_only` is loopback-test scaffolding: the product exposes the
    /// public default and custom relays alone, and always passes false here.
    pub(crate) fn boot(config: CoreConfig, relay_mode: RelayMode) -> Result<Self, Error> {
        let direct_only = matches!(&relay_mode, RelayMode::Disabled);
        Self::boot_installed(config, relay_mode, direct_only, None)
    }

    /// `direct_only` is loopback-test scaffolding beside `boot`: the saved
    /// relay decides the bound mode in the product, which always passes false.
    pub(super) fn open_with(
        store: Arc<dyn InstallationStore>,
        config: OpenConfig,
        direct_only: bool,
    ) -> Result<Self, Error> {
        let loaded = match store.load()? {
            None => None,
            Some(bytes) => Some(installation::decode(&bytes)?),
        };
        let fresh = loaded.is_none();
        let (
            identity,
            relay,
            trust,
            replay,
            terminal_policy,
            file_retention_policy,
            power_policy,
            file_resource_policy,
            system_policy,
        ) = match loaded {
            None => (
                None,
                config.initial_relay.clone(),
                Trust::default(),
                ReplayCache::new(),
                crate::terminal::TerminalPolicy::default(),
                crate::job::FileRetentionPolicy::default(),
                crate::power::PowerPolicy::default(),
                crate::job::FileResourcePolicy::default(),
                crate::system::SystemPolicy::default(),
            ),
            Some(record) => (
                Some(record.secret),
                record.relay,
                record.trust,
                record.replay,
                record.terminal_policy,
                record.file_retention_policy,
                record.power_policy,
                record.file_resource_policy,
                record.system_policy,
            ),
        };
        let relay_mode = if direct_only {
            RelayMode::Disabled
        } else {
            relay.to_relay_mode()?
        };
        let core = Self::boot_installed(
            CoreConfig {
                relay: relay.clone(),
                listen: config.listen,
                event_capacity: config.event_capacity,
                identity,
            },
            relay_mode,
            direct_only,
            Some(Installed {
                power_policy,
                file_resource_policy,
                file_retention_policy,
                terminal_policy,
                system_policy,
                trust,
                replay,
                record_relay: relay,
                store: Some(store),
            }),
        )?;
        if fresh {
            let saved = {
                let mut state = lock(&core.shared.state);
                persist(&core.shared, &mut state)
            };
            if let Err(error) = saved {
                core.shutdown();
                return Err(error);
            }
        }
        Ok(core)
    }

    pub(super) fn boot_installed(
        config: CoreConfig,
        relay_mode: RelayMode,
        direct_only: bool,
        installed: Option<Installed>,
    ) -> Result<Self, Error> {
        let event_capacity = match config.event_capacity {
            1..=1024 => config.event_capacity,
            0 => return Err(Error::InvalidSetting("event capacity must be at least 1")),
            _ => return Err(Error::InvalidSetting("event capacity must be at most 1024")),
        };
        let secret = match config.identity {
            Some(bytes) => SecretKey::from_bytes(&bytes),
            None => SecretKey::generate(),
        };
        let bound_relay = config.relay.clone();
        let (
            trust,
            replay,
            record_relay,
            store,
            terminal_policy,
            file_retention_policy,
            power_policy,
            file_resource_policy,
            system_policy,
        ) = match installed {
            Some(installed) => (
                installed.trust,
                installed.replay,
                installed.record_relay,
                installed.store,
                installed.terminal_policy,
                installed.file_retention_policy,
                installed.power_policy,
                installed.file_resource_policy,
                installed.system_policy,
            ),
            None => (
                Trust::default(),
                ReplayCache::new(),
                bound_relay.clone(),
                None,
                crate::terminal::TerminalPolicy::default(),
                crate::job::FileRetentionPolicy::default(),
                crate::power::PowerPolicy::default(),
                crate::job::FileResourcePolicy::default(),
                crate::system::SystemPolicy::default(),
            ),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("vnilink")
            .build()
            .map_err(Error::network)?;
        let runtime_handle = runtime.handle().clone();
        let listen = if direct_only {
            Listen::Loopback
        } else {
            config.listen
        };
        let endpoint = runtime.block_on(bind_endpoint(
            secret.clone(),
            relay_mode,
            listen,
            direct_only || !bound_relay.is_public(),
        ))?;
        let input_cleanup = Arc::new(AtomicUsize::new(0));
        let cleanup_executor = input_cleanup::Executor::new().map_err(Error::network)?;
        let shared = Arc::new_cyclic(move |weak| Shared {
            sharing_serial: Mutex::new(()),
            power_factory: Mutex::new(power::Factory {
                revision: 0,
                adapter: None,
            }),
            power_status: Mutex::new(
                crate::power::PowerKind::ALL.map(crate::power::PowerStatus::idle),
            ),
            file_retention_policy: Arc::new(Mutex::new(file_retention_policy)),
            endpoint,
            secret,
            relay: bound_relay,
            store,
            runtime_handle,
            self_weak: weak.clone(),
            teardown_phase: AtomicU8::new(0),
            teardown_lock: Mutex::new(()),
            teardown_cv: Condvar::new(),
            #[cfg(any(test, feature = "test-probes"))]
            close_count: AtomicUsize::new(0),
            #[cfg(test)]
            teardown_budget_ms: AtomicU64::new(0),
            #[cfg(test)]
            block_blob_shutdown: AtomicBool::new(false),
            #[cfg(test)]
            block_endpoint_close: AtomicBool::new(false),
            events: EventHub::new(event_capacity),
            state: Mutex::new(State {
                input_cleanup: input_cleanup.clone(),
                power_policy,
                file_resource_policy,
                file_retention_policy,
                terminal_policy,
                system_policy,
                desktop_quality_default: crate::session::controller_offer(),
                desktop_codec_default: None,
                system_operations: HashMap::new(),
                next_operation_id: 1,
                privacy: None,
                terminal_pending: HashMap::new(),
                hosting: false,
                trust,
                replay,
                record_relay,
                incoming: HashMap::new(),
                incoming_slots: Arc::new(AtomicUsize::new(0)),
                outgoing: HashMap::new(),
                clipboard_owner: None,
                desktop_control: None,
                lock_watch: crate::power::LockAfterControl::default(),
                next_session_id: 1,
                services: HashMap::new(),
                hosted_services: HashMap::new(),
                next_service_id: 1,
                transfer_epochs: HashMap::new(),
                generation: 0,
                shut_down: false,
                pairing_window: None,
                control_request: None,
                next_control_request: 1,
            }),
            desktop: Arc::new(Mutex::new(None)),
            desktop_info: Mutex::new(None),
            cleanup_executor,
            desktop_owners: Arc::new(AtomicUsize::new(0)),
            input_cleanup,
            capture: Mutex::new(None),
            clipboard: Mutex::new(None),
            clipboard_file_source: Mutex::new(None),
            paste_waiters: Mutex::new(HashMap::new()),
            paste_preparing: Mutex::new(std::collections::HashSet::new()),
            paste_nonce: AtomicU64::new(1),
            camera_factory: Mutex::new(None),
            camera_open: Mutex::new(std::collections::HashSet::new()),
            audio_factory: Mutex::new(None),
            terminal_factory: Mutex::new(None),
            system_actions: Mutex::new(None),
            privacy_serial: Mutex::new(()),
            terminal_sessions: Mutex::new(HashMap::new()),
            clipboard_available: AtomicBool::new(false),
            video_inbox: Mutex::new(None),
            video_subscriber: Mutex::new(Weak::new()),
            session_gate: tokio::sync::Mutex::new(()),
            desktop_gate: tokio::sync::Mutex::new(()),
            desktop_streams: AtomicUsize::new(0),
            desktop_routers: Mutex::new(std::collections::HashSet::new()),
            topology: Mutex::new(super::topology::TopologySnapshot {
                revision: 0,
                displays: Vec::new(),
            }),
            topology_watch: tokio::sync::watch::channel(0).0,
            swap_control_command: AtomicBool::new(false),
            video_caps: Mutex::new(crate::video_caps::VideoCodecCaps::default()),
            keyboard_mode: AtomicU8::new(KeyboardMode::Character as u8),
            pointer_defaults: Mutex::new(crate::PointerSettings::default()),
            cursor_receiver: AtomicBool::new(false),
            file_roots: Mutex::new(crate::fs::RootSet::default()),
            blob_dir: Mutex::new(None),
            blob_store: Mutex::new(None),
            blob_open: tokio::sync::Mutex::new(()),
            blob_offers: Arc::new(Mutex::new(std::collections::HashSet::new())),
            blob_recent_fetches: Arc::new(Mutex::new(HashMap::new())),
            blob_import_pins: Arc::new(Mutex::new(std::collections::HashMap::new())),
            blob_job_offers: Mutex::new(std::collections::HashSet::new()),
            blob_serving: Mutex::new(std::collections::HashMap::new()),
            jobs: Arc::new(Mutex::new(std::collections::HashMap::new())),
            job_next: std::sync::atomic::AtomicU64::new(1),
            job_save: Mutex::new(()),
            job_owner_gate: Mutex::new(()),
            job_cancel: Mutex::new(std::collections::HashMap::new()),
            job_driving: Mutex::new(std::collections::HashSet::new()),
            file_resources: Arc::new(Mutex::new(file_resources::Resources::default())),
            backend_state: Mutex::new(std::collections::HashMap::new()),
        });
        let accept = Arc::clone(&shared);
        runtime.spawn(async move { accept_loop(accept).await });
        let power_shared = Arc::downgrade(&shared);
        runtime.spawn(async move {
            power::start(power_shared);
        });
        Ok(Self { runtime, shared })
    }

    pub fn shutdown(&self) {
        // Settle the blob store first so its directory reopens cleanly.
        // A storage fence may already have scheduled the same close.
        self.blob_close_store();
        {
            let mut state = lock(&self.shared.state);
            if !state.shut_down {
                state.shut_down = true;
                stop_owned_work(&mut state, &self.shared);
            }
        }
        self.finish_endpoint_close();
    }

    /// Closes the endpoint once. A storage fence may already have claimed the
    /// close; this waits for that task. A claim released without a finished
    /// close is retried here. The core stays fenced when close does not finish.
    fn finish_endpoint_close(&self) {
        if !claim_endpoint_close(&self.shared) {
            wait_for_endpoint_close(&self.shared);
            if self.shared.teardown_phase.load(Ordering::Acquire) != 0 {
                return;
            }
            if !claim_endpoint_close(&self.shared) {
                wait_for_endpoint_close(&self.shared);
                return;
            }
        }
        let blocked = take_close_block(&self.shared);
        let limit = teardown_limit(&self.shared, blocked);
        let endpoint = self.shared.endpoint.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.runtime.spawn(async move {
            let closed = tokio::time::timeout(limit, async move {
                if blocked {
                    std::future::pending::<()>().await;
                }
                endpoint.close().await;
            })
            .await
            .is_ok();
            let _ = tx.send(closed);
        });
        let observed = rx.recv_timeout(limit.saturating_add(Duration::from_secs(1)));
        match observed {
            Ok(true) => mark_teardown_finished(&self.shared),
            _ => {
                eprintln!("vnilink: installation endpoint close exceeded 5s; restart required");
                release_teardown_claim(&self.shared);
            }
        }
    }

    pub fn endpoint_id(&self) -> String {
        self.shared.secret.public().to_string()
    }

    /// Secret bytes of the identity bound in this process. Do not log them.
    ///
    /// [`Core::open`] already saves this secret in the installation record.
    /// After [`Core::reset_identity`], the new secret is only in the store.
    /// This stopped core still reports the key it bound.
    pub fn export_identity(&self) -> [u8; 32] {
        self.shared.secret.to_bytes()
    }

    /// Saved relay configuration for the next open. Unlike the snapshot, this
    /// includes pending changes. Credentials remain opaque inside CustomRelay:
    /// callers can preserve or replace them but cannot read the token string.
    pub fn relay_configuration(&self) -> Result<RelaySettings, Error> {
        Ok(self.alive_state()?.record_relay.clone())
    }

    /// Compare saved and bound relays inside Rust, including private credentials.
    pub fn relay_restart_required(&self) -> Result<bool, Error> {
        Ok(self.alive_state()?.record_relay != self.shared.relay)
    }

    /// Saves the relay the next [`Core::open`] binds.
    ///
    /// The running endpoint, [`Core::snapshot`], and new invitations keep the
    /// relay bound at start. A credential stays in the installation record and
    /// out of [`Debug`]. This does not dial the relay.
    pub fn set_relay(&self, relay: RelaySettings) -> Result<(), Error> {
        relay.to_relay_mode()?;
        let mut state = self.alive_state()?;
        if self.shared.store.is_none() {
            return Err(Error::InvalidSetting(
                "saving the relay requires an installation store",
            ));
        }
        let previous = state.record_relay.clone();
        state.record_relay = relay;
        if let Err(error) = persist(&self.shared, &mut state) {
            state.record_relay = previous;
            return Err(error);
        }
        Ok(())
    }

    /// Replaces the saved identity and forgets paired devices, grants, and
    /// used invitations. The saved relay setting stays.
    ///
    /// The running endpoint stops. Open the store again for the new endpoint
    /// id. There is no path back to the previous identity or its grants. A
    /// locked or unavailable store leaves this process and the saved record
    /// unchanged.
    pub fn reset_identity(&self) -> Result<(), Error> {
        {
            let mut state = self.alive_state()?;
            let store = self.shared.store.clone().ok_or(Error::InvalidSetting(
                "resetting identity requires an installation store",
            ))?;
            let secret = SecretKey::generate();
            let bytes = installation::encode(
                &secret.to_bytes(),
                &state.record_relay,
                &Trust::default(),
                &ReplayCache::new(),
                &state.terminal_policy,
                &state.file_retention_policy,
                &state.power_policy,
                &state.file_resource_policy,
                &state.system_policy,
            )?;
            save_installation(&self.shared, &mut state, store.as_ref(), &bytes)?;
        }
        self.shutdown();
        Ok(())
    }

    pub fn subscribe(&self) -> Result<EventSubscription, Error> {
        self.ensure_alive()?;
        self.shared.events.subscribe()
    }

    /// Live hosted peers, oldest id first. `Snapshot::incoming` remains the
    /// latest session for single-view callers. Use this when iterating peers.
    pub fn hosted_sessions(&self) -> Result<Vec<SessionInfo>, Error> {
        let state = self.alive_state()?;
        let mut sessions: Vec<SessionInfo> = state
            .incoming
            .values()
            .filter(|session| !finished(&session.phase))
            .map(session_info)
            .collect();
        sessions.sort_by_key(|s| s.id);
        Ok(sessions)
    }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let state = lock(&self.shared.state);
        if state.shut_down {
            return Err(Error::ShutDown);
        }
        Ok(Snapshot {
            endpoint_id: self.endpoint_id(),
            public_relay: self.shared.relay.is_public(),
            relay_urls: self.shared.relay.relay_urls()?,
            hosting: state.hosting,
            devices: state
                .trust
                .devices()
                .map(|device| DeviceInfo {
                    endpoint_id: device.display_id.clone(),
                    hostname: state.trust.hostname_of(&device.endpoint_id),
                    pet_name: state.trust.pet_name_of(&device.endpoint_id),
                    relays: device.relays.clone(),
                    direct: device.direct.clone(),
                    grants: state.trust.sharing.grants(),
                    os: state.trust.os_of(&device.endpoint_id),
                    presence: super::link::presence(&state, &device.display_id),
                })
                .collect(),
            incoming: state.incoming_latest().map(session_info),
            outgoing: state.single_outgoing().map(session_info),
            capabilities: desktop_capabilities(&self.shared),
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        desktop_capabilities(&self.shared)
    }

    /// How many times this core claimed endpoint close. A storage fence and
    /// a later `shutdown` share one claim when that close finishes. A claim
    /// released without a finished close can be claimed again.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn endpoint_close_count(&self) -> usize {
        self.shared.close_count.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn endpoint_is_closed(&self) -> bool {
        self.shared.endpoint.is_closed()
    }

    #[cfg(test)]
    pub(crate) fn teardown_phase(&self) -> u8 {
        self.shared.teardown_phase.load(Ordering::Acquire)
    }

    /// Shortens only a blocked blob shutdown or endpoint close. A real close
    /// keeps the production budget.
    #[cfg(test)]
    pub(crate) fn arm_blocked_teardown(
        &self,
        budget: Duration,
        block_blob: bool,
        block_close: bool,
    ) {
        let millis = u64::try_from(budget.as_millis()).unwrap_or(0);
        self.shared
            .teardown_budget_ms
            .store(millis, Ordering::Release);
        self.shared
            .block_blob_shutdown
            .store(block_blob, Ordering::Release);
        self.shared
            .block_endpoint_close
            .store(block_close, Ordering::Release);
    }
}
