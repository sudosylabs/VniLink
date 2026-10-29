//! What a test needs of the computer it runs on: hardware, a device or a
//! tool that a shared CI computer may lack.
//!
//! A test that cannot run here says so on one line, `skipped: <test> needs
//! <what>: <why>`, and returns; it never passes in silence. A computer that
//! must run them all, as one with the hardware they were written for, sets
//! `VNILINK_REQUIRE=all` (or a comma-separated list of the `what`s it has),
//! and a missing one fails the test instead.

use std::io::Write;

/// Whether a test that needs `what` runs: `found` is `Err` with why it is
/// missing on this computer. Missing, it prints why and returns `false`, or
/// panics where `VNILINK_REQUIRE` names `what`.
#[track_caller]
pub fn needs(what: &str, found: Result<(), String>) -> bool {
    let Err(why) = found else {
        return true;
    };
    let test = std::thread::current()
        .name()
        .unwrap_or("this test")
        .to_owned();
    if required(what) {
        panic!("{test} needs {what}, which VNILINK_REQUIRE says this computer has: {why}");
    }
    // Written past the test harness, which keeps a passing test's output.
    let _ = writeln!(
        std::io::stderr().lock(),
        "skipped: {test} needs {what}: {why}"
    );
    false
}

/// `Ok` when `program` runs here (`program --version` or the given
/// arguments exit), for [`needs`].
pub fn tool(program: &str, args: &[&str]) -> Result<(), String> {
    match std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(_) => Ok(()),
        Err(error) => Err(format!("{program} could not be run ({error})")),
    }
}

fn required(what: &str) -> bool {
    std::env::var("VNILINK_REQUIRE").is_ok_and(|list| {
        list.split(',')
            .map(str::trim)
            .any(|item| item == "all" || item == what)
    })
}
