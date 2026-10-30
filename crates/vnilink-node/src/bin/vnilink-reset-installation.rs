// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Forgets the local installation after preserving its database files.
//! Refuses to run unless `--confirm-forget-installation` is present.

fn main() {
    std::process::exit(vnilink_node::reset_installation_cli());
}
