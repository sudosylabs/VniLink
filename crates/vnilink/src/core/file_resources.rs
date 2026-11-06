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
/// Free bytes on a volume, and that volume's size.
pub(super) struct Space {
    pub free: u64,
    pub volume: u64,
}

/// The free reserve for a volume: 1% of its size, and never more than `cap`.
pub(super) fn free_floor(volume: u64, cap: u64) -> u64 {
    volume.saturating_div(100).min(cap)
}

impl Resources {
    fn admit(
        &mut self,
        id: u64,
        bytes: u64,
        space: Space,
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
        let floor = free_floor(space.volume, policy.minimum_free_bytes);
        let Some(needed) = total.checked_add(floor) else {
            return Err(Error::InvalidSetting("file resource accounting overflow"));
        };
        if needed > space.free {
            return Err(Error::InsufficientStorage {
                needed,
                free: space.free,
            });
        }
        self.reservations.insert(id, bytes);
        Ok(true)
    }
}
pub(super) fn reserve(
    shared: &Shared,
    id: u64,
    size: u64,
    space: Space,
) -> Result<Option<Reservation>, Error> {
    let policy = lock(&shared.state).file_resource_policy;
    // A file occupies its size once per volume: the partial on the transfer
    // volume, and a destination sibling only when that is another volume, since
    // on one volume publishing renames the partial.
    if !lock(&shared.file_resources).admit(id, size, space, policy)? {
        return Ok(None);
    }
    Ok(Some(Reservation {
        resources: shared.file_resources.clone(),
        id,
    }))
}
/// A volume's space from `statvfs`'s block counts and fragment size.
#[cfg(unix)]
fn space_from(free_blocks: u64, blocks: u64, fragment: u64) -> Space {
    Space {
        free: free_blocks.saturating_mul(fragment),
        volume: blocks.saturating_mul(fragment),
    }
}
#[cfg(unix)]
pub(super) fn available(path: &Path) -> Result<Space, Error> {
    let stats = nix::sys::statvfs::statvfs(path)
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))?;
    Ok(space_from(
        stats.blocks_available() as u64,
        stats.blocks() as u64,
        stats.fragment_size() as u64,
    ))
}
#[cfg(unix)]
pub(super) fn available_fd(fd: impl std::os::fd::AsFd) -> Result<Space, Error> {
    let stats = nix::sys::statvfs::fstatvfs(fd)
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))?;
    Ok(space_from(
        stats.blocks_available() as u64,
        stats.blocks() as u64,
        stats.fragment_size() as u64,
    ))
}
#[cfg(windows)]
pub(super) fn available(path: &Path) -> Result<Space, Error> {
    vnilink_rename::windows::disk_space(path)
        .map(|space| Space {
            free: space.available,
            volume: space.volume,
        })
        .map_err(|_| Error::InvalidSetting("available storage could not be checked"))
}
#[cfg(not(any(unix, windows)))]
pub(super) fn available(_: &Path) -> Result<Space, Error> {
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
    /// A volume large enough that 1% is at least the policy cap, so the
    /// reserve in these tests is the cap.
    fn room(free: u64) -> Space {
        Space {
            free,
            volume: 10_000,
        }
    }
    #[test]
    fn concurrent_reservations_enforce_exact_count_bytes_and_headroom() {
        let mut resources = Resources::default();
        assert!(resources.admit(1, 40, room(110), policy()).unwrap());
        assert!(!resources.admit(1, 40, room(110), policy()).unwrap());
        assert!(resources.admit(2, 60, room(110), policy()).unwrap());
        assert!(!resources.admit(3, 0, room(110), policy()).unwrap());
        resources.reservations.remove(&1);
        assert!(!resources.admit(3, 41, room(110), policy()).unwrap());
        assert!(resources.admit(3, 40, room(110), policy()).unwrap());
        resources.reservations.clear();
        assert!(resources.admit(4, 100, room(109), policy()).is_err());
        assert!(resources.admit(4, 101, room(u64::MAX), policy()).is_err());
        assert!(resources.reservations.is_empty());
    }
    #[test]
    fn a_file_on_a_small_volume_uses_one_percent_and_says_what_it_needs() {
        let volume = 8 * 1024 * 1024 * 1024;
        let cap = 256 * 1024 * 1024;
        assert_eq!(free_floor(volume, cap), volume / 100);
        let file = 100 * 1024 * 1024;
        let free = 200 * 1024 * 1024;
        let mut resources = Resources::default();
        let policy = FileResourcePolicy {
            max_concurrent_receivers: 1,
            max_reserved_bytes: 64 * 1024 * 1024 * 1024,
            minimum_free_bytes: cap,
        };
        let admitted = resources
            .admit(1, file, Space { free, volume }, policy)
            .unwrap();
        assert!(admitted, "100 MiB fits an 8 GiB volume with 200 MiB free");
        resources.reservations.clear();
        let tight = free_floor(volume, cap) - 1;
        let error = resources
            .admit(
                2,
                file,
                Space {
                    free: file + tight,
                    volume,
                },
                policy,
            )
            .unwrap_err();
        let Error::InsufficientStorage {
            needed,
            free: reported,
        } = error
        else {
            panic!("the refusal names the space, not a fixed sentence: {error}");
        };
        assert_eq!(needed, file + free_floor(volume, cap));
        assert_eq!(reported, file + tight);
        let huge = 80 * 1024 * 1024 * 1024;
        let wide = FileResourcePolicy::default();
        assert!(
            resources
                .admit(
                    3,
                    huge,
                    Space {
                        free: huge + wide.minimum_free_bytes,
                        volume: 10 * huge,
                    },
                    wide,
                )
                .unwrap(),
            "a file past the old 64 GiB cap is admitted when the volume has room"
        );
    }
    #[test]
    fn reservation_drop_releases_budget_on_unwind() {
        let resources = Arc::new(Mutex::new(Resources::default()));
        assert!(lock(&resources).admit(1, 100, room(110), policy()).unwrap());
        let captured = resources.clone();
        let _ = std::panic::catch_unwind(move || {
            let _reservation = Reservation {
                resources: captured,
                id: 1,
            };
            panic!("injected worker failure");
        });
        assert!(lock(&resources).admit(2, 100, room(110), policy()).unwrap());
    }
}
