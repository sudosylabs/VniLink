// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the terminal scenarios share beyond the kit's
//! ([`vnilink_testkit::terminal`]): a shell read to its exit, and the
//! command lines each platform's shell takes for what a scenario types.

use vnilink::{Error, RemoteTerminalService};
use vnilink_testkit::terminal::read_until;
use vnilink_testkit::DEFAULT_WAIT;

/// `terminal`'s output until the shell exits, and its exit code.
pub(super) fn read_to_exit(terminal: &RemoteTerminalService) -> Result<(String, u32), Error> {
    let mut output = Vec::new();
    let exit = read_until(
        terminal,
        &mut output,
        "the shell to exit",
        DEFAULT_WAIT,
        |_| false,
    );
    let text = String::from_utf8_lossy(&output).into_owned();
    let exit = exit.ok_or(Error::InvalidSetting("the shell did not exit"))?;
    Ok((text, exit))
}

/// `unix` on Unix's `sh`, `windows` on Windows' `cmd`: the command line a
/// scenario types, the rest of it the same on both.
pub(super) fn command(unix: &'static str, windows: &'static str) -> &'static str {
    if cfg!(windows) {
        windows
    } else {
        unix
    }
}
