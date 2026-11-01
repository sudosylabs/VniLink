// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What two linked engines cost a computer that does nothing: how often
//! their threads wake while a controller and a host hold their presence
//! links and no service runs. The operating system counts each process's
//! wakeups: macOS as interrupt wake ups, Linux as voluntary context switches.

use vnilink_testkit::bench::{Better, Report};

/// Times this process's threads were woken by an interrupt such as a timer.
/// Activity Monitor's package idle wake ups count only while the whole chip
/// is idle, which a computer running a build is not.
#[cfg(target_os = "macos")]
fn wakeups() -> u64 {
    // SAFETY: `proc_pid_rusage` fills the buffer, which has the size of the
    // flavor asked for, and reads nothing else.
    unsafe {
        let mut info: libc::rusage_info_v4 = std::mem::zeroed();
        let status = libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            std::ptr::addr_of_mut!(info).cast(),
        );
        assert_eq!(status, 0, "the process's resource usage");
        info.ri_interrupt_wkups
    }
}

/// Times this process's threads blocked and were woken: the sum of their
/// voluntary context switches.
#[cfg(target_os = "linux")]
fn wakeups() -> u64 {
    std::fs::read_dir("/proc/self/task")
        .expect("the process's threads")
        .filter_map(|task| {
            let status = std::fs::read_to_string(task.ok()?.path().join("status")).ok()?;
            status
                .lines()
                .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))?
                .trim()
                .parse::<u64>()
                .ok()
        })
        .sum()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn main() {
    use std::time::{Duration, Instant};
    use vnilink::DevicePresence;
    use vnilink_testkit::{wait_for, Pair};

    const SETTLE: Duration = Duration::from_secs(5);
    const WINDOW: Duration = Duration::from_secs(20);

    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    host.set_presence_links(true).unwrap();
    controller.set_presence_links(true).unwrap();
    let peer = pair.host.endpoint();
    wait_for("the link", Duration::from_secs(20), || {
        controller
            .snapshot()
            .ok()?
            .devices
            .into_iter()
            .find(|device| device.endpoint_id == peer)
            .filter(|device| device.presence == DevicePresence::Connected)
    });
    std::thread::sleep(SETTLE);
    let (before, started) = (wakeups(), Instant::now());
    std::thread::sleep(WINDOW);
    let per_second = wakeups().saturating_sub(before) as f64 / started.elapsed().as_secs_f64();

    let mut report = Report::new("idle");
    report.record(
        "linked and idle, wakeups per second",
        per_second,
        "1/s",
        Better::Lower,
    );
    drop(pair);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() {
    let _ = (Report::new("idle"), Better::Lower);
    println!(
        "idle wakeups are counted from the process's context switches, which only macOS and Linux report"
    );
}
