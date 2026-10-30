// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Relative paths and single names under a shared root.
//!
//! The rules mirror a transfer manifest's: no absolute paths, no drive
//! prefixes, no backslashes or colons (so a controller library never
//! interprets a Windows path from a Windows host), no NUL, no `.`/`..`
//! components. An empty relative path names the root itself.

use crate::error::Error;

use super::{MAX_COMPONENTS, MAX_COMPONENT_BYTES, MAX_REL_BYTES};

pub(crate) const UNSAFE_PATH: &str = "the path is not a safe relative path";
pub(crate) const UNSAFE_NAME: &str = "the name is not a safe file name";

/// Validates one file or directory basename: a single safe component.
pub(crate) fn check_name(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > MAX_COMPONENT_BYTES
        || name.as_bytes().contains(&0)
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name == "."
        || name == ".."
    {
        return Err(Error::InvalidSetting(UNSAFE_NAME));
    }
    Ok(())
}

/// Prefixes of the names VniLink gives what it is still writing into, or
/// moving out of, a folder: a copy staged beside its destination
/// (`.vnlpub.`), a file arriving in place (`.vnlpart.`), a name changing
/// only its case (`.vnlcase.`) and a folder being deleted (`.vnldelete.`).
const ENGINE_TEMP_PREFIXES: [&str; 4] = [".vnlpub.", ".vnlpart.", ".vnlcase.", ".vnldelete."];

/// Whether `name` is one of VniLink's own temporary entries. They are not
/// the user's: listings, folder fingerprints and watches leave them out,
/// so a pane never shows them or lists its folder again because of them.
pub(crate) fn is_engine_temp(name: &str) -> bool {
    ENGINE_TEMP_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

/// Validates a relative directory path. Empty names the root itself.
pub(crate) fn check_rel(path: &str) -> Result<(), Error> {
    if path.is_empty() {
        return Ok(());
    }
    if path.len() > MAX_REL_BYTES
        || path.as_bytes().contains(&0)
        || path.contains('\\')
        || path.contains(':')
        || path.starts_with('/')
        || path.ends_with('/')
    {
        return Err(Error::InvalidSetting(UNSAFE_PATH));
    }
    let mut count = 0usize;
    for component in path.split('/') {
        count += 1;
        if count > MAX_COMPONENTS
            || component.is_empty()
            || component == "."
            || component == ".."
            || component.len() > MAX_COMPONENT_BYTES
        {
            return Err(Error::InvalidSetting(UNSAFE_PATH));
        }
    }
    Ok(())
}
