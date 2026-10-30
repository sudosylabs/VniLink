// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Receiver admission. Reservations are process-local estimates, not disk quotas.
//! Lock order: read installation policy, release state, then resources. Never
//! hold resources across filesystem operations or job/state/owner locks.
use super::{lock, Shared};
use crate::{job::FileResourcePolicy, Error};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

#[derive(Default)]
pub(super) struct Resources {
    reservations: HashMap<u64, u64>,
}
pub(super) struct Reservation {
    resources: Arc<Mutex<Resources>>,
    id: u64,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        lock(&self.resources).reservations.remove(&self.id);
    }
}
impl Resources {
    fn admit(
        &mut self,
        id: u64,
        bytes: u64,
        available: u64,
        policy: FileResourcePolicy,
    ) -> Result<bool, Error> {
        if self.reservations.contains_key(&id) {
            return Ok(false);
        }
        if self.reservations.len() >= usize::from(policy.max_concurrent_receivers) {
            return Ok(false);
        }
        let reserved = self
            .reservations
            .values()
            .try_fold(0u64, |total, next| total.checked_add(*next))
            .ok_or(Error::InvalidSetting("file resource accounting overflow"))?;
        let total = reserved
            .checked_add(bytes)
            .ok_or(Error::InvalidSetting("file resource accounting overflow"))?;
        if bytes > policy.max_reserved_bytes {
            return Err(Error::InvalidSetting(
                "file exceeds the receiver resource budget",
            ));
        }
        if total > policy.max_reserved_bytes {
            return Ok(false);
        }
        if total
            .checked_add(policy.minimum_free_bytes)
            .is_none_or(|required| required > available)
        {
            return Err(Error::InvalidSetting(
                "insufficient available storage for this file",
            ));
        }
        self.reservations.insert(id, bytes);
        Ok(true)
    }
}
pub(super) fn reserve(
    shared: &Shared,
    id: u64,
    size: u64,
    available: u64,
) -> Result<Option<Reservation>, Error> {
    let policy = lock(&shared.state).file_resource_policy;
    // Cache, export staging, destination sibling may coexist on one volume.
    // Reserve the full worst case even when verified cache ranges already exist.
    let bytes = size
        .checked_mul(3)
        .ok_or(Error::InvalidSetting("file resource accounting overflow"))?;
    if !lock(&shared.file_resources).admit(id, bytes, available, policy)? {
        return Ok(None);
    }
    Ok(Some(Reservation {
        resources: shared.file_resources.clone(),
        id,
    }))
}
#[cfg(unix)]
pub(super) fn available(path: &Path) -> Result<u64, Error> {
    let stats = nix::sys::statvfs::statvfs(path)
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))?;
    Ok((stats.blocks_available() as u64).saturating_mul(stats.fragment_size() as u64))
}
#[cfg(unix)]
pub(super) fn available_fd(fd: impl std::os::fd::AsFd) -> Result<u64, Error> {
    let stats = nix::sys::statvfs::fstatvfs(fd)
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))?;
    Ok((stats.blocks_available() as u64).saturating_mul(stats.fragment_size() as u64))
}
#[cfg(windows)]
pub(super) fn available(path: &Path) -> Result<u64, Error> {
    vnilink_rename::windows::available_storage(path)
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))
}
#[cfg(not(any(unix, windows)))]
pub(super) fn available(_: &Path) -> Result<u64, Error> {
    Err(Error::Unsupported("storage availability"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy() -> FileResourcePolicy {
        FileResourcePolicy {
            max_concurrent_receivers: 2,
            max_reserved_bytes: 100,
            minimum_free_bytes: 10,
        }
    }
    #[test]
    fn concurrent_reservations_enforce_exact_count_bytes_and_headroom() {
        let mut resources = Resources::default();
        assert!(resources.admit(1, 40, 110, policy()).unwrap());
        assert!(!resources.admit(1, 40, 110, policy()).unwrap());
        assert!(resources.admit(2, 60, 110, policy()).unwrap());
        assert!(!resources.admit(3, 0, 110, policy()).unwrap());
        resources.reservations.remove(&1);
        assert!(!resources.admit(3, 41, 110, policy()).unwrap());
        assert!(resources.admit(3, 40, 110, policy()).unwrap());
        resources.reservations.clear();
        assert!(resources.admit(4, 100, 109, policy()).is_err());
        assert!(resources.admit(4, 101, u64::MAX, policy()).is_err());
        assert!(resources.reservations.is_empty());
    }
    #[test]
    fn reservation_drop_releases_budget_on_unwind() {
        let resources = Arc::new(Mutex::new(Resources::default()));
        assert!(lock(&resources).admit(1, 100, 110, policy()).unwrap());
        let captured = resources.clone();
        let _ = std::panic::catch_unwind(move || {
            let _reservation = Reservation {
                resources: captured,
                id: 1,
            };
            panic!("injected worker failure");
        });
        assert!(lock(&resources).admit(2, 100, 110, policy()).unwrap());
    }
}
