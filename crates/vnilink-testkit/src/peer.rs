//! A host on another computer: the test agent (`vnilink-testkit agent`)
//! runs a scenario's host half on its own engine, and a suite on this
//! computer runs the controller half against it through a [`RemoteHost`].
//!
//! The two pair as the application pairs: the agent writes a `.vnl` file
//! and prints its check digits; the suite opens the file and confirms those
//! digits ([`PEER_CHECK`]), then connects, and the agent's owner types the
//! digits the suite printed. Nobody is approved without a person reading
//! digits across. Each side's identity lives in memory and ends with its
//! process; the agent serves one controller per run.
//!
//! The suite then opens the agent's channel (`vnilink::probes::AgentChannel`)
//! and speaks [`Request`]s, each answered by one [`Reply`], as JSON. Both
//! sides must be built from the same sources ([`revision`]): the protocol
//! is edited in place, never negotiated. Scenarios are compiled into the
//! agent; a suite names one and never sends a command or a path to run.
//! A record names the area and the peer's fingerprint, never content.

pub mod agent;
mod scenarios;

use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vnilink::probes::AgentChannel;
use vnilink::{SessionPath, SessionPhase};

use crate::{Engine, Reach};

/// The agent's pairing file a suite runs against, from `--peer`.
pub const PEER: &str = "VNILINK_PEER";
/// The check digits the agent printed beside that file, from `--peer-check`.
pub const PEER_CHECK: &str = "VNILINK_PEER_CHECK";

/// How long the agent's owner has to type the suite's digits: the engine
/// waits a minute for an answer, and the dial a little more.
const APPROVAL_WAIT: Duration = Duration::from_secs(75);
/// How long a request may take to be answered when it does not say.
const ANSWER_WAIT: Duration = Duration::from_secs(60);

/// What a computer runs: its operating system, architecture and build
/// profile. Never its name or its user's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Platform {
    pub os: String,
    pub arch: String,
    pub profile: String,
}

impl Platform {
    /// This process's platform.
    pub fn this() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_owned(),
        }
    }
}

/// What the suite asks of the agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    /// The first request: both sides must run the same [`revision`].
    Hello {
        revision: String,
        platform: Platform,
    },
    /// Starts the scenario `scenario` with `params`; one at a time.
    Start { scenario: String, params: Value },
    /// Waits up to `within_ms` for the running scenario's `what`.
    Await { what: String, within_ms: u64 },
    /// Ends the running scenario, returning what the agent recorded.
    Stop,
}

/// The agent's answer to one [`Request`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// The answer to Hello: the agent's platform.
    Welcome { platform: Platform },
    /// The scenario started; `facts` are what the controller half needs.
    Ready { facts: Value },
    /// What was awaited happened.
    Done { facts: Value },
    /// The scenario ended; the agent's records since it started, as JSON
    /// lines.
    Report { records: Vec<String> },
    /// The request was not carried out, and why.
    Refused { reason: String },
}

/// Why a peer could not be paired with or did not answer.
#[derive(Debug)]
pub enum PeerError {
    /// The engine refused, such as wrong digits or an expired file.
    Engine(vnilink::Error),
    /// The other side refused a request, saying why.
    Refused(String),
    /// The other side answered something this side does not speak.
    Malformed,
    /// The pairing file could not be read.
    File(std::io::Error),
}

impl fmt::Display for PeerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => write!(formatter, "{error}"),
            Self::Refused(reason) => write!(formatter, "the agent refused: {reason}"),
            Self::Malformed => write!(formatter, "the other side answered out of protocol"),
            Self::File(error) => write!(formatter, "the pairing file could not be read: {error}"),
        }
    }
}

impl std::error::Error for PeerError {}

impl From<vnilink::Error> for PeerError {
    fn from(error: vnilink::Error) -> Self {
        Self::Engine(error)
    }
}

/// The revision both sides must share: a digest of the agent protocol and
/// its scenarios, so a suite and an agent built from different sources
/// refuse each other instead of misreading each other. Line endings are
/// ignored, as a Windows checkout writes them differently.
pub fn revision() -> String {
    const SOURCES: [&str; 3] = [
        include_str!("peer.rs"),
        include_str!("peer/agent.rs"),
        include_str!("peer/scenarios.rs"),
    ];
    let mut hasher = blake3::Hasher::new();
    for source in SOURCES {
        hasher.update(source.replace('\r', "").as_bytes());
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

/// Sends one message on `channel`.
fn send<T: Serialize>(channel: &AgentChannel, message: &T) -> Result<(), PeerError> {
    let bytes = serde_json::to_vec(message).map_err(|_| PeerError::Malformed)?;
    Ok(channel.send(bytes)?)
}

/// The next message on `channel`, waiting up to `within`.
fn receive<T: for<'de> Deserialize<'de>>(
    channel: &AgentChannel,
    within: Duration,
) -> Result<T, PeerError> {
    let bytes = channel.recv(within)?;
    serde_json::from_slice(&bytes).map_err(|_| PeerError::Malformed)
}

/// The suite's side of a run against the agent: its own engine, which
/// other computers can reach, and its handle on the agent.
pub struct Remote {
    pub suite: Engine,
    pub host: RemoteHost,
}

impl Remote {
    /// The agent this run names ([`PEER`]), paired with an engine of the
    /// suite's own; `None` when the run names none. Fails the suite when
    /// the agent cannot be paired with: a run that names one never falls
    /// back to this computer alone.
    pub fn from_env() -> Option<Self> {
        std::env::var_os(PEER).filter(|file| !file.is_empty())?;
        let suite = Engine::reachable("controller", Reach::Network);
        let host = match RemoteHost::from_env(&suite)? {
            Ok(host) => host,
            Err(error) => panic!("the agent named by --peer could not be reached: {error}"),
        };
        Some(Self { suite, host })
    }

    /// What the bench records of the agent and the path to it.
    pub fn bench_peer(&self) -> crate::bench::Peer {
        let path = self.host.path(&self.suite);
        crate::bench::Peer {
            platform: self.host.platform().clone(),
            relay: path.as_ref().is_some_and(|path| path.relay),
            rtt_ms: path.map_or(0, |path| path.rtt_ms),
        }
    }
}

/// Fails a suite that runs both engines on this computer when its run
/// names a peer ([`PEER`]): it has no scenario for one, and running alone
/// instead would pass off one computer's result as two's.
pub(crate) fn refuse_peer() {
    if std::env::var_os(PEER).is_some_and(|file| !file.is_empty()) {
        panic!("no peer scenario: this suite runs both engines on this computer; run it without --peer");
    }
}

/// The suite's handle on the agent's engine: paired, connected and
/// greeted, ready to run scenarios.
pub struct RemoteHost {
    channel: AgentChannel,
    endpoint: String,
    session: u64,
    platform: Platform,
}

impl RemoteHost {
    /// The agent this run names ([`PEER`], [`PEER_CHECK`]), paired with
    /// `engine`; `None` when the run names none.
    pub fn from_env(engine: &Engine) -> Option<Result<Self, PeerError>> {
        let file = std::env::var_os(PEER).filter(|file| !file.is_empty())?;
        Some((|| {
            let bytes = std::fs::read(&file).map_err(PeerError::File)?;
            let check = std::env::var(PEER_CHECK).unwrap_or_default();
            Self::pair(engine, &bytes, check.trim())
        })())
    }

    /// Pairs `engine` with the agent that wrote `file`, whose screen shows
    /// `check`; connects once the agent's owner typed the digits this
    /// prints; opens the agent's channel and greets it.
    pub fn pair(engine: &Engine, file: &[u8], check: &str) -> Result<Self, PeerError> {
        let preview = engine.core.open_pairing_file(file)?;
        engine.core.confirm_pairing(&preview, check)?;
        println!(
            "type {} on the agent's terminal to approve this computer",
            engine.core.control_check()
        );
        let pending = engine.core.begin_connect_services(&preview.endpoint_id)?;
        let session = active(engine, pending.id, APPROVAL_WAIT)?;
        let channel = engine.core.dial_agent(&preview.endpoint_id)?;
        let mut host = Self {
            channel,
            endpoint: preview.endpoint_id,
            session,
            platform: Platform::this(),
        };
        match host.ask(
            &Request::Hello {
                revision: revision(),
                platform: Platform::this(),
            },
            ANSWER_WAIT,
        )? {
            Reply::Welcome { platform } => host.platform = platform,
            Reply::Refused { reason } => return Err(PeerError::Refused(reason)),
            _ => return Err(PeerError::Malformed),
        }
        Ok(host)
    }

    /// The agent's endpoint id.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// What the agent's computer runs.
    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    /// The path the suite's connection to the agent sends on now, as the
    /// engine sees it; `None` while it has none.
    pub fn path(&self, engine: &Engine) -> Option<SessionPath> {
        engine
            .core
            .session_paths(self.session)
            .ok()?
            .into_iter()
            .find(|path| path.selected)
    }

    /// Sends `request` and returns its answer, waiting up to `within`.
    pub fn ask(&mut self, request: &Request, within: Duration) -> Result<Reply, PeerError> {
        send(&self.channel, request)?;
        receive(&self.channel, within)
    }

    /// Starts `scenario` on the agent; the facts its controller half needs.
    pub fn start(&mut self, scenario: &str, params: Value) -> Result<Value, PeerError> {
        let request = Request::Start {
            scenario: scenario.to_owned(),
            params,
        };
        match self.ask(&request, ANSWER_WAIT)? {
            Reply::Ready { facts } => Ok(facts),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// Waits up to `within` for the running scenario's `what`.
    pub fn wait(&mut self, what: &str, within: Duration) -> Result<Value, PeerError> {
        let request = Request::Await {
            what: what.to_owned(),
            within_ms: u64::try_from(within.as_millis()).unwrap_or(u64::MAX),
        };
        // The agent answers once it waited; the channel waits a little longer.
        match self.ask(&request, within.saturating_add(ANSWER_WAIT))? {
            Reply::Done { facts } => Ok(facts),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// Ends the running scenario; what the agent recorded while it ran.
    pub fn stop(&mut self) -> Result<Vec<String>, PeerError> {
        match self.ask(&Request::Stop, ANSWER_WAIT)? {
            Reply::Report { records } => Ok(records),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }
}

/// `engine`'s controller session `id`, once active within `within`; a
/// refusal when it closed first.
fn active(engine: &Engine, id: u64, within: Duration) -> Result<u64, PeerError> {
    let started = Instant::now();
    loop {
        let session = engine.core.controller_session(id)?;
        match session.phase {
            SessionPhase::Active => return Ok(id),
            SessionPhase::Failed(_) | SessionPhase::Closed => {
                return Err(vnilink::Error::Unauthorized.into())
            }
            _ if started.elapsed() > within => return Err(vnilink::Error::Timeout.into()),
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::agent::Agent;
    use super::*;
    use crate::{traced, TestResult};

    /// The agent and a suite in one process pair by the agent's file and
    /// both computers' digits, as two computers do, and greet each other;
    /// a suite that confirms other digits pairs with nothing.
    #[test]
    fn a_suite_pairs_with_the_agent_by_its_file_and_digits() -> TestResult {
        let _trace = traced();
        let agent = Agent::open(Reach::Loopback)?;
        let file = agent.export(Duration::from_secs(60))?;
        let suite = Engine::start("controller");
        let wrong = if file.check == "000" { "111" } else { "000" };
        assert!(
            matches!(
                RemoteHost::pair(&suite, &file.bytes, wrong),
                Err(PeerError::Engine(vnilink::Error::PairingCheckMismatch))
            ),
            "other digits pair with nothing"
        );
        let suite_check = suite.core.control_check();
        std::thread::scope(|scope| -> TestResult {
            let serving = scope.spawn(|| {
                // The agent's owner types what the suite printed; the
                // agent's engine asks by the same digits.
                let controller = agent.admit(Instant::now() + APPROVAL_WAIT, |request| {
                    assert_eq!(request.check, suite_check, "both screens show the digits");
                    Some(suite_check.clone())
                })?;
                agent.serve(&controller, Duration::from_secs(10))
            });
            let mut host = RemoteHost::pair(&suite, &file.bytes, &file.check)?;
            assert_eq!(
                host.endpoint(),
                agent.engine().endpoint(),
                "the suite reached the agent"
            );
            assert_eq!(
                host.platform(),
                &Platform::this(),
                "the agent says what it runs"
            );
            assert!(
                matches!(host.start("fax", Value::Null), Err(PeerError::Refused(_))),
                "an agent runs only the scenarios compiled into it"
            );
            drop(host);
            let ending = serving.join().expect("the agent's thread")?;
            assert_eq!(
                ending,
                agent::Ending::PeerGone,
                "the agent ends with its suite"
            );
            Ok(())
        })
    }

    /// A suite built from other sources is refused at Hello, and the agent
    /// says so instead of serving it.
    #[test]
    fn an_agent_refuses_a_suite_of_another_revision() -> TestResult {
        let _trace = traced();
        let agent = Agent::open(Reach::Loopback)?;
        let file = agent.export(Duration::from_secs(60))?;
        let suite = Engine::start("controller");
        let preview = suite.core.open_pairing_file(&file.bytes)?;
        suite.core.confirm_pairing(&preview, &file.check)?;
        std::thread::scope(|scope| -> TestResult {
            let serving = scope.spawn(|| {
                let controller = agent.admit(Instant::now() + APPROVAL_WAIT, |request| {
                    Some(request.check.clone())
                })?;
                agent.serve(&controller, Duration::from_secs(10))
            });
            let pending = suite.core.begin_connect_services(&preview.endpoint_id)?;
            active(&suite, pending.id, APPROVAL_WAIT)?;
            let channel = suite.core.dial_agent(&preview.endpoint_id)?;
            send(
                &channel,
                &Request::Hello {
                    revision: "another".to_owned(),
                    platform: Platform::this(),
                },
            )?;
            let reply: Reply = receive(&channel, ANSWER_WAIT)?;
            assert!(
                matches!(&reply, Reply::Refused { reason } if reason.contains("revision")),
                "a suite of another revision is refused: {reply:?}"
            );
            assert!(
                matches!(
                    serving.join().expect("the agent's thread"),
                    Err(PeerError::Refused(_))
                ),
                "the agent ends rather than serve it"
            );
            Ok(())
        })
    }
}
