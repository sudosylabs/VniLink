// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Which adapter and output Desktop Duplication opens.
//!
//! An output can be duplicated only through a device on the adapter it is
//! attached to: a device on another GPU gets `DXGI_ERROR_UNSUPPORTED`, as
//! on a hybrid laptop whose panel is on the integrated GPU. DXGI's adapter
//! order does not say which adapter that is. It usually lists the adapter
//! with the primary display first, but a process given the high-performance
//! GPU, or one in session 0 (a service or a remote shell), saw the discrete
//! GPU first on a hybrid Intel + NVIDIA laptop (2026-10-22), and that
//! adapter had no outputs. So every output of every adapter is listed and the one the host
//! captures is chosen by what it shows ([`primary`]).

/// One output of one adapter, as DXGI describes it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Output {
    /// Index of the adapter in DXGI's order.
    pub(crate) adapter: u32,
    /// Index of the output on that adapter.
    pub(crate) output: u32,
    /// The output shows part of the desktop.
    pub(crate) attached: bool,
    /// Windows names this output's monitor the primary one.
    pub(crate) primary: bool,
    /// Where its top-left corner lies on the virtual desktop.
    pub(crate) origin: (i32, i32),
}

/// The output that shows the primary display: the one whose monitor
/// Windows marks primary, else the one at the desktop origin, where the
/// primary display always sits, else the first attached one. Outputs not
/// on the desktop are never chosen. Among equals, DXGI's order decides.
pub(crate) fn primary(outputs: &[Output]) -> Option<Output> {
    let attached = || outputs.iter().filter(|output| output.attached);
    attached()
        .find(|output| output.primary)
        .or_else(|| attached().find(|output| output.origin == (0, 0)))
        .or_else(|| attached().next())
        .copied()
}
