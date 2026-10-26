//! The test agent as a person runs it (`vnilink-testkit agent`): it writes
//! a pairing file and its digits, approves one computer by the digits typed
//! on its terminal, serves that computer's suite, refuses every other
//! computer that opens the same file, and stops on its own when nothing is
//! asked of it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink_testkit::peer::{PeerError, RemoteHost};
use vnilink_testkit::{traced, Engine, Scratch, TestResult};

/// How long the agent waits for a request before it stops, in seconds.
const IDLE: u64 = 10;
/// How long a line the agent prints may take.
const LINE_WAIT: Duration = Duration::from_secs(60);

/// The agent's process, killed if a test fails before it exits.
struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
}

impl AgentProcess {
    fn start(file: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vnilink-testkit"))
            .args(["agent", "--loopback", "--ttl", "60", "--idle"])
            .arg(IDLE.to_string())
            .arg("--out")
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the agent starts");
        let stdin = child.stdin.take().expect("the agent's input");
        let stdout = child.stdout.take().expect("the agent's output");
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
        }
    }

    /// The next line the agent prints that starts with `prefix`.
    fn line(&self, prefix: &str) -> String {
        let deadline = Instant::now() + LINE_WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("the agent did not print {prefix:?}"));
            if line.starts_with(prefix) {
                return line;
            }
        }
    }

    /// Types `text` and Return on the agent's terminal.
    fn type_line(&mut self, text: &str) {
        writeln!(self.stdin, "{text}").expect("the agent reads its terminal");
    }

    /// The agent's exit status, once it exited within `within`.
    fn exited(&mut self, within: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().expect("the agent's status") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the agent did not stop within {within:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for AgentProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The agent writes its file and digits; a suite that confirms them and
/// whose digits the agent's owner types is served; a second computer
/// opening the same file is refused; and the agent stops once nothing is
/// asked of it for its idle limit.
#[test]
fn the_agent_serves_the_one_computer_its_owner_approved_and_stops_when_idle() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("agent");
    let file = scratch.join("agent.vnl");
    let mut agent = AgentProcess::start(&file);
    let check = agent.line("check digits: ")["check digits: ".len()..][..3].to_owned();
    let bytes = std::fs::read(&file)?;

    let suite = Engine::start("controller");
    let mut host = std::thread::scope(|scope| {
        let pairing = scope.spawn(|| RemoteHost::pair(&suite, &bytes, &check));
        agent.line("a computer asks to be approved");
        agent.type_line(&suite.core.control_check());
        pairing.join().expect("the pairing thread")
    })?;
    agent.line("approved");
    let facts = host.start("files", Value::Null)?;
    assert!(
        facts["root"].is_u64() && facts["inbox"].is_string(),
        "the files scenario says where uploads land: {facts}"
    );
    let intruder = Engine::start("intruder");
    assert!(
        matches!(
            RemoteHost::pair(&intruder, &bytes, &check),
            Err(PeerError::Engine(vnilink::Error::Unauthorized))
        ),
        "the file admits no second computer"
    );

    let records = host.stop()?;
    assert!(
        records.iter().all(|record| {
            serde_json::from_str::<Value>(record).is_ok_and(|event| event["engine"] == "agent")
        }),
        "every record the agent reports is its engine's: {records:?}"
    );

    agent.line(&format!("nothing was asked for {IDLE} s"));
    assert!(
        agent.exited(Duration::from_secs(10)).success(),
        "an idle agent stops cleanly"
    );
    drop(host);
    Ok(())
}
