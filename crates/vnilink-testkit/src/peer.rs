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
//!
//! Times cross as each side's monotonic clock, related by an estimated
//! offset ([`clock`]). Files cross only between a scenario's own folders
//! and the suite (`folders`): rendezvous files by plain name, evidence
//! read from its output folder. A build script that does not link the kit
//! reaches the agent through [`bridge`].

pub mod agent;
pub mod bridge;
pub mod clock;
mod folders;
pub mod processes;
mod scenarios;

use std::fmt;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vnilink::probes::AgentChannel;
use vnilink::{SessionPath, SessionPhase};

use crate::{Engine, Reach};

pub use clock::{ClockOffset, ClockSample};

/// The agent's pairing file a suite runs against, from `--peer`.
pub const PEER: &str = "VNILINK_PEER";
/// The check digits the agent printed beside that file, from `--peer-check`.
pub const PEER_CHECK: &str = "VNILINK_PEER_CHECK";

/// How long the agent's owner has to type the suite's digits: the engine
/// waits a minute for an answer, and the dial a little more.
const APPROVAL_WAIT: Duration = Duration::from_secs(75);
/// How long a request may take to be answered when it does not say.
const ANSWER_WAIT: Duration = Duration::from_secs(60);
/// How long a stop may take to be answered: a launched side is given 30 s
/// to quit and 10 s more to be ended (`scenarios::launch`), well inside.
const STOP_WAIT: Duration = Duration::from_secs(90);
/// The longest an `Await` waits on the agent.
pub const AWAIT_MOST: Duration = Duration::from_secs(10 * 60);

/// A request or reply with its sequence number: each reply carries its
/// request's, so an answer that arrives after its request gave up is told
/// apart from the next one's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Numbered<T> {
    pub id: u64,
    pub message: T,
}

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
    /// The agent's clock now ([`clock::now_us`]); the suite asks several
    /// times to estimate how far it is from its own ([`ClockOffset`]).
    Clock,
    /// Gives the running scenario's side the rendezvous file `name`, its
    /// bytes in base64: what the other side published for it.
    Put { name: String, data: String },
    /// The rendezvous files the running scenario's side published since
    /// the last poll, and whether that side still runs.
    Poll,
    /// The files under the running scenario's output folder.
    List,
    /// Up to [`CHUNK`] bytes of the output file `path`, from `offset`.
    Fetch { path: String, offset: u64 },
}

/// The most bytes one [`Reply::Chunk`] carries, well inside a frame once
/// in base64.
pub const CHUNK: usize = 4 * 1024 * 1024;

/// The rendezvous files a scenario's side published since the last poll,
/// by name with their bytes, and whether that side still runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Polled {
    pub files: Vec<(String, Vec<u8>)>,
    pub running: bool,
}

/// A file and its bytes in base64.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Named {
    pub name: String,
    pub data: String,
}

/// A file under a scenario's output folder: its path there, with `/`
/// between folders, and its size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Listed {
    pub path: String,
    pub size: u64,
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
    /// The agent's clock when it answered, in microseconds.
    Clock { us: u64 },
    /// Rendezvous files the scenario's side published, and whether it
    /// still runs.
    Files { files: Vec<Named>, running: bool },
    /// The files under the scenario's output folder.
    Listing { files: Vec<Listed> },
    /// Bytes of an output file in base64; `end` when they are its last.
    Chunk { data: String, end: bool },
    /// The request was not carried out, and why.
    Refused { reason: String },
}

/// The files a poll returned, with their bytes: only rendezvous names, so
/// whoever writes them where the name says stays in its folder.
fn polled_files(files: Vec<Named>) -> Result<Vec<(String, Vec<u8>)>, PeerError> {
    files
        .into_iter()
        .map(|file| {
            if !folders::rendezvous_name(&file.name) {
                return Err(PeerError::Malformed);
            }
            let bytes = decode(&file.data).ok_or(PeerError::Malformed)?;
            Ok((file.name, bytes))
        })
        .collect()
}

/// `bytes` as base64, as the protocol carries file contents.
pub fn encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The bytes of base64 `data`; `None` when it is not base64.
pub fn decode(data: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
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

/// The revision both sides must share: a digest of the agent protocol, its
/// scenarios and the code of the measurement sides they launch, so a suite
/// and an agent built from different sources refuse each other instead of
/// misreading each other. Line endings are ignored, as a Windows checkout
/// writes them differently.
pub fn revision() -> String {
    const SOURCES: [&str; 20] = [
        include_str!("../../../build/lib/latency.ts"),
        include_str!("../../../build/lib/perceived.ts"),
        include_str!("../../../build/lib/remote.ts"),
        include_str!("../../../build/lib/test.ts"),
        include_str!("../../../test/latency/across.ts"),
        include_str!("../../../test/latency/main.ts"),
        include_str!("../../../test/latency/stimulus.ts"),
        include_str!("../../../test/perceived/main.ts"),
        include_str!("../../../test/perceived/stimulus.ts"),
        include_str!("peer.rs"),
        include_str!("peer/agent.rs"),
        include_str!("peer/clock.rs"),
        include_str!("peer/folders.rs"),
        include_str!("peer/processes.rs"),
        include_str!("peer/scenarios.rs"),
        include_str!("peer/scenarios/camera.rs"),
        include_str!("peer/scenarios/desktop.rs"),
        include_str!("peer/scenarios/launch.rs"),
        include_str!("peer/scenarios/terminal.rs"),
        include_str!("terminal.rs"),
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
            clock_error_us: None,
        }
    }

    /// As [`Self::bench_peer`], for a run that converted the agent's times
    /// to within `bound_us` ([`ClockOffset::bound_us`]): it records how far
    /// off they may be.
    pub fn bench_peer_timed(&self, bound_us: u64) -> crate::bench::Peer {
        crate::bench::Peer {
            clock_error_us: Some(bound_us),
            ..self.bench_peer()
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
    /// The last request's sequence number.
    sent: u64,
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
        Self::pair_announcing(engine, file, check, |digits| {
            println!("type {digits} on the agent's terminal to approve this computer");
        })
    }

    /// As [`Self::pair`], telling `announce` the digits the agent's owner
    /// must type rather than printing them.
    pub fn pair_announcing(
        engine: &Engine,
        file: &[u8],
        check: &str,
        announce: impl FnOnce(&str),
    ) -> Result<Self, PeerError> {
        let preview = engine.core.open_pairing_file(file)?;
        engine.core.confirm_pairing(&preview, check)?;
        announce(&engine.core.control_check());
        let pending = engine.core.begin_connect_services(&preview.endpoint_id)?;
        let session = active(engine, pending.id, APPROVAL_WAIT)?;
        let channel = engine.core.dial_agent(&preview.endpoint_id)?;
        let mut host = Self {
            channel,
            endpoint: preview.endpoint_id,
            session,
            platform: Platform::this(),
            sent: 0,
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

    /// Sends `request` and returns its answer, waiting up to `within`. An
    /// answer to an earlier request that gave up waiting is set aside.
    pub fn ask(&mut self, request: &Request, within: Duration) -> Result<Reply, PeerError> {
        self.sent += 1;
        let id = self.sent;
        send(
            &self.channel,
            &Numbered {
                id,
                message: request,
            },
        )?;
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let reply: Numbered<Reply> = receive(&self.channel, left)?;
            match reply.id.cmp(&id) {
                std::cmp::Ordering::Less => continue,
                std::cmp::Ordering::Equal => return Ok(reply.message),
                std::cmp::Ordering::Greater => return Err(PeerError::Malformed),
            }
        }
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
        let within = within.min(AWAIT_MOST);
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
        match self.ask(&Request::Stop, STOP_WAIT)? {
            Reply::Report { records } => Ok(records),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// How far the agent's clock is from this process's, from `pings`
    /// questions of its time: the fastest answer's estimate.
    pub fn clock_offset(&mut self, pings: usize) -> Result<ClockOffset, PeerError> {
        let mut samples = Vec::with_capacity(pings);
        for _ in 0..pings.max(1) {
            let asked_us = clock::now_us();
            let Reply::Clock { us } = self.ask(&Request::Clock, ANSWER_WAIT)? else {
                return Err(PeerError::Malformed);
            };
            samples.push(ClockSample {
                asked_us,
                agent_us: us,
                answered_us: clock::now_us(),
            });
        }
        ClockOffset::estimate(&samples).ok_or(PeerError::Malformed)
    }

    /// Gives the running scenario's side the rendezvous file `name`.
    pub fn put(&mut self, name: &str, bytes: &[u8]) -> Result<(), PeerError> {
        let request = Request::Put {
            name: name.to_owned(),
            data: encode(bytes),
        };
        match self.ask(&request, ANSWER_WAIT)? {
            Reply::Done { .. } => Ok(()),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// The rendezvous files the running scenario's side published since
    /// the last poll, with their bytes, and whether that side still runs.
    pub fn poll(&mut self) -> Result<Polled, PeerError> {
        match self.ask(&Request::Poll, ANSWER_WAIT)? {
            Reply::Files { files, running } => Ok(Polled {
                files: polled_files(files)?,
                running,
            }),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// The files under the running scenario's output folder.
    pub fn list(&mut self) -> Result<Vec<Listed>, PeerError> {
        match self.ask(&Request::List, ANSWER_WAIT)? {
            Reply::Listing { files } => Ok(files),
            Reply::Refused { reason } => Err(PeerError::Refused(reason)),
            _ => Err(PeerError::Malformed),
        }
    }

    /// The output file `listed` of the running scenario, fetched a chunk at
    /// a time: at most the size it was listed with, so a file that keeps
    /// growing still ends.
    pub fn fetch(&mut self, listed: &Listed) -> Result<Vec<u8>, PeerError> {
        let size = usize::try_from(listed.size).map_err(|_| PeerError::Malformed)?;
        let mut bytes = Vec::new();
        loop {
            let request = Request::Fetch {
                path: listed.path.clone(),
                offset: bytes.len() as u64,
            };
            match self.ask(&request, ANSWER_WAIT)? {
                Reply::Chunk { data, end } => {
                    let chunk = decode(&data).ok_or(PeerError::Malformed)?;
                    let empty = chunk.is_empty();
                    bytes.extend(chunk);
                    if end || bytes.len() >= size {
                        bytes.truncate(size);
                        return Ok(bytes);
                    }
                    if empty {
                        return Err(PeerError::Malformed);
                    }
                }
                Reply::Refused { reason } => return Err(PeerError::Refused(reason)),
                _ => return Err(PeerError::Malformed),
            }
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
    use super::agent::{Agent, Allowance};
    use super::*;
    use crate::{traced, wait_for, TestResult, DEFAULT_WAIT};
    use serde_json::json;

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

    /// Runs `test` against an agent in this process, paired as two
    /// computers pair, which serves until the test drops its handle.
    fn with_agent(test: impl FnOnce(&Engine, &mut RemoteHost) -> TestResult) -> TestResult {
        with_agent_allowing(&[], |_agent, suite, host| test(suite, host))
    }

    /// As [`with_agent`], for an agent whose owner allowed `allowed`; the
    /// test also sees the agent's engine.
    fn with_agent_allowing(
        allowed: &[Allowance],
        test: impl FnOnce(&Engine, &Engine, &mut RemoteHost) -> TestResult,
    ) -> TestResult {
        let mut agent = Agent::open(Reach::Loopback)?;
        for allowance in allowed {
            agent.allow(*allowance)?;
        }
        let file = agent.export(Duration::from_secs(60))?;
        let suite = Engine::start("controller");
        let suite_check = suite.core.control_check();
        std::thread::scope(|scope| -> TestResult {
            let serving = scope.spawn(|| {
                let controller = agent.admit(Instant::now() + APPROVAL_WAIT, |_| {
                    Some(suite_check.clone())
                })?;
                agent.serve(&controller, Duration::from_secs(30))
            });
            let mut host = RemoteHost::pair(&suite, &file.bytes, &file.check)?;
            test(agent.engine(), &suite, &mut host)?;
            drop(host);
            serving.join().expect("the agent's thread")?;
            Ok(())
        })
    }

    /// The agent answers its clock, and the suite's estimate of how far it
    /// is holds the truth within the error it states: in one process both
    /// read one clock, so the truth is no offset at all.
    #[test]
    fn the_clock_estimate_holds_the_truth_within_its_stated_error() -> TestResult {
        let _trace = traced();
        with_agent(|_suite, host| {
            let offset = host.clock_offset(10)?;
            assert!(
                offset.offset_us.unsigned_abs() <= offset.error_us(),
                "one clock: {offset:?} is within {} µs of no offset",
                offset.error_us()
            );
            let before = clock::now_us();
            let Reply::Clock { us } = host.ask(&Request::Clock, ANSWER_WAIT)? else {
                panic!("the agent answers its clock");
            };
            let after = clock::now_us();
            let converted = offset.suite_us(us);
            assert!(
                converted + offset.error_us() >= before && converted <= after + offset.error_us(),
                "a converted time falls within its error of when it was read"
            );
            Ok(())
        })
    }

    /// The desktop scenario serves its scripted screen, seen only, and
    /// says when it started by the agent's clock: every change it sends
    /// arrives after that start on the suite's clock, and stopping it
    /// shares nothing again.
    #[test]
    fn the_desktop_scenario_serves_a_scripted_screen_timed_by_the_agent() -> TestResult {
        let _trace = traced();
        with_agent(|suite, host| {
            let offset = host.clock_offset(10)?;
            assert!(
                matches!(
                    host.start("desktop", json!({ "script": "drawn" })),
                    Err(PeerError::Refused(_))
                ),
                "a script the scenario does not know is refused"
            );
            let facts = host.start(
                "desktop",
                json!({ "script": "ticking", "every_ms": 50, "share_percent": 30 }),
            )?;
            let started = offset.suite_instant(facts["started_us"].as_u64().expect("its start"));
            suite
                .core
                .set_window_decoders(crate::desktop::window_decoders(false))?;
            let connection = suite.connect_services_to(host.endpoint())?;
            let service = suite.open_desktop(&connection)?;
            let pictures = service.subscribe_video()?;
            let changes: Vec<Instant> =
                crate::desktop::watch_pictures(&pictures, Duration::from_secs(2))
                    .into_iter()
                    .filter(|(_, frame)| frame.role == vnilink::FrameRole::Changed)
                    .map(|(at, _)| at)
                    .collect();
            assert!(
                changes.len() >= 5,
                "the screen's changes arrive: {}",
                changes.len()
            );
            let error = Duration::from_micros(offset.error_us());
            assert!(
                changes.iter().all(|at| *at + error >= started),
                "no change arrives before the screen started"
            );
            drop(pictures);
            service.close();
            suite.core.disconnect_session(connection.id)?;
            host.stop()?;
            Ok(())
        })
    }

    /// The camera scenario sends the pictures asked for once the suite
    /// opens the camera, each stamped with the agent's clock, which the
    /// offset turns into a time before the picture arrived.
    #[test]
    fn the_camera_scenario_sends_pictures_stamped_with_the_agent_clock() -> TestResult {
        let _trace = traced();
        with_agent(|suite, host| {
            let offset = host.clock_offset(10)?;
            let asked = clock::now_us();
            host.start(
                "camera",
                json!({ "pictures": 12, "bytes": 1024, "fps": 60 }),
            )?;
            let connection = suite.connect_services_to(host.endpoint())?;
            let owner = suite.core.attach_controller_service(
                connection.id,
                connection.generation,
                vnilink::ServiceKind::Camera,
            )?;
            let camera = suite.core.open_camera_service(
                owner.id,
                owner.generation,
                crate::camera::config(),
            )?;
            let mut stamps = Vec::new();
            while stamps.len() < 12 {
                let Some(frame) = camera.recv_timeout(crate::DEFAULT_WAIT) else {
                    break;
                };
                stamps.push((offset.suite_us(frame.timestamp_us), clock::now_us()));
            }
            assert_eq!(
                host.wait("sent", Duration::from_secs(10))?,
                json!({ "sent": 12 })
            );
            assert_eq!(stamps.len(), 12, "every picture arrives on loopback");
            assert!(
                stamps
                    .iter()
                    .all(|(sent, arrived)| *sent <= arrived + offset.error_us()),
                "each picture was sent before it arrived: {stamps:?}"
            );
            assert!(
                stamps
                    .iter()
                    .all(|(sent, _)| sent + offset.error_us() >= asked),
                "and after the scenario was asked to start: {stamps:?} from {asked}"
            );
            camera.close();
            suite.core.disconnect_session(connection.id)?;
            host.stop()?;
            Ok(())
        })
    }

    /// Scenarios that show this computer's real screen run only when the
    /// agent's owner allowed it on the agent's command line.
    #[test]
    fn screen_scenarios_need_the_owner_s_allowance() -> TestResult {
        let _trace = traced();
        with_agent(|_suite, host| {
            for scenario in ["latency-host", "latency-viewer"] {
                assert!(
                    matches!(
                        host.start(scenario, json!({ "viewer_key": "ab".repeat(32) })),
                        Err(PeerError::Refused(reason)) if reason.contains("--allow screen")
                    ),
                    "{scenario} needs --allow screen"
                );
            }
            Ok(())
        })
    }

    /// The terminal scenario gives the paired computer a real shell, so it
    /// runs only when the agent's owner allowed exactly that: allowing the
    /// screen is not enough.
    #[test]
    fn the_terminal_scenario_needs_the_owner_s_allowance() -> TestResult {
        let _trace = traced();
        with_agent_allowing(&[Allowance::Screen], |agent, _suite, host| {
            assert!(
                matches!(
                    host.start("terminal", Value::Null),
                    Err(PeerError::Refused(reason)) if reason.contains("--allow terminal")
                ),
                "the terminal scenario needs --allow terminal"
            );
            assert_eq!(
                agent.core.sharing_policy()?,
                vnilink::SharingPolicy::default(),
                "a refused scenario shares nothing"
            );
            Ok(())
        })
    }

    /// The files scenario shares the agent's whole disk, so it runs only
    /// when the agent's owner allowed exactly that, and without the
    /// allowance the agent's disk is never readied to share. Allowed, the
    /// paired computer reaches the disk only while the scenario runs, at
    /// the inbox the agent names, and once it stops is refused again and
    /// the inbox is gone.
    #[test]
    fn the_files_scenario_shares_the_disk_only_while_its_owner_allows_it() -> TestResult {
        let _trace = traced();
        with_agent_allowing(
            &[Allowance::Screen, Allowance::Terminal],
            |agent, _suite, host| {
                assert!(
                    matches!(
                        host.start("files", Value::Null),
                        Err(PeerError::Refused(reason)) if reason.contains("--allow files")
                    ),
                    "the files scenario needs --allow files"
                );
                assert!(
                    agent.core.file_roots()?.is_empty(),
                    "an agent not allowed files never readies its disk"
                );
                Ok(())
            },
        )?;
        with_agent_allowing(&[Allowance::Files], |agent, suite, host| {
            assert!(
                suite.files_at(host.endpoint()).is_err(),
                "before the scenario the paired computer cannot reach the disk"
            );
            let facts = host.start("files", Value::Null)?;
            let (Some(root), Some(inbox)) = (facts["root"].as_u64(), facts["inbox"].as_str())
            else {
                panic!("the files scenario says where uploads land: {facts}");
            };
            let files = suite.files_at(host.endpoint())?;
            let listed = files.pipe.list(root, inbox, 0, 10)?;
            assert_eq!(listed.total, 0, "the inbox starts empty");
            let folder = agent
                .core
                .file_roots()?
                .into_iter()
                .find_map(|(id, path)| {
                    (id == root).then(|| inbox.split('/').fold(path, |path, step| path.join(step)))
                })
                .ok_or("the inbox's root is shared")?;
            assert!(folder.is_dir(), "the inbox is a folder of the agent's");
            assert!(
                !folder.starts_with(agent.folder()),
                "the inbox is the scenario's own, not the engine's folder"
            );
            host.stop()?;
            assert!(
                matches!(
                    files.pipe.list(root, inbox, 0, 10),
                    Err(vnilink::Error::NoSession | vnilink::Error::TransferDenied)
                ),
                "once the scenario stopped the disk is out of reach"
            );
            assert_eq!(
                agent.core.sharing_policy()?,
                vnilink::SharingPolicy::default(),
                "the agent shares nothing again"
            );
            assert!(!folder.exists(), "the inbox is removed");
            suite.core.disconnect_session(files.connection.id)?;
            Ok(())
        })
    }

    /// The engine ([`vnilink::Core::engine_key`]) whose serves the next test
    /// parks: set before its job starts, which may serve at once.
    static PARK_TARGET: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
    /// Whether the parked serve arrived, and whether it may go on.
    static PARK_STATE: std::sync::Mutex<(bool, bool)> = std::sync::Mutex::new((false, false));
    static PARK_CHANGED: std::sync::Condvar = std::sync::Condvar::new();

    /// Parks the targeted serve with its source open until released; any
    /// other serve goes on at once.
    fn park_serve(engine: usize, _job: u64) {
        if *PARK_TARGET.lock().expect("the park target") != Some(engine) {
            return;
        }
        let mut state = PARK_STATE.lock().expect("the park state");
        state.0 = true;
        PARK_CHANGED.notify_all();
        while !state.1 {
            state = PARK_CHANGED.wait(state).expect("the park state");
        }
    }

    /// An upload to the agent still in flight when the files scenario stops
    /// does not complete: stopping withdraws Files sharing, the agent's job
    /// ends without the file, and nothing lands in the inbox, which is gone.
    #[test]
    fn an_upload_in_flight_when_the_files_scenario_stops_lands_nothing() -> TestResult {
        let _trace = traced();
        with_agent_allowing(&[Allowance::Files], |agent, suite, host| {
            let facts = host.start("files", Value::Null)?;
            let (Some(root), Some(inbox)) = (facts["root"].as_u64(), facts["inbox"].as_str())
            else {
                panic!("the files scenario says where uploads land: {facts}");
            };
            let inbox_folder = agent
                .core
                .file_roots()?
                .into_iter()
                .find_map(|(id, path)| {
                    (id == root).then(|| inbox.split('/').fold(path, |path, step| path.join(step)))
                })
                .ok_or("the inbox's root is shared")?;
            std::fs::write(
                suite.folder().join("in-flight.bin"),
                crate::files::payload(256 * 1024, 1901),
            )?;
            // The agent fetches what it receives, so the suite shares Files
            // with it for the upload.
            suite.core.set_desktop_sharing(vnilink::Grants {
                transfer: true,
                ..vnilink::Grants::default()
            })?;
            let files = suite.files_at(host.endpoint())?;
            let here = suite.location();
            let (folder, names) = crate::files::source(&here.path("in-flight.bin"));
            *PARK_STATE.lock().expect("the park state") = (false, false);
            *PARK_TARGET.lock().expect("the park target") = Some(suite.core.engine_key());
            vnilink::Core::set_serve_source_probe(Some(park_serve));
            suite.core.files_service_upload(
                files.service.id,
                files.service.generation,
                here.root,
                &folder,
                &names,
                root,
                inbox,
            )?;
            let parked = {
                let state = PARK_STATE.lock().expect("the park state");
                let (state, timeout) = PARK_CHANGED
                    .wait_timeout_while(state, DEFAULT_WAIT, |state| !state.0)
                    .expect("the park state");
                !timeout.timed_out() && state.0
            };
            let stopped = host.stop();
            {
                let mut state = PARK_STATE.lock().expect("the park state");
                state.1 = true;
                PARK_CHANGED.notify_all();
            }
            vnilink::Core::set_serve_source_probe(None);
            *PARK_TARGET.lock().expect("the park target") = None;
            assert!(parked, "the agent's fetch reached the suite's serve");
            stopped?;
            let ended = wait_for("the agent's job to end", DEFAULT_WAIT, || {
                let jobs = agent.core.job_list().ok()?;
                let job = jobs.into_iter().find(|job| !job.sender)?;
                (job.state != vnilink::JobState::Transferring).then_some(job)
            });
            assert_ne!(
                ended.state,
                vnilink::JobState::Complete,
                "an upload in flight at the stop does not complete: {ended:?}"
            );
            assert_eq!(ended.files_done, 0, "no file landed: {ended:?}");
            crate::holds("the inbox staying gone", Duration::from_millis(500), || {
                !inbox_folder.exists()
            });
            suite.core.disconnect_session(files.connection.id)?;
            Ok(())
        })
    }

    /// While the terminal scenario runs the agent shares its shell and
    /// nothing else, keeps none past its connection, and runs it in the
    /// scenario's own folder; stopping it ends the shell and everything it
    /// started, in the background or under `nohup` too, removes the folder
    /// and shares nothing again, and the shell cannot be resumed. Nothing
    /// typed or printed is recorded, on either side or in the agent's
    /// report.
    #[test]
    fn the_terminal_scenario_shares_a_shell_in_its_own_folder_until_stopped() -> TestResult {
        use crate::peer::processes::runs;
        use crate::terminal::{contains, read_until, start_lingering, ENTER, OUTPUT_FILE, SIZE};
        use vnilink::terminal::TerminalOptions;
        use vnilink::{ServicePermissions, SharingPolicy};

        let trace = traced();
        with_agent_allowing(&[Allowance::Terminal], |agent, suite, host| {
            let facts = host.start("terminal", json!({ "output_bytes": 4096 }))?;
            assert_eq!(
                facts,
                json!({ "output_file": OUTPUT_FILE }),
                "the agent wrote the file a shell types"
            );
            assert_eq!(
                agent.core.sharing_policy()?,
                SharingPolicy {
                    terminal: true,
                    ..SharingPolicy::default()
                },
                "the agent shares the terminal and nothing else"
            );
            assert_eq!(
                agent.core.service_permissions(&suite.endpoint())?,
                ServicePermissions {
                    terminal: true,
                    ..ServicePermissions::default()
                },
                "the suite may open the agent's terminal and nothing else"
            );
            let connection = suite.connect_services_to(host.endpoint())?;
            let options = |persistent| TerminalOptions {
                size: SIZE,
                persistent,
                resume_id: None,
            };
            assert!(
                matches!(
                    suite.open_terminal(&connection, options(true)),
                    Err(vnilink::Error::Unauthorized)
                ),
                "no shell is kept past its connection"
            );
            let terminal = suite.open_terminal(&connection, options(false))?;
            let shell = terminal.session();
            let mut processes = start_lingering(&terminal);
            processes.push(shell.process_id);
            assert!(
                processes.iter().all(|pid| runs(*pid)),
                "the shell and what it started run"
            );

            // The typed line holds a quote or caret the printed one lacks.
            #[cfg(unix)]
            let (typed, printed) = ("echo 'vnilink-pr''inted'; pwd", "vnilink-printed");
            #[cfg(windows)]
            let (typed, printed) = ("echo vnilink-pr^inted & cd", "vnilink-printed");
            terminal.input(format!("{typed}{ENTER}").as_bytes())?;
            let mut output = Vec::new();
            read_until(
                &terminal,
                &mut output,
                "the shell's answer",
                Duration::from_secs(20),
                |output| contains(output, printed) && contains(output, "vnilink-testkit-terminal-"),
            );
            let text = String::from_utf8_lossy(&output);
            // The folder as the shell printed it: a console wraps it in
            // control sequences and, on Windows, a `>` prompt.
            let folder = text
                .lines()
                .filter(|line| !line.contains(typed))
                .find_map(|line| {
                    let at = line.find("vnilink-testkit-terminal-")?;
                    let bounded = |c: char| c.is_whitespace() || c.is_control() || c == '>';
                    let start = line[..at].rfind(bounded).map_or(0, |at| at + 1);
                    let end = line[at..].find(bounded).map_or(line.len(), |end| at + end);
                    Some(std::path::PathBuf::from(&line[start..end]))
                })
                .expect("the shell names its folder");
            assert!(
                folder.join(OUTPUT_FILE).is_file(),
                "the shell works in the scenario's folder: {}",
                folder.display()
            );
            assert!(
                !folder.starts_with(agent.folder()),
                "never in the agent engine's own folder"
            );

            let records = host.stop()?;
            assert!(
                agent.core.hosted_terminal_sessions()?.is_empty(),
                "no shell outlives the stop"
            );
            assert!(
                processes.iter().all(|pid| !runs(*pid)),
                "neither the shell nor anything it started outlives the stop: {:?}",
                processes
                    .iter()
                    .filter(|pid| runs(**pid))
                    .collect::<Vec<_>>()
            );
            assert!(
                suite
                    .open_terminal(
                        &connection,
                        TerminalOptions {
                            resume_id: Some(shell.id.clone()),
                            ..options(true)
                        }
                    )
                    .is_err(),
                "a stopped shell cannot be resumed"
            );
            assert!(!folder.exists(), "the scenario's folder is removed");
            assert_eq!(
                agent.core.sharing_policy()?,
                SharingPolicy::default(),
                "the agent shares nothing again"
            );
            assert!(
                agent.core.terminal_policy()?.allow_persistence,
                "the agent's own terminal policy is as it was"
            );
            for content in [typed, printed] {
                assert!(
                    records.iter().all(|record| !record.contains(content)),
                    "the agent's report holds no terminal content"
                );
            }
            trace.assert_unrecorded("terminal content", &[typed, printed]);
            suite.core.disconnect_session(connection.id)?;
            Ok(())
        })
    }

    /// The suite takes a polled file only by a rendezvous name: an agent
    /// naming a path elsewhere is answering out of protocol.
    #[test]
    fn a_polled_file_has_a_rendezvous_name() {
        let named = |name: &str| Named {
            name: name.to_owned(),
            data: encode(b"{}"),
        };
        assert!(matches!(
            polled_files(vec![named("host.json")]).as_deref(),
            Ok([(name, _)]) if name == "host.json"
        ));
        for name in [
            "../x.json",
            "/etc/x.json",
            "a/b.json",
            "NUL.json",
            "host.txt",
        ] {
            assert!(
                matches!(polled_files(vec![named(name)]), Err(PeerError::Malformed)),
                "{name} is refused"
            );
        }
    }

    /// A request that gave up waiting does not shift the answers: its late
    /// reply is set aside and the next request gets its own.
    #[test]
    fn a_late_answer_is_not_taken_for_the_next_one() -> TestResult {
        let _trace = traced();
        with_agent(|_suite, host| {
            host.start("camera", json!({ "pictures": 1 }))?;
            let waited = host.ask(
                &Request::Await {
                    what: "sent".to_owned(),
                    within_ms: 300,
                },
                Duration::from_millis(20),
            );
            assert!(
                matches!(waited, Err(PeerError::Engine(vnilink::Error::Timeout))),
                "the suite stops waiting before the agent answers: {waited:?}"
            );
            std::thread::sleep(Duration::from_millis(500));
            assert!(
                matches!(host.ask(&Request::Clock, ANSWER_WAIT)?, Reply::Clock { .. }),
                "the clock request gets the clock, not the late refusal"
            );
            host.stop()?;
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
                &Numbered {
                    id: 1,
                    message: Request::Hello {
                        revision: "another".to_owned(),
                        platform: Platform::this(),
                    },
                },
            )?;
            let Numbered { id, message: reply } =
                receive::<Numbered<Reply>>(&channel, ANSWER_WAIT)?;
            assert_eq!(id, 1, "the refusal answers the Hello");
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
