// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

#[cfg(target_os = "macos")]
#[path = "tests/capture.rs"]
mod capture_tests;
#[cfg(target_os = "macos")]
#[path = "tests/clock.rs"]
mod clock_tests;
#[path = "tests/encode.rs"]
mod encode_tests;
#[path = "tests/host.rs"]
mod host_tests;
#[path = "tests/pixels.rs"]
mod pixels_tests;
#[cfg(target_os = "macos")]
#[path = "tests/system.rs"]
mod system_tests;
#[cfg(target_os = "macos")]
#[path = "tests/videotoolbox.rs"]
mod videotoolbox_tests;
