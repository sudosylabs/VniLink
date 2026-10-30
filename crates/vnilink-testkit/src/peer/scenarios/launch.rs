// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `latency-host` and `latency-viewer`: one side of the latency
//! measurement, started from the agent's own checkout as the measurement
//! starts it on one computer, through `node build/run.ts testkit:launch`.
//! The checkout is the one the agent was built from, the program is its
//! development Electron with `out/latency.cjs`, and
//! the suite chooses neither: it may set only the variables in
//! [`LATENCY_VARIABLES`], to whole numbers in their ranges.
//!
//! The side's rendezvous files travel as messages ([`Folders`]). A side
//! takes only the files the other role publishes, each checked for what
//! it may hold ([`accepted`]): `read.json` is `{"seconds": 1..=600}` and
//! nothing else, so the suite cannot name a program for the side to run,
//! and a latency host takes only the viewer the suite declared when it
//! started the scenario, which it shares its screen with to be seen, never
//! controlled (`test/latency/main.ts`).
//!
//! The side runs in a process group of its own; stopping the scenario, or
//! dropping it, ends that group and every process it started, waiting a
//! bounded time for them to quit by themselves first.

use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{json, Value};

use super::Scenario;
use crate::peer::folders::Folders;
#[cfg(unix)]
use crate::peer::processes::reap;
#[cfg(unix)]
use crate::peer::processes::{descendants, same, Started};
use crate::peer::Polled;
use crate::Engine;

/// The variables a suite may give a latency side, each a whole number in
/// its range: the viewer's frame rate (0 keeps its default), its bitrate in
/// kb/s (0 keeps the preset's), a busy stimulus (0 or 1) and how long a
/// host waits for its user's consent, in ms. Hold mode reads one screen and
/// never runs across two computers, and that a side runs across two
/// computers is the agent's to say, never the suite's
/// (`VNILINK_LATENCY_ACROSS`).
pub(crate) const LATENCY_VARIABLES: [(&str, std::ops::RangeInclusive<u64>); 4] = [
    ("VNILINK_LATENCY_FPS", 0..=240),
    ("VNILINK_LATENCY_BITRATE", 0..=50_000),
    ("VNILINK_LATENCY_BUSY", 0..=1),
    ("VNILINK_LATENCY_CONSENT_MS", 0..=600_000),
];

/// How long a stopped side may take to quit by itself: the launcher waits
/// up to 20 s for its Electron to see `done`.
const QUIT_WAIT: Duration = Duration::from_secs(30);
/// How long an ended side may take to be gone.
const END_WAIT: Duration = Duration::from_secs(5);

/// Which side the scenario starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    LatencyHost,
    LatencyViewer,
}

impl Side {
    /// The scenario's name, which `testkit:launch` takes too.
    fn name(self) -> &'static str {
        match self {
            Self::LatencyHost => "latency-host",
            Self::LatencyViewer => "latency-viewer",
        }
    }

    /// The side's role in the latency measurement.
    fn role(self) -> &'static str {
        match self {
            Self::LatencyHost => "host",
            Self::LatencyViewer => "viewer",
        }
    }
}

pub(super) struct Launched {
    side: Side,
    folders: Option<Folders>,
    output: Option<PathBuf>,
    process: Option<SideProcess>,
    /// The latency host's viewer: the key the suite declared at start.
    viewer_key: Option<String>,
}

impl Launched {
    pub(super) fn new(side: Side) -> Self {
        Self {
            side,
            folders: None,
            output: None,
            process: None,
            viewer_key: None,
        }
    }

    /// Ends the side and removes its folders.
    fn teardown(&mut self) {
        if let Some(folders) = self.folders.as_mut() {
            // The sides quit when they see `done`, as on one computer. The
            // suite may have given it already.
            let _ = folders.put("done.json", b"{}");
        }
        if let Some(process) = self.process.take() {
            process.end(self.side.name());
        }
        self.output = None;
        self.folders = None;
    }
}

impl Drop for Launched {
    fn drop(&mut self) {
        self.teardown();
    }
}

/// The checkout this agent was built from: its build tasks, development
/// Electron and `out/` are what a side runs.
fn checkout() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// The variables the suite asked for in `params["env"]`, each one of
/// [`LATENCY_VARIABLES`] set to a whole number.
pub(crate) fn requested_variables(params: &Value) -> Result<Vec<(String, String)>, String> {
    let Some(requested) = params.get("env").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let requested = requested
        .as_object()
        .ok_or("env names variables and their values")?;
    requested
        .iter()
        .map(|(name, value)| {
            let Some((_, range)) = LATENCY_VARIABLES
                .iter()
                .find(|(variable, _)| variable == name)
            else {
                return Err(format!(
                    "{name} is not a variable a suite may set; it may set {}",
                    LATENCY_VARIABLES.map(|(variable, _)| variable).join(", ")
                ));
            };
            let value = value.as_str().unwrap_or_default();
            let number = (!value.is_empty()
                && value.len() <= 9
                && value.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| value.parse::<u64>().ok())
            .flatten();
            if !number.is_some_and(|number| range.contains(&number)) {
                return Err(format!(
                    "{name} takes a whole number from {} to {}",
                    range.start(),
                    range.end()
                ));
            }
            Ok((name.clone(), value.to_owned()))
        })
        .collect()
}

/// The key an invitation `token` (`vnl1.` and hex) was made by: its
/// first 32 bytes, as 64 lowercase hex digits.
pub(crate) fn invitation_key(token: &str) -> Option<String> {
    let hex = token.strip_prefix("vnl1.")?.get(..64)?;
    hex.bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then(|| hex.to_ascii_lowercase())
}

/// Whether the `side` takes the rendezvous file `name` holding `bytes`
/// from the other side: only the files that role publishes, each checked
/// for what it may hold. `viewer_key` is the viewer a latency host was
/// started for.
pub(crate) fn accepted(
    side: Side,
    name: &str,
    bytes: &[u8],
    viewer_key: Option<&str>,
) -> Result<(), String> {
    let refused = |why: &str| Err(format!("the {} side takes no {name}: {why}", side.name()));
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(Value::Object(fields)) => Value::Object(fields),
        _ => return refused("it is not a JSON object"),
    };
    let fields = value.as_object().map(|fields| {
        let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
        keys.sort_unstable();
        keys
    });
    let keys = fields.unwrap_or_default();
    let text = |field: &str, most: usize| {
        value[field]
            .as_str()
            .is_some_and(|text| !text.is_empty() && text.len() <= most)
    };
    match (side, name) {
        // From the suite's runner, to both sides.
        (Side::LatencyHost | Side::LatencyViewer, "read.json") => {
            let seconds = value["seconds"].as_u64();
            if keys != ["seconds"] || !seconds.is_some_and(|seconds| (1..=600).contains(&seconds)) {
                return refused("it holds only {\"seconds\": 1..600}");
            }
        }
        (Side::LatencyHost | Side::LatencyViewer, "done.json") => {
            if !keys.is_empty() {
                return refused("it is {}");
            }
        }
        // What a viewer publishes, to the host.
        (Side::LatencyHost, "viewer.json") => {
            if keys != ["token"] || !text("token", 16_384) {
                return refused("it holds only the viewer's invitation");
            }
            let key = value["token"].as_str().and_then(invitation_key);
            if key.is_none() || key.as_deref() != viewer_key {
                return refused(
                    "its invitation is not the viewer's the suite started this host for",
                );
            }
        }
        (Side::LatencyHost, "viewer-streaming.json") => {
            if !keys.is_empty() {
                return refused("it is {}");
            }
        }
        // What a host publishes, to the viewer.
        (Side::LatencyViewer, "host.json") => {
            if keys != ["endpoint", "token"] || !text("token", 16_384) || !text("endpoint", 256) {
                return refused("it holds only the host's invitation and endpoint");
            }
        }
        (Side::LatencyViewer, "host-ready.json") => {
            if !keys.is_empty() {
                return refused("it is {}");
            }
        }
        (Side::LatencyViewer, "read-done.json") => {}
        _ => return refused("the other side does not publish it"),
    }
    Ok(())
}

/// Where a latency host keeps the desktop portal's grant between runs, as
/// on one computer: the checkout's `.build/latency`, readable by this user
/// only.
fn screen_grant(checkout: &Path) -> Result<PathBuf, String> {
    let folder = checkout.join(".build/latency");
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("the grant folder could not be made: {}", error.kind()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).map_err(
            |error| {
                format!(
                    "the grant folder could not be made private: {}",
                    error.kind()
                )
            },
        )?;
    }
    Ok(folder.join("host-screen-grant"))
}

impl Scenario for Launched {
    fn start(&mut self, _host: &Engine, params: &Value) -> Result<Value, String> {
        let requested = requested_variables(params)?;
        self.viewer_key = match self.side {
            Side::LatencyHost => Some(
                params["viewer_key"]
                    .as_str()
                    .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
                    .map(str::to_ascii_lowercase)
                    .ok_or("a latency host is started for one viewer: viewer_key, its invitation's key")?,
            ),
            _ => None,
        };
        let folders = Folders::new(self.side.name())?;
        let checkout = checkout();
        let log = std::fs::File::create(folders.output().join("agent-launch.log"))
            .map_err(|error| format!("the launch log could not be made: {}", error.kind()))?;
        let errors = log
            .try_clone()
            .map_err(|error| format!("the launch log could not be made: {}", error.kind()))?;
        // The agent's own runner says which Node runs the build; the suite never does.
        let node = std::env::var_os("VNILINK_AGENT_NODE").unwrap_or_else(|| "node".into());
        let mut command = Command::new(node);
        command
            .arg(checkout.join("build/run.ts"))
            .arg("testkit:launch")
            .arg(self.side.name())
            .current_dir(&checkout)
            .stdin(Stdio::piped())
            .stdout(log)
            .stderr(errors);
        // Only what the scenario sets, and the suite's whitelisted values.
        for (name, _) in std::env::vars_os() {
            if name.to_str().is_some_and(|name| {
                name.starts_with("VNILINK_LATENCY_") || name == "VNILINK_TEST_PROFILE"
            }) {
                command.env_remove(name);
            }
        }
        command
            .env("VNILINK_LATENCY_ROLE", self.side.role())
            .env("VNILINK_LATENCY_DIR", folders.rendezvous())
            .env("VNILINK_LATENCY_OUTPUT", folders.output())
            .env("VNILINK_LATENCY_SCREEN_GRANT", screen_grant(&checkout)?)
            .env("VNILINK_TEST_PROFILE", folders.profile())
            // Started by the agent, the side's other half is on another
            // computer: it runs no reader and shows its screen only to be seen.
            .env("VNILINK_LATENCY_ACROSS", "1")
            .envs(requested);
        if let Some(key) = &self.viewer_key {
            eprintln!(
                "this screen is shared, to be seen and not controlled, with the suite's viewer {key}"
            );
        }
        let process = SideProcess::spawn(command).map_err(|error| {
            format!(
                "the agent could not start its build's Node: {}",
                error.kind()
            )
        })?;
        self.output = Some(folders.output());
        self.folders = Some(folders);
        self.process = Some(process);
        Ok(json!({}))
    }

    fn wait(&mut self, _host: &Engine, what: &str, _within: Duration) -> Result<Value, String> {
        Err(format!(
            "the {} scenario cannot wait for {what:?}",
            self.side.name()
        ))
    }

    fn put(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        accepted(self.side, name, bytes, self.viewer_key.as_deref())?;
        self.folders
            .as_mut()
            .ok_or("the scenario has not started")?
            .put(name, bytes)
    }

    fn poll(&mut self) -> Result<Polled, String> {
        let running = self.process.as_ref().is_some_and(SideProcess::running);
        let files = match self.folders.as_mut() {
            Some(folders) => folders.poll()?,
            None => Vec::new(),
        };
        Ok(Polled { files, running })
    }

    fn output(&self) -> Option<&Path> {
        self.output.as_deref()
    }

    fn stop(&mut self, _host: &Engine) {
        self.teardown();
    }
}

/// The launcher's process, in a process group of its own (a job object on
/// Windows), watched by a thread that says when it exited.
struct SideProcess {
    /// Leads the side's process group; on Windows its job holds it.
    #[cfg(unix)]
    pid: u32,
    /// Closing it asks the launcher to stop its side.
    stdin: Option<ChildStdin>,
    exited: mpsc::Receiver<()>,
    /// Ends the launcher and everything it started when closed.
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl SideProcess {
    fn spawn(mut command: Command) -> std::io::Result<Self> {
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let mut child: Child = command.spawn()?;
        #[cfg(windows)]
        let job = match kill_on_close(&child) {
            Ok(job) => Some(job),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        #[cfg(unix)]
        let pid = child.id();
        let stdin = child.stdin.take();
        let (sender, exited) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = sender.send(());
        });
        Ok(Self {
            #[cfg(unix)]
            pid,
            stdin,
            exited,
            #[cfg(windows)]
            job,
        })
    }

    fn running(&self) -> bool {
        matches!(self.exited.try_recv(), Err(mpsc::TryRecvError::Empty))
    }

    /// Asks the launcher to stop, waits [`QUIT_WAIT`] for it, then ends its
    /// group and every process it started that is still there.
    fn end(mut self, name: &str) {
        // Every process the side started, while they are still its own.
        #[cfg(unix)]
        let started = descendants(&[self.pid]);
        drop(self.stdin.take());
        let quit = self.exited.recv_timeout(QUIT_WAIT).is_ok();
        if !quit {
            eprintln!("the {name} side did not quit; the agent ends it");
        }
        #[cfg(unix)]
        terminate(self.pid, &started, quit);
        #[cfg(windows)]
        {
            // Closing the job ends every process in it, the launcher's too.
            drop(self.job.take());
        }
        if !quit && self.exited.recv_timeout(END_WAIT).is_err() {
            eprintln!("the {name} side could not be ended");
        }
    }
}

/// Ends the launcher's group, if it did not quit, and whatever of
/// `started` remains, wherever it moved: a launched Electron runs in a
/// group of its own.
#[cfg(unix)]
fn terminate(pid: u32, started: &[Started], quit: bool) {
    let Ok(group) = libc::pid_t::try_from(pid) else {
        return;
    };
    let signal_all = |signal: libc::c_int| {
        if !quit {
            // SAFETY: a negative pid signals the launcher's own process
            // group, which the launcher led and nothing else joins.
            unsafe { libc::kill(-group, signal) };
        }
        for process in started.iter().filter(|process| same(process)) {
            if let Ok(process) = libc::pid_t::try_from(process.pid) {
                // SAFETY: signals a process the side started, checked to be
                // the same process just before.
                unsafe { libc::kill(process, signal) };
            }
        }
    };
    signal_all(libc::SIGTERM);
    let deadline = std::time::Instant::now() + END_WAIT;
    while std::time::Instant::now() < deadline && started.iter().any(same) {
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_all(libc::SIGKILL);
    // On Linux the agent adopts what the side left behind; ended, those
    // are its own children to reap.
    reap(started);
}

/// A job object that ends every process in it when its handle closes,
/// with `child` in it. The launcher's own children join it as they start.
#[cfg(windows)]
fn kill_on_close(child: &Child) -> std::io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::AsRawHandle;

    let job = crate::peer::processes::Job::new()?;
    job.assign(child.as_raw_handle())?;
    Ok(job.into_handle())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A suite sets only the whitelisted variables, to whole numbers; hold
    /// mode is not among them.
    #[test]
    fn a_suite_sets_only_whitelisted_variables_to_whole_numbers() {
        let accepted = requested_variables(
            &json!({ "env": { "VNILINK_LATENCY_FPS": "60", "VNILINK_LATENCY_CONSENT_MS": "600000", "VNILINK_LATENCY_BITRATE": "0" } }),
        );
        assert_eq!(
            accepted,
            Ok(vec![
                ("VNILINK_LATENCY_BITRATE".to_owned(), "0".to_owned()),
                ("VNILINK_LATENCY_CONSENT_MS".to_owned(), "600000".to_owned()),
                ("VNILINK_LATENCY_FPS".to_owned(), "60".to_owned()),
            ]),
            "whitelisted whole numbers in range pass"
        );
        assert_eq!(requested_variables(&json!({})), Ok(Vec::new()));
        for env in [
            json!({ "VNILINK_LATENCY_DIR": "/tmp" }),
            json!({ "VNILINK_LATENCY_ROLE": "host" }),
            json!({ "VNILINK_LATENCY_HOLD_MS": "5000" }),
            json!({ "VNILINK_LATENCY_ACROSS": "0" }),
            json!({ "VNILINK_LATENCY_ACROSS": "1" }),
            json!({ "VNILINK_LATENCY_FPS": "241" }),
            json!({ "VNILINK_LATENCY_BITRATE": "50001" }),
            json!({ "VNILINK_LATENCY_BUSY": "2" }),
            json!({ "VNILINK_LATENCY_CONSENT_MS": "600001" }),
            json!({ "VNILINK_TEST_PROFILE": "1" }),
            json!({ "PATH": "1" }),
            json!({ "LD_PRELOAD": "1" }),
            json!({ "VNILINK_LATENCY_FPS": "60; rm" }),
            json!({ "VNILINK_LATENCY_FPS": 60 }),
            json!({ "VNILINK_LATENCY_FPS": "" }),
            json!({ "VNILINK_LATENCY_FPS": "1234567890" }),
            json!(["VNILINK_LATENCY_FPS"]),
        ] {
            assert!(
                requested_variables(&json!({ "env": env })).is_err(),
                "{env} is refused"
            );
        }
    }

    fn token(key: &str) -> String {
        format!("vnl1.{key}{}", "00".repeat(60))
    }

    /// `read.json` names no program: it holds `{"seconds": 1..600}` and
    /// nothing else, and a file the other role does not publish is not
    /// taken at all.
    #[test]
    fn a_side_takes_only_what_the_other_role_publishes() {
        let viewer = "ab".repeat(32);
        for (side, name, bytes) in [
            (
                Side::LatencyHost,
                "read.json",
                r#"{"seconds":20}"#.to_owned(),
            ),
            (
                Side::LatencyViewer,
                "read.json",
                r#"{"seconds":600}"#.to_owned(),
            ),
            (Side::LatencyHost, "done.json", "{}".to_owned()),
            (
                Side::LatencyHost,
                "viewer.json",
                format!(r#"{{"token":"{}"}}"#, token(&viewer)),
            ),
            (Side::LatencyHost, "viewer-streaming.json", "{}".to_owned()),
            (
                Side::LatencyViewer,
                "host.json",
                format!(r#"{{"token":"{}","endpoint":"e"}}"#, token(&viewer)),
            ),
            (Side::LatencyViewer, "host-ready.json", "{}".to_owned()),
            (
                Side::LatencyViewer,
                "read-done.json",
                r#"{"code":0,"output":""}"#.to_owned(),
            ),
        ] {
            assert_eq!(
                accepted(side, name, bytes.as_bytes(), Some(&viewer)),
                Ok(()),
                "{side:?} takes {name}"
            );
        }
        for (side, name, bytes) in [
            (
                Side::LatencyHost,
                "read.json",
                r#"{"seconds":20,"reader":"/bin/sh","samples":"x","grant":"y"}"#,
            ),
            (Side::LatencyHost, "read.json", r#"{"reader":"/bin/sh"}"#),
            (Side::LatencyHost, "read.json", r#"{"seconds":0}"#),
            (Side::LatencyHost, "read.json", r#"{"seconds":601}"#),
            (Side::LatencyHost, "read.json", r#"{"seconds":"20"}"#),
            (Side::LatencyHost, "done.json", r#"{"x":1}"#),
            (Side::LatencyHost, "host.json", "{}"),
            (Side::LatencyHost, "read-done.json", "{}"),
            (Side::LatencyHost, "host-cpu.json", "{}"),
            (Side::LatencyViewer, "viewer.json", "{}"),
            (Side::LatencyViewer, "viewer-streaming.json", "{}"),
            (
                Side::LatencyViewer,
                "host.json",
                r#"{"token":"t","endpoint":"e","extra":1}"#,
            ),
            (Side::LatencyViewer, "anything.json", "{}"),
            (Side::LatencyHost, "done.json", "[]"),
            (Side::LatencyHost, "done.json", "not json"),
        ] {
            assert!(
                accepted(side, name, bytes.as_bytes(), Some(&viewer)).is_err(),
                "{side:?} refuses {name} holding {bytes}"
            );
        }
    }

    /// A latency host takes only the viewer it was started for: another
    /// computer's invitation is refused.
    #[test]
    fn a_host_takes_only_the_viewer_it_was_started_for() {
        let viewer = "ab".repeat(32);
        let other = "cd".repeat(32);
        let named = |key: &str| format!(r#"{{"token":"{}"}}"#, token(key));
        assert_eq!(
            accepted(
                Side::LatencyHost,
                "viewer.json",
                named(&viewer).as_bytes(),
                Some(&viewer)
            ),
            Ok(())
        );
        for (bytes, declared) in [
            (named(&other), Some(viewer.as_str())),
            (named(&viewer), None),
            (r#"{"token":"vnl1.zz"}"#.to_owned(), Some(viewer.as_str())),
        ] {
            assert!(
                accepted(Side::LatencyHost, "viewer.json", bytes.as_bytes(), declared).is_err(),
                "{bytes} is not the declared viewer {declared:?}"
            );
        }
    }

    /// The real launch refuses a `read.json` naming a program, and the side
    /// never sees it: the file is not written where the side reads.
    #[test]
    fn a_read_naming_a_program_never_reaches_the_side() -> crate::TestResult {
        let mut launched = Launched::new(Side::LatencyHost);
        launched.viewer_key = Some("ab".repeat(32));
        launched.folders = Some(Folders::new("launch")?);
        let rendezvous = launched
            .folders
            .as_ref()
            .map(Folders::rendezvous)
            .ok_or("folders")?;
        let read = br#"{"seconds":20,"reader":"/bin/sh","samples":"-c","grant":"touch /tmp/x"}"#;
        assert!(
            launched.put("read.json", read).is_err(),
            "the put is refused"
        );
        assert!(
            !rendezvous.join("read.json").exists(),
            "nothing reaches the side's rendezvous folder"
        );
        launched.put("read.json", br#"{"seconds":20}"#)?;
        assert_eq!(
            std::fs::read(rendezvous.join("read.json"))?,
            br#"{"seconds":20}"#
        );
        Ok(())
    }
}
