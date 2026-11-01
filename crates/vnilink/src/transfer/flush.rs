// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! How finished files reach the disk. A file is synced as soon as its last
//! segment checks; a job is reported complete only after its volume is
//! flushed once. Where a file sync already empties the drive's cache (Linux
//! `fsync`, Windows `FlushFileBuffers`) the volume flush has nothing to add;
//! on macOS `sync_data` empties it with `F_FULLFSYNC`, which costs several
//! milliseconds per file, so a file takes a plain `fsync` there and the job's
//! one `F_FULLFSYNC` makes every file of it durable together.

use std::{fs::File, io, path::Path};

/// Syncs a finished file's data to the drive, or to its cache on macOS
/// where [`flush_volume`] follows.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        nix::unistd::fsync(file.as_raw_fd()).map_err(io::Error::from)
    }
    #[cfg(not(target_os = "macos"))]
    file.sync_data()
}

/// Empties the cache of the drive holding `folder`, so every file synced
/// with [`sync_file`] before is durable.
pub(crate) fn flush_volume(folder: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let folder = File::open(folder)?;
        nix::fcntl::fcntl(folder.as_raw_fd(), nix::fcntl::FcntlArg::F_FULLFSYNC)
            .map(drop)
            .map_err(io::Error::from)?;
    }
    #[cfg(not(target_os = "macos"))]
    let _ = folder;
    Ok(())
}
