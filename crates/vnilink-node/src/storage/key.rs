// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The 32-byte installation key.
//!
//! Production uses the platform credential store. The encrypted record never
//! goes in that entry. The previous whole-record entry is not read or deleted.

use zeroize::{Zeroize, Zeroizing};

use super::crypto::{self, KEY_LEN};
use super::StoreFailure;

/// Credential service for the installation key. Not the legacy record entry.
pub(crate) const KEY_SERVICE: &str = "com.vnilink.storage";
/// Credential account for the installation key.
pub(crate) const KEY_ACCOUNT: &str = "installation-key";

#[derive(Debug)]
pub(crate) enum KeyAccess {
    Locked,
    Unavailable,
}

pub(crate) trait KeyVault: Send + Sync {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, KeyAccess>;
    fn write(&self, key: &[u8; KEY_LEN]) -> Result<(), KeyAccess>;
    fn delete(&self) -> Result<(), KeyAccess>;
}

pub(crate) struct KeyringVault {
    service: String,
    account: String,
}

impl KeyringVault {
    pub(crate) fn production() -> Self {
        Self {
            service: KEY_SERVICE.to_owned(),
            account: KEY_ACCOUNT.to_owned(),
        }
    }

    #[cfg(test)]
    pub(crate) fn named(service: &str, account: &str) -> Self {
        Self {
            service: service.to_owned(),
            account: account.to_owned(),
        }
    }
}

impl KeyVault for KeyringVault {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, KeyAccess> {
        let entry = keyring::Entry::new(&self.service, &self.account).map_err(map_keyring)?;
        match entry.get_secret() {
            Ok(mut bytes) => {
                let owned = Zeroizing::new(bytes.clone());
                bytes.zeroize();
                Ok(Some(owned))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_keyring(error)),
        }
    }

    fn write(&self, key: &[u8; KEY_LEN]) -> Result<(), KeyAccess> {
        let entry = keyring::Entry::new(&self.service, &self.account).map_err(map_keyring)?;
        entry.set_secret(key).map_err(map_keyring)?;
        let mut read_back = entry.get_secret().map_err(map_keyring)?;
        let matches = crypto::constant_time_eq(key, &read_back);
        read_back.zeroize();
        if !matches {
            return Err(KeyAccess::Unavailable);
        }
        Ok(())
    }

    fn delete(&self) -> Result<(), KeyAccess> {
        let entry = keyring::Entry::new(&self.service, &self.account).map_err(map_keyring)?;
        match entry.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring(error)),
        }
    }
}

fn map_keyring(error: keyring::Error) -> KeyAccess {
    match error {
        keyring::Error::NoStorageAccess(_) => KeyAccess::Locked,
        _ => KeyAccess::Unavailable,
    }
}

pub(crate) fn require_key(
    bytes: Zeroizing<Vec<u8>>,
) -> Result<Zeroizing<[u8; KEY_LEN]>, StoreFailure> {
    if bytes.len() != KEY_LEN {
        return Err(StoreFailure::Corrupt);
    }
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// In-process key. Subprocess tests use [`FileVault`] because this does not
/// cross a process boundary.
#[cfg(test)]
pub(crate) struct MemoryVault {
    inner: std::sync::Mutex<MemoryKey>,
}

#[cfg(test)]
struct MemoryKey {
    bytes: Option<Zeroizing<Vec<u8>>>,
    writes: usize,
}

#[cfg(test)]
impl MemoryVault {
    pub(crate) fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(MemoryKey {
                bytes: None,
                writes: 0,
            }),
        }
    }

    pub(crate) fn writes(&self) -> usize {
        self.inner.lock().expect("memory vault").writes
    }

    pub(crate) fn stored_len(&self) -> Option<usize> {
        self.inner
            .lock()
            .expect("memory vault")
            .bytes
            .as_ref()
            .map(|bytes| bytes.len())
    }
}

#[cfg(test)]
impl KeyVault for MemoryVault {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, KeyAccess> {
        Ok(self.inner.lock().expect("memory vault").bytes.clone())
    }

    fn write(&self, key: &[u8; KEY_LEN]) -> Result<(), KeyAccess> {
        let mut inner = self.inner.lock().expect("memory vault");
        inner.bytes = Some(Zeroizing::new(key.to_vec()));
        inner.writes = inner.writes.saturating_add(1);
        let stored = inner.bytes.as_ref().expect("stored key");
        if !crypto::constant_time_eq(key, stored) {
            return Err(KeyAccess::Unavailable);
        }
        Ok(())
    }

    fn delete(&self) -> Result<(), KeyAccess> {
        self.inner.lock().expect("memory vault").bytes = None;
        Ok(())
    }
}

/// Fixture key file. Tests pass a path outside the installation directory.
#[cfg(test)]
pub(crate) struct FileVault {
    path: std::path::PathBuf,
    writes: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl FileVault {
    pub(crate) fn new(path: std::path::PathBuf) -> Self {
        Self {
            path,
            writes: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(crate) fn writes(&self) -> usize {
        self.writes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
impl KeyVault for FileVault {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, KeyAccess> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(KeyAccess::Unavailable),
        }
    }

    fn write(&self, key: &[u8; KEY_LEN]) -> Result<(), KeyAccess> {
        write_private(&self.path, key)?;
        let mut read_back = std::fs::read(&self.path).map_err(|_| KeyAccess::Unavailable)?;
        let matches = crypto::constant_time_eq(key, &read_back);
        read_back.zeroize();
        if !matches {
            return Err(KeyAccess::Unavailable);
        }
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn delete(&self) -> Result<(), KeyAccess> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(KeyAccess::Unavailable),
        }
    }
}

#[cfg(test)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), KeyAccess> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|_| KeyAccess::Unavailable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| KeyAccess::Unavailable)?;
    }
    file.write_all(bytes).map_err(|_| KeyAccess::Unavailable)?;
    file.sync_all().map_err(|_| KeyAccess::Unavailable)?;
    Ok(())
}
