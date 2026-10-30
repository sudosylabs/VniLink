// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the terminal scenarios share: a terminal of the kit's size opened
//! on a connection, how long its shell has to answer, and a shell read to
//! its exit. Opening with options, reading output and the check that
//! nothing typed was recorded are the kit's
//! ([`vnilink_testkit::terminal`], [`vnilink_testkit::Traced`]).

use std::time::Duration;

use vnilink::terminal::TerminalOptions;
use vnilink::{Error, RemoteTerminalService, SessionInfo, SessionPhase};
use vnilink_testkit::terminal::{read_until, SIZE};
use vnilink_testkit::{wait_for, Engine, DEFAULT_WAIT};

/// How long a shell has to answer what was typed.
pub(super) const SHELL: Duration = Duration::from_secs(10);

/// A terminal of [`SIZE`] on `connection`, its shell ending with it.
pub(super) fn open(
    controller: &Engine,
    connection: &SessionInfo,
) -> Result<RemoteTerminalService, Error> {
    controller.open_terminal(connection, options(false, None))
}

pub(super) fn options(persistent: bool, resume_id: Option<String>) -> TerminalOptions {
    TerminalOptions {
        size: SIZE,
        persistent,
        resume_id,
    }
}

/// `terminal`'s output until the shell exits, and its exit code.
pub(super) fn read_to_exit(terminal: &RemoteTerminalService) -> (String, u32) {
    let mut output = Vec::new();
    let exit = read_until(terminal, &mut output, "the shell to exit", SHELL, |_| false);
    let text = String::from_utf8_lossy(&output).into_owned();
    (text, exit.expect("the shell exited"))
}

/// Waits until `host` keeps no shells.
pub(super) fn host_ended_shells(host: &Engine) {
    wait_for("the host to end its shells", DEFAULT_WAIT, || {
        host.core
            .hosted_terminal_sessions()
            .ok()?
            .is_empty()
            .then_some(())
    });
}

pub(super) fn phase(controller: &Engine, connection: &SessionInfo) -> SessionPhase {
    controller
        .core
        .controller_session(connection.id)
        .expect("the connection")
        .phase
}
