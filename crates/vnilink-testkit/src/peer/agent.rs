//! The agent's side: an engine of its own that another computer's suite
//! pairs with by file, and the loop that answers that suite's requests by
//! running the scenarios compiled in here.

use std::time::{Duration, Instant};

use vnilink::probes::{AgentChannel, AgentListener};
use vnilink::{ControlRequest, PairingFile};

use super::scenarios::{self, Scenario};
use super::{receive, revision, send, PeerError, Platform, Reply, Request};
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
}

/// The agent's engine, hosting and listening for its suite.
pub struct Agent {
    /// Dropped first: the engine stops offering the agent's protocol
    /// before it shuts down.
    listener: AgentListener,
    engine: Engine,
}

impl Agent {
    /// An engine named `agent` with its identity in memory, hosting and
    /// sharing nothing until a scenario starts. Its reports carry what the
    /// log (`vnilink_log::install`) kept.
    pub fn open(reach: Reach) -> Result<Self, PeerError> {
        let engine = Engine::reachable("agent", reach);
        engine.core.start_hosting()?;
        // Jobs a scenario's peer starts are driven as the application drives them.
        engine.core.start_job_driver();
        let listener = engine.core.serve_agent()?;
        Ok(Self { engine, listener })
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
        let channel = loop {
            let channel = match self.listener.accept(idle) {
                Ok(channel) => channel,
                Err(vnilink::Error::Timeout) => return Ok(Ending::Idle),
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
            running: None,
        };
        let ending = session.answer(&channel, idle);
        session.stop();
        ending
    }
}

/// One suite's requests and the scenario they started.
struct Session<'a> {
    engine: &'a Engine,
    running: Option<Running>,
}

struct Running {
    scenario: Box<dyn Scenario>,
    /// When it started, by the log's clock: its records follow.
    since: u64,
}

impl Session<'_> {
    fn answer(&mut self, channel: &AgentChannel, idle: Duration) -> Result<Ending, PeerError> {
        loop {
            let request = match receive::<Request>(channel, idle) {
                Ok(request) => request,
                Err(PeerError::Engine(vnilink::Error::Timeout)) => return Ok(Ending::Idle),
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
                        &Reply::Refused {
                            reason: reason.clone(),
                        },
                    )?;
                    return Err(PeerError::Refused(reason));
                }
            }
            send(channel, &self.reply(request))?;
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
                        Duration::from_millis(within_ms),
                    ) {
                        Ok(facts) => Reply::Done { facts },
                        Err(reason) => refused(reason),
                    }
                }
            },
            Request::Stop => Reply::Report {
                records: self.stop(),
            },
        }
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
