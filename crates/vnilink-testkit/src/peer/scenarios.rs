// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The scenarios compiled into the agent: each the host half of a suite or
//! a measurement. A scenario shares only its own service while it runs and
//! works only in the agent's own folders; the suite names it and passes
//! parameters, never a command or a path. Files shares a whole disk or
//! nothing, so the files scenario shares this computer's whole disk while
//! it runs; the paths it hands the suite name only its own folder.
//!
//! `files`, `desktop`, `camera` and `terminal` serve a suite on the agent's
//! engine; `latency-host`, `latency-viewer` and `perceived-stimulus` start
//! one side of a measurement from the agent's own checkout ([`launch`]).
//! A scenario that reaches beyond the agent's engine runs only with its
//! owner's allowance ([`needs`]).

mod camera;
mod desktop;
mod launch;
mod terminal;

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::Grants;

use super::agent::Allowance;
use super::Polled;
use crate::Engine;

/// Every scenario's name, for the agent's refusal of another.
pub(crate) const NAMES: [&str; 7] = [
    "files",
    "desktop",
    "camera",
    "terminal",
    "latency-host",
    "latency-viewer",
    "perceived-stimulus",
];

/// The host half of one scenario, run by the agent between Start and Stop.
pub(crate) trait Scenario: Send {
    /// Prepares the agent's engine; the facts the controller half needs.
    fn start(&mut self, host: &Engine, params: &Value) -> Result<Value, String>;
    /// Waits up to `within` for `what`; facts about how it ended.
    fn wait(&mut self, host: &Engine, what: &str, within: Duration) -> Result<Value, String>;
    /// Shares nothing again and removes what the scenario made.
    fn stop(&mut self, host: &Engine);
    /// Gives the scenario's side the rendezvous file `name`.
    fn put(&mut self, _name: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("this scenario exchanges no files".to_owned())
    }
    /// The rendezvous files its side published since the last poll, and
    /// whether that side still runs.
    fn poll(&mut self) -> Result<Polled, String> {
        Err("this scenario exchanges no files".to_owned())
    }
    /// The folder of the evidence the scenario's side leaves, which the
    /// suite may read.
    fn output(&self) -> Option<&Path> {
        None
    }
}

/// What the agent's owner must allow before the scenario `name` runs: the
/// latency sides show and read this computer's real screen, `terminal`
/// gives the paired computer a real shell here, and `files` shares this
/// computer's whole disk with it.
pub(crate) fn needs(name: &str) -> Option<Allowance> {
    match name {
        "latency-host" | "latency-viewer" => Some(Allowance::Screen),
        "terminal" => Some(Allowance::Terminal),
        "files" => Some(Allowance::Files),
        _ => None,
    }
}

/// The scenario named `name`.
pub(crate) fn named(name: &str) -> Option<Box<dyn Scenario>> {
    match name {
        "files" => Some(Box::new(FilesHost::default())),
        "desktop" => Some(Box::new(desktop::DesktopHost::default())),
        "camera" => Some(Box::new(camera::CameraHost::default())),
        "terminal" => Some(Box::new(terminal::TerminalHost::default())),
        "latency-host" => Some(Box::new(launch::Launched::new(launch::Side::LatencyHost))),
        "latency-viewer" => Some(Box::new(launch::Launched::new(launch::Side::LatencyViewer))),
        "perceived-stimulus" => Some(Box::new(launch::Launched::new(
            launch::Side::PerceivedStimulus,
        ))),
        _ => None,
    }
}

/// Waits up to `within` until `host` serves nothing, so the adapters a
/// scenario installs may be swapped: the last scenario's viewer has gone.
fn released(host: &Engine, within: Duration) -> Result<(), String> {
    let started = Instant::now();
    loop {
        let hosted = host
            .core
            .hosted_services()
            .map_err(|error| error.to_string())?;
        if hosted.is_empty() {
            return Ok(());
        }
        if started.elapsed() > within {
            return Err(format!(
                "the agent still serves the last scenario's viewer after {} s",
                within.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A whole number in `params` at `field`, within `range`, or `default`
/// when absent.
fn whole(
    params: &Value,
    field: &str,
    range: std::ops::RangeInclusive<u64>,
    default: u64,
) -> Result<u64, String> {
    match params.get(field) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|value| range.contains(value))
            .ok_or_else(|| {
                format!(
                    "{field} must be a whole number from {} to {}",
                    range.start(),
                    range.end()
                )
            }),
    }
}

/// What a host shares when it shares nothing.
const NOTHING: Grants = Grants {
    view: false,
    control: false,
    clipboard: false,
    transfer: false,
};

/// Receives the files the suite uploads into a scratch folder of its own,
/// over a files session, which Files sharing alone admits. Files shares
/// a whole disk or nothing, so while it runs the paired computer may read
/// and change this computer's whole disk: it runs only with its owner's
/// allowance (`--allow files`, which readies the disk before any computer
/// connects), grants Files only until it stops, and says so on the agent's
/// terminal when the disk is shared and when sharing ends. The suite is
/// handed the inbox as the engine names it, a root and a path, and names
/// no path itself.
#[derive(Default)]
struct FilesHost {
    /// The folder the uploads land in, removed at stop.
    inbox: Option<crate::Scratch>,
    /// Jobs the agent held when the scenario started.
    earlier: HashSet<u64>,
    /// Whether the agent told its owner the disk is shared.
    announced: bool,
}

impl Scenario for FilesHost {
    fn start(&mut self, host: &Engine, _params: &Value) -> Result<Value, String> {
        let inbox = crate::Scratch::new("files-inbox");
        let path = inbox.path().to_path_buf();
        self.inbox = Some(inbox);
        self.earlier = host
            .core
            .job_list()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(|job| job.id)
            .collect();
        if host
            .core
            .file_roots()
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            return Err(
                "the agent's disk is not ready to share; its owner allows that with --allow files"
                    .to_owned(),
            );
        }
        let inbox = crate::files::Location::of(&host.core, &path);
        host.core
            .set_desktop_sharing(Grants {
                transfer: true,
                ..NOTHING
            })
            .map_err(|error| error.to_string())?;
        println!(
            "the paired computer may now read and change this computer's whole disk; its uploads land in {}",
            path.display()
        );
        self.announced = true;
        Ok(json!({ "root": inbox.root, "inbox": inbox.relative }))
    }

    fn wait(&mut self, host: &Engine, what: &str, within: Duration) -> Result<Value, String> {
        if what != "received" {
            return Err(format!("the files scenario cannot wait for {what:?}"));
        }
        let started = Instant::now();
        loop {
            let jobs = host.core.job_list().map_err(|error| error.to_string())?;
            let ended = jobs.into_iter().find(|job| {
                !job.sender && !self.earlier.contains(&job.id) && job.state.is_terminal()
            });
            if let Some(job) = ended {
                self.earlier.insert(job.id);
                return Ok(json!({
                    "state": format!("{:?}", job.state),
                    "files": job.files_done,
                    "bytes": job.done_bytes,
                }));
            }
            if started.elapsed() > within {
                return Err(format!("no upload ended within {} s", within.as_secs()));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn stop(&mut self, host: &Engine) {
        // Withdrawing Files sharing ends the paired computer's reach into
        // the disk: every request of its own is refused from here on.
        let withdrawn = host.core.set_desktop_sharing(NOTHING);
        let removed = self.inbox.take().map(|inbox| {
            let path = inbox.path().to_path_buf();
            (inbox.close(), path)
        });
        if !std::mem::take(&mut self.announced) {
            if let Err(error) = withdrawn {
                eprintln!("the agent could not stop sharing: {error}");
            }
            return;
        }
        match (withdrawn, removed) {
            (Err(error), _) => println!(
                "the agent could not stop sharing this computer's disk ({error}); end the agent to stop it"
            ),
            (Ok(()), Some((Err(error), path))) => println!(
                "the paired computer no longer has this computer's disk, but {} could not be removed ({error})",
                path.display()
            ),
            (Ok(()), _) => {
                println!("the paired computer no longer has this computer's disk")
            }
        }
    }
}
