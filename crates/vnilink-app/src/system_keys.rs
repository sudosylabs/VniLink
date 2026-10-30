// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! This computer's own shortcuts, taken by its platform's keyboard hook while
//! a desktop it controls has the keyboard, and sent to that desktop as the
//! physical keys they are. Linux has no hook yet: there the window keeps
//! what the system lets it see.
// trace: records every ignored failure
use std::sync::{Arc, Weak};

use crate::FfiError;

#[cfg(target_os = "macos")]
type Hook = vnilink_macos::MacSystemKeys;
#[cfg(windows)]
type Hook = vnilink_windows::WindowsSystemKeys;

/// A running hook; dropping it stops taking keys.
pub(crate) struct SystemKeyCapture {
    #[cfg(any(target_os = "macos", windows))]
    _hook: Hook,
}

impl SystemKeyCapture {
    /// Takes this computer's shortcuts for `desktop` until dropped. A key
    /// the desktop no longer accepts is dropped with it.
    pub(crate) fn start(desktop: &Arc<vnilink::RemoteDesktopService>) -> Result<Self, FfiError> {
        let desktop: Weak<vnilink::RemoteDesktopService> = Arc::downgrade(desktop);
        let sink = Box::new(move |usage: u16, down: bool| {
            if let Some(desktop) = desktop.upgrade() {
                vnilink::ignored!(
                    vnilink::trace::INPUT,
                    "a key the desktop refuses is not this hook's to retry",
                    desktop.submit_local_key(vnilink::LocalKey::new(usage, down, ""))
                );
            }
        });
        #[cfg(any(target_os = "macos", windows))]
        {
            Hook::start(sink)
                .map(|hook| Self { _hook: hook })
                .map_err(crate::error::map_error)
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            drop(sink);
            Err(crate::error::map_error(vnilink::Error::Unsupported(
                "system shortcuts on this system",
            )))
        }
    }
}
