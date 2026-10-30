// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! `terminal`: the agent shares its engine's real shell
//! (`crate::terminal::open_shell`), working in a scratch folder of the
//! scenario's own, so a suite measures a typed key's echo and a busy
//! shell's output across two computers. It runs only when the agent's
//! owner allowed it (`--allow terminal`), and the agent tells its owner on
//! its terminal when the paired computer has a shell here, in which folder,
//! and when that ended, or what of it is still left.
//!
//! While it runs the agent shares the Terminal service and nothing else,
//! and keeps no shell past its connection: persistent terminals are
//! refused. Stopping it shares nothing again, ends every process its
//! shells started ([`ContainedShells`]) and removes its folder. What is
//! typed and printed is never recorded, as everywhere in the engine. The
//! suite names no path: given `{"output_bytes": n}`, the agent writes
//! [`OUTPUT_FILE`] of that size in the shell's folder for a shell that has
//! no quick generator of output to type.

use std::sync::Arc;
#[cfg(unix)]
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vnilink::terminal::{HostTerminalFactory, TerminalBackend, TerminalPolicy, TerminalSize};
use vnilink::{BackendStartup, Error, ServiceKind, SharingPolicy};

use super::{released, whole, Scenario};
use crate::terminal::{open_shell, output_text, OUTPUT_FILE};
use crate::{Engine, Scratch};

/// How long the last scenario's viewer may take to go.
const RELEASE_WAIT: Duration = Duration::from_secs(10);
/// How long the shells and what they started may take to end once ended,
/// and a cancelled shell's startup to let the terminal policy change.
const END_WAIT: Duration = Duration::from_secs(10);
/// How often the stop looks again. No event marks a shell's record leaving
/// the engine (`hosted_terminal_sessions`): a hosted service's close comes
/// before its shell ends, so the stop polls.
const LOOK_EVERY: Duration = Duration::from_millis(20);
/// The most bytes of output the suite may ask the agent to write.
const OUTPUT_MOST: u64 = 64 * 1024 * 1024;

#[derive(Default)]
pub(super) struct TerminalHost {
    /// The shell's working folder, removed at stop.
    folder: Option<Scratch>,
    /// The agent's terminal policy before the scenario refused persistence.
    policy: Option<TerminalPolicy>,
    /// The shells the engine opened, and every process they started.
    shells: Option<Arc<ContainedShells>>,
    /// Whether the agent told its owner the paired computer has a shell.
    announced: bool,
}

impl Scenario for TerminalHost {
    fn start(&mut self, host: &Engine, params: &Value) -> Result<Value, String> {
        let output_bytes = usize::try_from(whole(params, "output_bytes", 0..=OUTPUT_MOST, 0)?)
            .map_err(|error| error.to_string())?;
        released(host, RELEASE_WAIT)?;
        let folder = Scratch::new("terminal");
        if output_bytes > 0 {
            std::fs::write(folder.join(OUTPUT_FILE), output_text(output_bytes))
                .map_err(|error| format!("{OUTPUT_FILE} could not be written: {}", error.kind()))?;
        }
        let shells = Arc::new(ContainedShells::new(
            open_shell(folder.path()).map_err(|error| error.to_string())?,
        )?);
        self.folder = Some(folder);
        let policy = host
            .core
            .terminal_policy()
            .map_err(|error| error.to_string())?;
        host.core
            .set_terminal_policy(TerminalPolicy {
                allow_persistence: false,
                ..policy
            })
            .map_err(|error| error.to_string())?;
        self.policy = Some(policy);
        host.core
            .set_terminal_factory(Some(Arc::clone(&shells) as Arc<dyn HostTerminalFactory>))
            .map_err(|error| error.to_string())?;
        self.shells = Some(shells);
        // Exactly the terminal: whatever else was shared is not.
        host.core
            .set_sharing_policy(SharingPolicy {
                terminal: true,
                ..SharingPolicy::default()
            })
            .map_err(|error| error.to_string())?;
        if let Some(folder) = &self.folder {
            println!(
                "the paired computer now has a shell on this computer, working in {}",
                folder.path().display()
            );
            self.announced = true;
        }
        Ok(json!({ "output_file": (output_bytes > 0).then_some(OUTPUT_FILE) }))
    }

    fn wait(&mut self, _host: &Engine, what: &str, _within: Duration) -> Result<Value, String> {
        Err(format!("the terminal scenario cannot wait for {what:?}"))
    }

    fn stop(&mut self, host: &Engine) {
        let mut left = Vec::new();
        if let Err(error) = host.core.set_service_sharing(ServiceKind::Terminal, false) {
            left.push(format!(
                "terminal sharing could not be turned off ({error})"
            ));
        }
        // What the shells started is ended while their parents still show
        // where it came from; then the shells, which no factory ends.
        let ending = self.shells.as_ref().map(|shells| shells.end());
        if let Err(error) = host.core.set_terminal_factory(None) {
            left.push(format!("the shells could not be ended ({error})"));
        }
        let deadline = Instant::now() + END_WAIT;
        let shells_left = loop {
            let shells = host
                .core
                .hosted_terminal_sessions()
                .map_or(0, |shells| shells.len());
            if shells == 0 || Instant::now() > deadline {
                break shells;
            }
            std::thread::sleep(LOOK_EVERY);
        };
        if shells_left > 0 {
            left.push(format!("{shells_left} shell(s) still open in the engine"));
        }
        if let (Some(shells), Some(ending)) = (self.shells.take(), ending) {
            let processes = shells.ended(ending, deadline);
            if !processes.is_empty() {
                left.push(format!("processes still running: {}", processes.join(", ")));
            }
        }
        if let Some(policy) = self.policy.take() {
            // A cancelled shell's startup holds the policy briefly.
            loop {
                match host.core.set_terminal_policy(policy) {
                    Err(vnilink::Error::Busy) if Instant::now() < deadline => {
                        std::thread::sleep(LOOK_EVERY);
                    }
                    Err(error) => {
                        left.push(format!(
                            "its terminal policy could not be restored ({error})"
                        ));
                        break;
                    }
                    Ok(()) => break,
                }
            }
        }
        if let Some(folder) = self.folder.take() {
            let path = folder.path().to_path_buf();
            if let Err(error) = folder.close_by(Instant::now() + END_WAIT) {
                left.push(format!("{} could not be removed ({error})", path.display()));
            }
            if std::mem::take(&mut self.announced) {
                if left.is_empty() {
                    println!(
                        "the paired computer's shell here has ended and {} was removed",
                        path.display()
                    );
                } else {
                    println!(
                        "the paired computer's shell here was stopped, but not all of it ended; check this computer: {}",
                        left.join("; ")
                    );
                }
            }
        }
        for problem in left {
            eprintln!("the terminal scenario: {problem}");
        }
    }
}

/// The scenario's shell backend, which keeps every process its shells
/// start within reach so the stop ends them all, not only the shells. The
/// engine's backend ends a shell's own process groups, as a terminal
/// application does; a scenario on someone else's computer ends what was
/// put in the background, under `nohup` or in a session of its own too.
///
/// On Unix each shell leads a session of its own: the stop ends every
/// process in those sessions and under them (`processes::sessions`). On
/// Linux the agent also adopts orphans (`processes::adopt_orphans`), which
/// finds a process that left the session after its parent ended; on macOS
/// such a process is not found. On Windows each shell is put in a job
/// object as it starts, before it reads any input, and everything it
/// starts joins the job, which the stop ends.
pub(super) struct ContainedShells {
    shell: Arc<dyn HostTerminalFactory>,
    /// The shells' process ids, each a session leader on Unix.
    #[cfg(unix)]
    leaders: Mutex<Vec<u32>>,
    #[cfg(windows)]
    job: crate::peer::processes::Job,
}

/// What the stop found to end.
pub(super) struct Ending {
    #[cfg(unix)]
    processes: Vec<crate::peer::processes::Started>,
    #[cfg(windows)]
    running: u32,
}

impl ContainedShells {
    fn new(shell: Arc<dyn HostTerminalFactory>) -> Result<Self, String> {
        Ok(Self {
            shell,
            #[cfg(unix)]
            leaders: Mutex::new(Vec::new()),
            #[cfg(windows)]
            job: crate::peer::processes::Job::new()
                .map_err(|error| format!("the shells' job could not be made: {error}"))?,
        })
    }

    /// Ends the shells and every process they started.
    fn end(&self) -> Ending {
        #[cfg(unix)]
        {
            use crate::peer::processes::{sessions, signal};
            let leaders = crate::lock(&self.leaders).clone();
            let processes = sessions(&leaders);
            signal(&processes, libc::SIGKILL);
            Ending { processes }
        }
        #[cfg(windows)]
        {
            Ending {
                running: self.job.end(END_WAIT),
            }
        }
    }

    /// What of `ending` still runs by `deadline`, each named.
    fn ended(&self, ending: Ending, deadline: Instant) -> Vec<String> {
        #[cfg(unix)]
        {
            use crate::peer::processes::{reap, same};
            loop {
                reap(&ending.processes);
                let left: Vec<String> = ending
                    .processes
                    .iter()
                    .filter(|process| same(process))
                    .map(|process| process.pid.to_string())
                    .collect();
                if left.is_empty() || Instant::now() > deadline {
                    return left;
                }
                std::thread::sleep(LOOK_EVERY);
            }
        }
        #[cfg(windows)]
        {
            let _ = deadline;
            match ending.running {
                0 => Vec::new(),
                running => vec![format!("{running} in the shells' job")],
            }
        }
    }
}

impl HostTerminalFactory for ContainedShells {
    fn open(&self, size: TerminalSize, startup: BackendStartup) -> Result<TerminalBackend, Error> {
        let backend = self.shell.open(size, startup)?;
        #[cfg(unix)]
        crate::lock(&self.leaders).push(backend.process_id);
        #[cfg(windows)]
        if self.job.assign_pid(backend.process_id).is_err() {
            // Dropping the backend ends the shell it started.
            drop(backend);
            return Err(Error::Unsupported("a shell the agent can end"));
        }
        Ok(backend)
    }
}
