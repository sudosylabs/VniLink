//! Opaque installation record.
//!
//! The bytes hold the installation secret, paired devices, grants, used
//! invitation nonces, and this installation's relay settings. The host stores
//! them. This module does not open a credential store.
use crate::error::Error;

mod record;

/// Where the host keeps one installation record.
///
/// `load` returns [`None`] when nothing has been saved. `save` replaces the
/// previous bytes. Both run on the caller's thread while a change is applied,
/// so they must return without waiting on the network. Do not log the bytes:
/// they contain the installation secret and any relay credential.
pub trait InstallationStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, Error>;
    fn save(&self, record: &[u8]) -> Result<(), Error>;
}

/// Process-local record. Tests use this. It is not a platform credential store,
/// and dropping the last clone drops the bytes. [`Debug`] prints no record.
#[derive(Clone)]
pub struct MemoryStore {
    inner: std::sync::Arc<std::sync::Mutex<MemoryInner>>,
}

struct MemoryInner {
    record: Option<Vec<u8>>,
    load_fault: Option<StoreFault>,
    save_fault: Option<StoreFault>,
    save_attempts: usize,
    #[cfg(test)]
    panic_on_save: bool,
}

#[derive(Clone, Copy)]
enum StoreFault {
    Locked,
    Unavailable,
    RecoveryRequired,
}

impl From<StoreFault> for Error {
    fn from(fault: StoreFault) -> Self {
        match fault {
            StoreFault::Locked => Error::StoreLocked,
            StoreFault::Unavailable => Error::StoreUnavailable,
            StoreFault::RecoveryRequired => Error::StoreRecoveryRequired,
        }
    }
}

impl std::fmt::Debug for MemoryStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MemoryStore")
    }
}

fn lock(store: &MemoryStore) -> std::sync::MutexGuard<'_, MemoryInner> {
    store
        .inner
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

impl MemoryStore {
    pub fn empty() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::Mutex::new(MemoryInner {
                record: None,
                load_fault: None,
                save_fault: None,
                save_attempts: 0,
                #[cfg(test)]
                panic_on_save: false,
            })),
        }
    }

    pub(crate) fn containing(bytes: Vec<u8>) -> Self {
        let store = Self::empty();
        lock(&store).record = Some(bytes);
        store
    }

    pub(crate) fn locked() -> Self {
        let store = Self::empty();
        lock(&store).load_fault = Some(StoreFault::Locked);
        store
    }

    pub(crate) fn unavailable() -> Self {
        let store = Self::empty();
        lock(&store).load_fault = Some(StoreFault::Unavailable);
        store
    }

    /// The next [`InstallationStore::save`] returns `error` once.
    #[cfg(any(test, feature = "test-probes"))]
    pub fn fail_next_save(&self, error: Error) {
        let fault = match error {
            Error::StoreLocked => StoreFault::Locked,
            Error::StoreUnavailable => StoreFault::Unavailable,
            Error::StoreRecoveryRequired => StoreFault::RecoveryRequired,
            _ => panic!("the injected save fault must be a store error"),
        };
        lock(self).save_fault = Some(fault);
    }

    /// The next save panics once, before it replaces the record.
    #[cfg(test)]
    pub(crate) fn panic_next_save(&self) {
        lock(self).panic_on_save = true;
    }

    pub(crate) fn record(&self) -> Option<Vec<u8>> {
        lock(self).record.clone()
    }

    /// Saves attempted since this store was created, including injected failures.
    #[cfg(test)]
    pub(crate) fn save_attempts(&self) -> usize {
        lock(self).save_attempts
    }
}

impl InstallationStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, Error> {
        let inner = lock(self);
        if let Some(fault) = inner.load_fault {
            return Err(fault.into());
        }
        Ok(inner.record.clone())
    }

    fn save(&self, record: &[u8]) -> Result<(), Error> {
        let mut inner = lock(self);
        inner.save_attempts = inner.save_attempts.saturating_add(1);
        #[cfg(test)]
        if inner.panic_on_save {
            inner.panic_on_save = false;
            panic!("installation store test panic");
        }
        if let Some(fault) = inner.save_fault.take() {
            return Err(fault.into());
        }
        inner.record = Some(record.to_vec());
        Ok(())
    }
}

pub(crate) use record::{decode, encode};

#[cfg(test)]
pub(crate) use record::checksum;
