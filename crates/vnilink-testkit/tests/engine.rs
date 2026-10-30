// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! One engine on its own: the settings and local calls an application makes
//! of its engine before, or without, any peer, and what the engine refuses
//! when they name a session, a display or a service it does not have.

#[path = "engine/desktop.rs"]
mod desktop;
#[path = "engine/keyboard.rs"]
mod keyboard;
#[path = "engine/sessions.rs"]
mod sessions;
#[path = "engine/system.rs"]
mod system;
