//! Open, shutdown, identity, and relay.

use super::*;

use super::host::{accept_loop, bind_endpoint};

use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::{Arc, Mutex, Weak};
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
    /// setting. The host implements [`InstallationStore`] with the platform
    /// credential store. [`crate::MemoryStore`] keeps the bytes only in this
    /// process.
    ///
    /// A locked, unavailable, or invalid store leaves any existing bytes
    /// where they are. The saved relay is the one that is bound, including
    /// the public n0 relays when that is the saved setting.
    pub fn open(store: Arc<dyn InstallationStore>, config: OpenConfig) -> Result<Self, Error> {
        Self::open_with(store, config, false)
    }

    pub(crate) fn open_direct(
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
        let (identity, relay, trust, replay) = match loaded {
            None => (
                None,
                config.initial_relay.clone(),
                Trust::default(),
                ReplayCache::new(),
            ),
            Some(record) => (
                Some(record.secret),
                record.relay,
                record.trust,
                record.replay,
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
                trust,
                replay,
                record_relay: relay,
                store: Some(store),
            }),
        )?;
        if fresh {
            let saved = {
                let state = lock(&core.shared.state);
                persist(&core.shared, &state)
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
        let (trust, replay, record_relay, store) = match installed {
            Some(installed) => (
                installed.trust,
                installed.replay,
                installed.record_relay,
                installed.store,
            ),
            None => (
                Trust::default(),
                ReplayCache::new(),
                bound_relay.clone(),
                None,
            ),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("vnilink")
            .build()
            .map_err(Error::network)?;
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
        let shared = Arc::new(Shared {
            endpoint,
            secret,
            relay: bound_relay,
            store,
            events: EventHub::new(event_capacity),
            state: Mutex::new(State {
                hosting: false,
                trust,
                replay,
                record_relay,
                incoming: None,
                outgoing: HashMap::new(),
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
            desktop: Mutex::new(None),
            capture: Mutex::new(None),
            clipboard: Mutex::new(None),
            clipboard_available: AtomicBool::new(false),
            transfer_dir: Mutex::new(None),
            video_inbox: Mutex::new(None),
            video_subscriber: Mutex::new(Weak::new()),
            session_gate: tokio::sync::Mutex::new(()),
            desktop_gate: tokio::sync::Mutex::new(()),
            swap_control_command: AtomicBool::new(false),
            h264_encode: AtomicBool::new(false),
            h264_decode: AtomicBool::new(false),
            keyboard_mode: AtomicU8::new(KeyboardMode::Character as u8),
            h264_high_encode: AtomicBool::new(false),
            cursor_receiver: AtomicBool::new(false),
            cursor_outbox: tokio::sync::watch::channel(None).0,
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
            job_cancel: Mutex::new(std::collections::HashMap::new()),
            job_driving: Mutex::new(std::collections::HashSet::new()),
        });
        let accept = Arc::clone(&shared);
        runtime.spawn(async move { accept_loop(accept).await });
        Ok(Self { runtime, shared })
    }

    pub fn shutdown(&self) {
        // Settle the blob store first so its directory reopens cleanly.
        self.blob_close_store();
        {
            let mut state = lock(&self.shared.state);
            if state.shut_down {
                return;
            }
            state.shut_down = true;
            state.pairing_window = None;
            state.control_request = None;
            signal_stop(state.incoming.as_mut());
            for session in state.outgoing.values_mut() {
                signal_stop(Some(session));
            }
            super::services::synchronize(&mut state, &self.shared.events);
        }
        let endpoint = self.shared.endpoint.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.runtime.spawn(async move {
            endpoint.close().await;
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(Duration::from_secs(5));
    }

    pub fn endpoint_id(&self) -> String {
        self.shared.secret.public().to_string()
    }

    /// Secret bytes for the platform credential store. Do not log them.
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
        if let Err(error) = persist(&self.shared, &state) {
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
            let state = self.alive_state()?;
            let store = self.shared.store.as_ref().ok_or(Error::InvalidSetting(
                "resetting identity requires an installation store",
            ))?;
            let secret = SecretKey::generate();
            let bytes = installation::encode(
                &secret.to_bytes(),
                &state.record_relay,
                &Trust::default(),
                &ReplayCache::new(),
            )?;
            store.save(&bytes)?;
        }
        self.shutdown();
        Ok(())
    }

    pub fn subscribe(&self) -> Result<EventSubscription, Error> {
        self.ensure_alive()?;
        self.shared.events.subscribe()
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
                    grants: device.grants,
                })
                .collect(),
            incoming: state.incoming.as_ref().map(session_info),
            outgoing: state.single_outgoing().map(session_info),
            capabilities: desktop_capabilities(&self.shared),
        })
    }

    pub fn capabilities(&self) -> Capabilities {
        desktop_capabilities(&self.shared)
    }
}
