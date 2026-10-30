// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A terminal's shell starts from a process that owns its descriptors.
//!
//! The engine runs in Electron's utility process. On Linux, Chromium
//! replaces `close(2)` in its processes with one that aborts when a
//! descriptor it owns is closed ("Crashing due to FD ownership violation"),
//! and the forked child that becomes the shell inherits it. This binary
//! replaces `close` the same way, which is why it is a test binary of its
//! own: the replacement holds for the whole process.
#![cfg(target_os = "linux")]

use std::io::Read;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// The descriptor this process owns, as Chromium owns its own.
static OWNED: AtomicI32 = AtomicI32::new(-1);

/// Chromium's `close`, in miniature: closing the owned descriptor aborts.
#[no_mangle]
pub extern "C" fn close(fd: libc::c_int) -> libc::c_int {
    if fd == OWNED.load(Ordering::Relaxed) {
        let message = b"Crashing due to FD ownership violation\n";
        // SAFETY: writes a static buffer to stderr, then aborts; both are
        // async-signal-safe, as a forked child requires.
        unsafe {
            libc::write(2, message.as_ptr().cast(), message.len());
            libc::abort();
        }
    }
    // SAFETY: the raw system call, which this replacement stands in for.
    unsafe { libc::syscall(libc::SYS_close, fd) as libc::c_int }
}

#[test]
fn a_shell_starts_in_a_process_that_owns_a_descriptor() {
    // Inherited across `fork` and `exec`, as Chromium's descriptors are.
    // SAFETY: opens /dev/null without close-on-exec; the descriptor lives
    // as long as the test.
    let owned = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY) };
    assert!(owned > 2);
    OWNED.store(owned, Ordering::Relaxed);

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", "echo shell-started"]);
    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut buffer = [0; 256];
    while Instant::now() < deadline {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => output.extend_from_slice(&buffer[..read]),
        }
        if output.windows(13).any(|window| window == b"shell-started") {
            break;
        }
    }
    let status = child.wait().unwrap();
    let output = String::from_utf8_lossy(&output);
    assert!(
        output.contains("shell-started"),
        "the shell did not run: {output:?}"
    );
    assert!(status.success(), "{status:?}: {output:?}");
}
