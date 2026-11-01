// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One balanced Media Foundation startup. `MFStartup` counts its callers and
//! `MFShutdown` must answer each, or every encoder and decoder opened leaves
//! the platform one reference higher for the life of the process.
// trace: records every ignored failure
use windows::Win32::Media::MediaFoundation::{
    MFShutdown, MFStartup, MFSTARTUP_NOSOCKET, MF_VERSION,
};

/// Media Foundation, started for as long as this value lives. A transform
/// must be released before it, so owners declare it after their transforms.
#[derive(Debug)]
pub(crate) struct MediaFoundation(());

impl MediaFoundation {
    pub(crate) fn start() -> windows::core::Result<Self> {
        // SAFETY: MFStartup has no preconditions beyond COM being usable on the thread.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }?;
        Ok(Self(()))
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: balances the MFStartup that made this value.
        unsafe {
            ignored_hr!(
                vnilink::trace::ENCODE,
                "Media Foundation that cannot shut down ends with the process",
                MFShutdown()
            );
        }
    }
}
