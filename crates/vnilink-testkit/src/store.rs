//! Installations a test holds: an engine opened on a store the test keeps,
//! a store whose failures the test chooses, and an engine with no store at
//! all. A kit [`crate::Engine`] keeps its store to itself and shares its
//! disk; these do neither.

use std::sync::{Arc, Mutex};

use vnilink::{
    Core, CoreConfig, CustomRelay, Error, InstallationStore, Listen, MemoryStore, RelaySettings,
};

use crate::relay::LocalRelay;

/// An engine whose installation store the test holds, to make its next save
/// fail or to reopen it with another relay: in a [`MemoryStore`] unless the
/// test brings a [`TestStore`]. It is set up as [`open_in`] sets one up,
/// sharing no disk.
pub struct Installed<S: InstallationStore + Clone + 'static = MemoryStore> {
    pub core: Core,
    pub store: S,
    name: String,
}

impl Installed {
    /// A new installation named `name`, on the public relays it never
    /// reaches.
    pub fn start(name: &str) -> Self {
        Self::start_with(name, RelaySettings::public_n0())
    }

    /// A new installation named `name` whose first relay setting is `relay`.
    pub fn start_with(name: &str, relay: RelaySettings) -> Self {
        Self::open_in(name, MemoryStore::empty(), relay).expect("a loopback engine")
    }
}

impl Installed<TestStore> {
    /// A new installation named `name` in a [`TestStore`], on the public
    /// relays it never reaches.
    pub fn faulty(name: &str) -> Self {
        Self::open_in(name, TestStore::default(), RelaySettings::public_n0())
            .expect("a loopback engine")
    }
}

impl<S: InstallationStore + Clone + 'static> Installed<S> {
    /// The installation `store` holds, or a new one saved there, named
    /// `name`, whose first relay setting is `relay`; the store's refusal
    /// when it refuses.
    pub fn open_in(name: &str, store: S, relay: RelaySettings) -> Result<Self, Error> {
        let core = open_in(name, Arc::new(store.clone()), relay)?;
        Ok(Self {
            core,
            store,
            name: name.to_owned(),
        })
    }

    /// Shuts this engine down and opens its installation again, as quitting
    /// and starting the application does.
    pub fn restart(&mut self) {
        self.restart_with(RelaySettings::public_n0());
    }

    /// As [`Self::restart`], offering `relay` as the first relay setting,
    /// which a saved one overrides.
    pub fn restart_with(&mut self, relay: RelaySettings) {
        self.core.shutdown();
        self.core =
            open_in(&self.name, Arc::new(self.store.clone()), relay).expect("a loopback engine");
    }
}

impl<S: InstallationStore + Clone + 'static> Drop for Installed<S> {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

/// Opens the installation in `store` as the kit opens an engine: on
/// loopback, named `name` in records and to its peers, with codecs; the
/// store's refusal when it refuses.
pub fn open_in(
    name: &str,
    store: Arc<dyn InstallationStore>,
    relay: RelaySettings,
) -> Result<Core, Error> {
    vnilink::trace::name_next_engine(name);
    let core = Core::open_direct(store, relay)?;
    core.set_trace_name(name);
    core.set_hostname(name).expect("a computer name");
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "testkit"))
        .expect("codecs");
    Ok(core)
}

/// An engine with no installation store, as [`Core::start`] starts one: of
/// `identity`, or a new one, publishing at most `event_capacity` events
/// ahead of a reader. An engine without a store cannot be kept off relays
/// as [`Core::open_direct`] keeps one, so its relay is `relay`, on this
/// computer.
pub fn start_storeless(
    name: &str,
    relay: &LocalRelay,
    identity: Option<[u8; 32]>,
    event_capacity: usize,
) -> Result<Core, Error> {
    vnilink::trace::name_next_engine(name);
    let core = Core::start(CoreConfig {
        relay: RelaySettings::custom(vec![CustomRelay::new(relay.url())?])?,
        listen: Listen::Loopback,
        event_capacity,
        identity,
    })?;
    core.set_trace_name(name);
    Ok(core)
}

/// An installation store that counts every save asked of it and fails its
/// loads, or its next save, or panics in it, as the test chooses: the
/// platform stores' failures, which a [`MemoryStore`] offers only in part.
#[derive(Clone, Default)]
pub struct TestStore(Arc<Mutex<Held>>);

#[derive(Default)]
struct Held {
    record: Option<Vec<u8>>,
    refusing: Option<Fault>,
    next_save: Option<Fault>,
    panic_next_save: bool,
    saves: usize,
}

/// A store's failure, as a platform store reports one.
#[derive(Clone, Copy)]
pub enum Fault {
    Locked,
    Unavailable,
    RecoveryRequired,
}

impl Fault {
    fn error(self) -> Error {
        match self {
            Fault::Locked => Error::StoreLocked,
            Fault::Unavailable => Error::StoreUnavailable,
            Fault::RecoveryRequired => Error::StoreRecoveryRequired,
        }
    }
}

impl TestStore {
    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.0.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    /// A store already holding `record`.
    pub fn containing(record: Vec<u8>) -> Self {
        let store = Self::default();
        store.held().record = Some(record);
        store
    }

    /// A store whose every load fails with `fault`.
    pub fn refusing(fault: Fault) -> Self {
        let store = Self::default();
        store.held().refusing = Some(fault);
        store
    }

    /// The next save fails with `fault`, once, leaving the record as it
    /// was.
    pub fn fail_next_save(&self, fault: Fault) {
        self.held().next_save = Some(fault);
    }

    /// The next save panics, once, leaving the record as it was.
    pub fn panic_next_save(&self) {
        self.held().panic_next_save = true;
    }

    /// The record it holds.
    pub fn record(&self) -> Option<Vec<u8>> {
        self.held().record.clone()
    }

    /// The saves asked of it, failed ones included.
    pub fn save_attempts(&self) -> usize {
        self.held().saves
    }
}

impl InstallationStore for TestStore {
    fn load(&self) -> Result<Option<Vec<u8>>, Error> {
        let held = self.held();
        match held.refusing {
            Some(fault) => Err(fault.error()),
            None => Ok(held.record.clone()),
        }
    }

    fn save(&self, record: &[u8]) -> Result<(), Error> {
        let mut held = self.held();
        held.saves += 1;
        if std::mem::take(&mut held.panic_next_save) {
            drop(held);
            panic!("the store panics while saving, as the test asked");
        }
        if let Some(fault) = held.next_save.take() {
            return Err(fault.error());
        }
        held.record = Some(record.to_vec());
        Ok(())
    }
}
