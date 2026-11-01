// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The host's clipboard adapter, which each Desktop's clipboard pipe
//! applies a controller's text through.

use super::*;

use crate::clipboard::HostClipboard;
use crate::error::Error;

impl Core {
    /// Installs the adapter that receives clipboard text on this host.
    ///
    /// The library does not read or write the operating-system clipboard.
    /// An idle connection may configure it before a Desktop opens; while a
    /// Desktop streams it is refused as busy.
    pub fn set_clipboard(&self, clipboard: Box<dyn HostClipboard>) -> Result<(), Error> {
        let _engine = self.engine();
        let mut slot = lock(&self.shared.clipboard);
        let _state = self.alive_state()?;
        if self
            .shared
            .desktop_streams
            .load(std::sync::atomic::Ordering::Acquire)
            != 0
        {
            return Err(Error::Busy);
        }
        let _gate = self
            .shared
            .desktop_gate
            .try_lock()
            .map_err(|_| Error::Busy)?;
        *slot = Some(clipboard);
        self.shared
            .clipboard_available
            .store(true, std::sync::atomic::Ordering::Release);
        tracing::info!(target: crate::trace::CLIPBOARD, "clipboard adapter installed");
        Ok(())
    }
}
