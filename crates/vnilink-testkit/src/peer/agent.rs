// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The agent's side: an engine of its own that another computer's suite
//! pairs with by file, and the loop that answers that suite's requests by
//! running the scenarios compiled in here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink::probes::{AgentChannel, AgentListener};
use vnilink::{ControlRequest, PairingFile};

use super::scenarios::{self, Scenario};
use super::{
    clock, decode, encode, folders, receive, revision, send, Named, Numbered, PeerError, Platform,
    Polled, Reply, Request,
};
use crate::{Engine, Reach};

/// The most records a report carries, its last ones.
const REPORTED: usize = 4_000;
/// The most bytes of records a report carries, well inside a frame.
const REPORTED_BYTES: usize = 8 * 1024 * 1024;

/// How the agent's service of its suite ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    /// The suite closed its channel or its computer went away.
    PeerGone,
    /// No request came for the idle limit.
    Idle,
    /// The agent was asked to stop, as by Ctrl-C.
    Interrupted,
}

/// How often a waiting agent looks whether it was asked to stop.
const LOOK_EVERY: Duration = Duration::from_millis(200);

/// The agent's engine, hosting and listening for its suite.
pub struct Agent {
    /// Dropped first: the engine stops offering the agent's protocol
    /// before it shuts down.
    listener: AgentListener,
    engine: Engine,
    /// What the agent's owner lets scenarios do here (`--allow`).
    allowed: Vec<Allowance>,
}

/// What a scenario may do on the agent's computer only when the agent's
/// owner allows it on the agent's command line (`--allow <name>`, repeated
/// or as a comma list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Allowance {
    /// Show or read this computer's real screen: the latency sides.
    Screen,
    /// Give the paired computer a shell here: the terminal scenario.
    Terminal,
    /// Share this computer's whole disk with the paired computer: the
    /// files scenario. Files shares a whole disk or nothing.
    Files,
}

impl Allowance {
    pub const ALL: [Self; 3] = [Self::Screen, Self::Terminal, Self::Files];

    /// Its name on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Terminal => "terminal",
            Self::Files => "files",
        }
    }

    /// What it lets a scenario do, as the agent tells its owner.
    pub fn effect(self) -> &'static str {
        match self {
            Self::Screen => "shows and reads this computer's screen",
            Self::Terminal => "gives the paired computer a shell on this computer",
            Self::Files => "shares this computer's whole disk with the paired computer",
        }
    }

    /// The allowances `value` names, one or a comma list.
    pub fn parse(value: &str) -> Result<Vec<Self>, String> {
        value
            .split(',')
            .map(|name| {
                Self::ALL
                    .into_iter()
                    .find(|allowance| allowance.name() == name.trim())
                    .ok_or_else(|| {
                        format!(
                            "--allow takes {}, or several separated by commas",
                            Self::ALL.map(Self::name).join(" or ")
                        )
                    })
            })
            .collect()
    }
}

impl Agent {
    /// An engine named `agent` with its identity in memory, hosting and
    /// sharing nothing until a scenario starts. Its reports carry what the
    /// log (`vnilink_log::install`) kept.
    pub fn open(reach: Reach) -> Result<Self, PeerError> {
        // The clock the agent answers with starts no later than its engine.
        clock::now_us();
        // Its disk stays its own: only the files scenario shares it, with
        // its owner's allowance.
        let engine = Engine::keeping_its_disk("agent", reach);
        engine.core.start_hosting()?;
        // Jobs a scenario's peer starts are driven as the application drives them.
        engine.core.start_job_driver();
        let listener = engine.core.serve_agent()?;
        Ok(Self {
            engine,
            listener,
            allowed: Vec::new(),
        })
    }

    /// Lets the scenarios that need `allowance` run. Its owner says so on
    /// its command line, before any computer connects. Allowing files
    /// readies the engine's disk for sharing now, since an engine shares
    /// its disk only while no one controls it; the paired computer reaches
    /// it only while the files scenario grants it Files.
    pub fn allow(&mut self, allowance: Allowance) -> Result<(), PeerError> {
        if allowance == Allowance::Files {
            self.engine.core.enable_filesystem_access()?;
        }
        if !self.allowed.contains(&allowance) {
            self.allowed.push(allowance);
        }
        Ok(())
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// A pairing file valid for `ttl` that lets one computer ask to be
    /// approved: write its bytes to a `.vnl` file and print its digits.
    pub fn export(&self, ttl: Duration) -> Result<PairingFile, PeerError> {
        Ok(self.engine.core.export_pairing_file(ttl, true)?)
    }

    /// Waits until `until` for a computer that opened the file to ask, and
    /// approves it with the digits `typed` returns for its request: what
    /// the owner typed, or `None` to reject it. Wrong digits reject that
    /// request and the agent waits for another. Returns the approved
    /// computer's endpoint id.
    pub fn admit(
        &self,
        until: Instant,
        mut typed: impl FnMut(&ControlRequest) -> Option<String>,
    ) -> Result<String, PeerError> {
        loop {
            if Instant::now() > until {
                return Err(vnilink::Error::InvitationExpired.into());
            }
            let Some(request) = self.engine.core.control_request()? else {
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            let Some(digits) = typed(&request) else {
                self.engine.core.reject_control_request()?;
                continue;
            };
            match self.engine.core.confirm_control_request(digits.trim()) {
                Ok(device) => return Ok(device.endpoint_id),
                // Wrong or late digits: that request is over, the file is not.
                Err(vnilink::Error::PairingCheckMismatch | vnilink::Error::NoControlRequest) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Answers the requests of `controller`, the one computer approved,
    /// until it goes away or sends nothing for `idle`. A suite of another
    /// [`revision`] is refused and ends the service.
    pub fn serve(&self, controller: &str, idle: Duration) -> Result<Ending, PeerError> {
        self.serve_until(controller, idle, &AtomicBool::new(false))
    }

    /// As [`Self::serve`], until `stop` is set too: the running scenario
    /// then stops, removing what it made, before this returns.
    pub fn serve_until(
        &self,
        controller: &str,
        idle: Duration,
        stop: &AtomicBool,
    ) -> Result<Ending, PeerError> {
        let waiting = Instant::now();
        let channel = loop {
            if stop.load(Ordering::Acquire) {
                return Ok(Ending::Interrupted);
            }
            let channel = match self.listener.accept(LOOK_EVERY) {
                Ok(channel) => channel,
                Err(vnilink::Error::Timeout) if waiting.elapsed() > idle => {
                    return Ok(Ending::Idle)
                }
                Err(vnilink::Error::Timeout) => continue,
                Err(error) => return Err(error.into()),
            };
            // Only paired computers reach the listener, and only the
            // approved one is paired; a stale channel is closed.
            if channel.peer_id() == controller {
                break channel;
            }
        };
        let mut session = Session {
            engine: &self.engine,
            allowed: &self.allowed,
            running: None,
        };
        let ending = session.answer(&channel, idle, stop);
        session.stop();
        ending
    }
}

/// One suite's requests and the scenario they started.
struct Session<'a> {
    engine: &'a Engine,
    allowed: &'a [Allowance],
    running: Option<Running>,
}

struct Running {
    scenario: Box<dyn Scenario>,
    /// When it started, by the log's clock: its records follow.
    since: u64,
}

impl Session<'_> {
    fn answer(
        &mut self,
        channel: &AgentChannel,
        idle: Duration,
        stop: &AtomicBool,
    ) -> Result<Ending, PeerError> {
        let mut asked = Instant::now();
        loop {
            if stop.load(Ordering::Acquire) {
                return Ok(Ending::Interrupted);
            }
            let Numbered {
                id,
                message: request,
            } = match receive::<Numbered<Request>>(channel, LOOK_EVERY) {
                Ok(numbered) => {
                    asked = Instant::now();
                    numbered
                }
                Err(PeerError::Engine(vnilink::Error::Timeout)) if asked.elapsed() > idle => {
                    return Ok(Ending::Idle)
                }
                Err(PeerError::Engine(vnilink::Error::Timeout)) => continue,
                Err(PeerError::Engine(vnilink::Error::NoSession)) => return Ok(Ending::PeerGone),
                Err(error) => return Err(error),
            };
            if let Request::Hello {
                revision: theirs, ..
            } = &request
            {
                if *theirs != revision() {
                    let reason = format!(
                        "the suite runs test kit revision {theirs} and the agent {}; build both from the same commit",
                        revision()
                    );
                    send(
                        channel,
                        &Numbered {
                            id,
                            message: Reply::Refused {
                                reason: reason.clone(),
                            },
                        },
                    )?;
                    return Err(PeerError::Refused(reason));
                }
            }
            let message = self.reply(request);
            send(channel, &Numbered { id, message })?;
        }
    }

    fn reply(&mut self, request: Request) -> Reply {
        let refused = |reason: String| Reply::Refused { reason };
        match request {
            Request::Hello { .. } => Reply::Welcome {
                platform: Platform::this(),
            },
            Request::Start { scenario, params } => {
                if self.running.is_some() {
                    return refused("a scenario is already running; stop it first".to_owned());
                }
                let Some(mut started) = scenarios::named(&scenario) else {
                    return refused(format!(
                        "the agent has no scenario {scenario:?}; it has {}",
                        scenarios::NAMES.join(", ")
                    ));
                };
                if let Some(needed) = scenarios::needs(&scenario) {
                    if !self.allowed.contains(&needed) {
                        return refused(format!(
                            "the {scenario} scenario {}; its owner allows that by starting the agent with --allow {}",
                            needed.effect(),
                            needed.name()
                        ));
                    }
                }
                let since = vnilink_log::now_ms();
                match started.start(self.engine, &params) {
                    Ok(facts) => {
                        self.running = Some(Running {
                            scenario: started,
                            since,
                        });
                        Reply::Ready { facts }
                    }
                    Err(reason) => {
                        started.stop(self.engine);
                        refused(reason)
                    }
                }
            }
            Request::Await { what, within_ms } => match self.running.as_mut() {
                None => refused("no scenario is running".to_owned()),
                Some(running) => {
                    match running.scenario.wait(
                        self.engine,
                        &what,
                        Duration::from_millis(within_ms).min(super::AWAIT_MOST),
                    ) {
                        Ok(facts) => Reply::Done { facts },
                        Err(reason) => refused(reason),
                    }
                }
            },
            Request::Stop => Reply::Report {
                records: self.stop(),
            },
            Request::Clock => Reply::Clock {
                us: clock::now_us(),
            },
            Request::Put { name, data } => {
                let Some(bytes) = decode(&data) else {
                    return refused(format!("{name}'s bytes are not base64"));
                };
                match self
                    .scenario()
                    .and_then(|scenario| scenario.put(&name, &bytes))
                {
                    Ok(()) => Reply::Done { facts: Value::Null },
                    Err(reason) => refused(reason),
                }
            }
            Request::Poll => match self.scenario().and_then(|scenario| scenario.poll()) {
                Ok(Polled { files, running }) => Reply::Files {
                    files: files
                        .into_iter()
                        .map(|(name, bytes)| Named {
                            name,
                            data: encode(&bytes),
                        })
                        .collect(),
                    running,
                },
                Err(reason) => refused(reason),
            },
            Request::List => match self.output().and_then(|output| folders::list(&output)) {
                Ok(files) => Reply::Listing { files },
                Err(reason) => refused(reason),
            },
            Request::Fetch { path, offset } => {
                match self
                    .output()
                    .and_then(|output| folders::read_chunk(&output, &path, offset))
                {
                    Ok((bytes, end)) => Reply::Chunk {
                        data: encode(&bytes),
                        end,
                    },
                    Err(reason) => refused(reason),
                }
            }
        }
    }

    /// The running scenario.
    fn scenario(&mut self) -> Result<&mut dyn Scenario, String> {
        match self.running.as_mut() {
            Some(running) => Ok(running.scenario.as_mut()),
            None => Err("no scenario is running".to_owned()),
        }
    }

    /// The running scenario's output folder.
    fn output(&mut self) -> Result<std::path::PathBuf, String> {
        self.scenario()?
            .output()
            .map(std::path::Path::to_path_buf)
            .ok_or_else(|| "this scenario leaves no files".to_owned())
    }

    /// Ends the running scenario, if any; the records since it started.
    fn stop(&mut self) -> Vec<String> {
        let Some(mut running) = self.running.take() else {
            return Vec::new();
        };
        running.scenario.stop(self.engine);
        records_since(running.since)
    }
}

/// However the service ends, a panic in a reply included, the running
/// scenario stops and removes what it made.
impl Drop for Session<'_> {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The agent's records since `since`, its last ones within a report's
/// bounds.
fn records_since(since: u64) -> Vec<String> {
    let mut records: Vec<String> = vnilink_log::export()
        .lines()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|event| event.get("ms").and_then(serde_json::Value::as_u64))
                .is_some_and(|ms| ms >= since)
        })
        .map(str::to_owned)
        .collect();
    let mut bytes = 0;
    let keep = records
        .iter()
        .rev()
        .take(REPORTED)
        .take_while(|line| {
            bytes += line.len() + 8;
            bytes <= REPORTED_BYTES
        })
        .count();
    records.drain(..records.len() - keep);
    records
}

#[cfg(test)]
mod tests {
    use super::Allowance;

    /// `--allow` takes one allowance or a comma list of them, and nothing
    /// it does not know.
    #[test]
    fn allow_takes_one_allowance_or_a_comma_list() {
        assert_eq!(Allowance::parse("screen"), Ok(vec![Allowance::Screen]));
        assert_eq!(
            Allowance::parse("screen, terminal,files"),
            Ok(vec![
                Allowance::Screen,
                Allowance::Terminal,
                Allowance::Files
            ])
        );
        for unknown in ["", "input", "screen,", "terminal,disk"] {
            assert!(
                Allowance::parse(unknown)
                    .is_err_and(|reason| reason.contains("screen or terminal or files")),
                "{unknown:?} is refused, naming what --allow takes"
            );
        }
    }
}
