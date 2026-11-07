// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

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
    #[cfg(any(test, feature = "test-probes"))]
    save_fault: Option<StoreFault>,
}

#[cfg(any(test, feature = "test-probes"))]
#[derive(Clone, Copy)]
enum StoreFault {
    Locked,
    Unavailable,
    RecoveryRequired,
}

#[cfg(any(test, feature = "test-probes"))]
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
                #[cfg(any(test, feature = "test-probes"))]
                save_fault: None,
            })),
        }
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

    #[cfg(test)]
    pub(crate) fn record(&self) -> Option<Vec<u8>> {
        lock(self).record.clone()
    }
}

impl InstallationStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, Error> {
        Ok(lock(self).record.clone())
    }

    fn save(&self, record: &[u8]) -> Result<(), Error> {
        let mut inner = lock(self);
        #[cfg(any(test, feature = "test-probes"))]
        if let Some(fault) = inner.save_fault.take() {
            return Err(fault.into());
        }
        inner.record = Some(record.to_vec());
        Ok(())
    }
}

pub(crate) use record::{decode, encode, LocalSettings, Policies};
