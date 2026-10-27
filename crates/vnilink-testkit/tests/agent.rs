//! The test agent as a person runs it (`vnilink-testkit agent`): it writes
//! a pairing file and its digits, approves one computer by the digits typed
//! on its terminal, serves that computer's suite, refuses every other
//! computer that opens the same file, and stops on its own when nothing is
//! asked of it. A build script drives it through `vnilink-testkit bridge`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;
use vnilink_testkit::peer::processes::runs;
use vnilink_testkit::peer::{PeerError, RemoteHost};
use vnilink_testkit::{terminal, traced, Engine, Scratch, TestResult};

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
    /// The agent writing its pairing file to `file`, with `args` added to
    /// its command line and `env` to its environment.
    fn start_with(file: &std::path::Path, args: &[&str], env: &[(&str, &std::path::Path)]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vnilink-testkit"))
            .args(["agent", "--loopback", "--ttl", "60", "--idle"])
            .arg(IDLE.to_string())
            .arg("--out")
            .arg(file)
            .args(args)
            .envs(env.iter().copied())
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
/// asked of it for its idle limit. Its owner allowed the files scenario,
/// and the agent says when that scenario shares this computer's disk and
/// when it no longer does.
#[test]
fn the_agent_serves_the_one_computer_its_owner_approved_and_stops_when_idle() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("agent");
    let file = scratch.join("agent.vnl");
    let mut agent = AgentProcess::start_with(&file, &["--allow", "files"], &[]);
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
    agent.line("the paired computer may now read and change this computer's whole disk");
    let intruder = Engine::start("intruder");
    assert!(
        matches!(
            RemoteHost::pair(&intruder, &bytes, &check),
            Err(PeerError::Engine(vnilink::Error::Unauthorized))
        ),
        "the file admits no second computer"
    );

    let records = host.stop()?;
    agent.line("the paired computer no longer has this computer's disk");
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

/// An agent started with `args`, and a suite in this process paired with
/// it as a person pairs them.
fn paired(
    scratch: &Scratch,
    args: &[&str],
) -> Result<(AgentProcess, Engine, RemoteHost), PeerError> {
    let file = scratch.join("agent.vnl");
    let mut agent = AgentProcess::start_with(&file, args, &[]);
    let check = agent.line("check digits: ")["check digits: ".len()..][..3].to_owned();
    let bytes = std::fs::read(&file).map_err(PeerError::File)?;
    let suite = Engine::start("controller");
    let host = std::thread::scope(|scope| {
        let pairing = scope.spawn(|| RemoteHost::pair(&suite, &bytes, &check));
        agent.line("a computer asks to be approved");
        agent.type_line(&suite.core.control_check());
        pairing.join().expect("the pairing thread")
    })?;
    agent.line("approved");
    Ok((agent, suite, host))
}

/// A shell the terminal scenario shares, opened from the suite: the folder
/// the agent named for it, and the ids of the shell and of what it started
/// that a terminal's own ending leaves running.
struct SharedShell {
    folder: std::path::PathBuf,
    processes: Vec<u32>,
    _terminal: vnilink::RemoteTerminalService,
}

impl SharedShell {
    fn open(
        agent: &AgentProcess,
        suite: &Engine,
        host: &mut RemoteHost,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        host.start("terminal", Value::Null)?;
        let opened = agent.line("the paired computer now has a shell on this computer");
        let folder = std::path::PathBuf::from(
            opened
                .split_once("working in ")
                .ok_or("the agent names the shell's folder")?
                .1,
        );
        assert!(folder.is_dir(), "the shell's folder exists while it runs");
        let connection = suite.connect_services_to(host.endpoint())?;
        let terminal = suite.open_terminal(
            &connection,
            vnilink::terminal::TerminalOptions {
                size: terminal::SIZE,
                persistent: false,
                resume_id: None,
            },
        )?;
        let mut processes = terminal::start_lingering(&terminal);
        processes.push(terminal.session().process_id);
        assert!(
            processes.iter().all(|pid| runs(*pid)),
            "the shell and what it started run"
        );
        Ok(Self {
            folder,
            processes,
            _terminal: terminal,
        })
    }

    /// Asserts the agent said the shell ended and its folder went, and
    /// that nothing of it is left.
    fn assert_ended(&self, agent: &AgentProcess) {
        let ended = agent.line("the paired computer's shell here");
        assert!(
            ended.contains("has ended") && ended.contains(&self.folder.display().to_string()),
            "the agent says the shell ended and which folder went: {ended}"
        );
        assert!(!self.folder.exists(), "the shell's folder is removed");
        let running: Vec<_> = self.processes.iter().filter(|pid| runs(**pid)).collect();
        assert!(
            running.is_empty(),
            "neither the shell nor anything it started is left: {running:?}"
        );
    }
}

/// The terminal scenario runs only when the agent's owner allowed it
/// apart from the screen; the agent then tells its owner the paired
/// computer has a shell here and in which folder, and once the scenario
/// stops, that the shell and everything it started ended and the folder
/// is gone.
#[test]
fn the_agent_shares_a_shell_only_when_its_owner_allowed_it_and_says_so() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("terminal-refused");
    let (_agent, _suite, mut host) = paired(&scratch, &["--allow", "screen"])?;
    assert!(
        matches!(
            host.start("terminal", Value::Null),
            Err(PeerError::Refused(reason)) if reason.contains("--allow terminal")
        ),
        "the refusal names the flag that allows a shell"
    );
    drop(host);

    let scratch = Scratch::new("terminal-allowed");
    let (mut agent, suite, mut host) =
        paired(&scratch, &["--allow", "screen", "--allow", "terminal"])?;
    let shell = SharedShell::open(&agent, &suite, &mut host)?;
    host.stop()?;
    shell.assert_ended(&agent);
    drop(host);
    agent.line("the suite went away");
    assert!(
        agent.exited(Duration::from_secs(10)).success(),
        "the agent stops cleanly"
    );
    Ok(())
}

/// A shared shell ends with the scenario however the agent's service ends
/// without a stop: nothing asked for its idle limit, or the suite gone.
#[test]
fn a_shared_shell_ends_when_the_agent_idles_or_its_suite_goes() -> TestResult {
    let _trace = traced();
    for (tag, ending) in [
        ("idle", "nothing was asked for"),
        ("gone", "the suite went away"),
    ] {
        let scratch = Scratch::new(tag);
        let (mut agent, suite, mut host) = paired(&scratch, &["--allow", "terminal"])?;
        let shell = SharedShell::open(&agent, &suite, &mut host)?;
        // Gone: the suite closes its channel. Idle: it stays, asking nothing.
        let kept = (tag == "idle").then_some(host);
        shell.assert_ended(&agent);
        agent.line(ending);
        assert!(
            agent.exited(Duration::from_secs(10)).success(),
            "the agent stops cleanly after {tag}"
        );
        drop(kept);
    }
    Ok(())
}

/// Interrupting the agent (Ctrl-C), or its terminal closing (SIGHUP),
/// while it shares a shell ends the shell and everything it started, and
/// removes its folder, before the agent exits.
#[cfg(unix)]
#[test]
fn a_shared_shell_ends_when_the_agent_is_interrupted_or_its_terminal_closes() -> TestResult {
    let _trace = traced();
    for signal in ["INT", "HUP"] {
        let scratch = Scratch::new(&format!("terminal-{signal}"));
        let (mut agent, suite, mut host) = paired(&scratch, &["--allow", "terminal"])?;
        let shell = SharedShell::open(&agent, &suite, &mut host)?;
        let sent = Command::new("kill")
            .arg(format!("-{signal}"))
            .arg(agent.child.id().to_string())
            .status()?;
        assert!(sent.success(), "the agent was sent SIG{signal}");
        shell.assert_ended(&agent);
        agent.line("asked to stop");
        assert_eq!(
            agent.exited(Duration::from_secs(60)).code(),
            Some(130),
            "the agent exits as interrupted by SIG{signal}"
        );
        drop(host);
    }
    Ok(())
}

/// An agent that allows screen scenarios and runs [`FAKE_SIDE`] as its
/// Node, and a bridge paired with it as a person pairs them.
#[cfg(unix)]
fn paired_with_fake_side(
    scratch: &Scratch,
) -> Result<(AgentProcess, BridgeProcess), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let side = scratch.join("node");
    std::fs::write(&side, FAKE_SIDE)?;
    std::fs::set_permissions(&side, std::fs::Permissions::from_mode(0o755))?;
    let file = scratch.join("agent.vnl");
    let mut agent = AgentProcess::start_with(
        &file,
        &["--allow", "screen"],
        &[("VNILINK_AGENT_NODE", side.as_path())],
    );
    let check = agent.line("check digits: ")["check digits: ".len()..][..3].to_owned();
    let bridge = BridgeProcess::start(&file, &check);
    let approve = bridge.next();
    assert_eq!(
        approve["event"], "approve",
        "the bridge says what to type: {approve}"
    );
    agent.line("a computer asks to be approved");
    agent.type_line(approve["digits"].as_str().expect("digits"));
    let ready = bridge.next();
    assert_eq!(ready["event"], "ready", "paired: {ready}");
    assert_eq!(ready["platform"]["os"], std::env::consts::OS);
    Ok((agent, bridge))
}

/// Interrupting the agent while it serves stops the running scenario,
/// ending everything its side started, before the agent exits.
#[cfg(unix)]
#[test]
fn an_interrupted_agent_stops_its_scenario_first() -> TestResult {
    let _trace = traced();
    let scratch = Scratch::new("interrupt");
    let (mut agent, mut bridge) = paired_with_fake_side(&scratch)?;
    let started = bridge.ask(serde_json::json!({ "op": "start", "scenario": "latency-viewer" }));
    assert_eq!(started["ok"], true, "{started}");
    bridge.polled("viewer.json");
    let evidence = scratch.join("evidence");
    bridge.ask(serde_json::json!({ "op": "fetch", "into": evidence }));
    let pid = std::fs::read_to_string(evidence.join("started.pid"))?;
    let alive = || {
        Command::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(alive(), "the side's own process runs");
    let interrupted = Command::new("kill")
        .args(["-INT", &agent.child.id().to_string()])
        .status()?;
    assert!(interrupted.success(), "the agent was sent SIGINT");
    agent.line("asked to stop");
    assert_eq!(
        agent.exited(Duration::from_secs(60)).code(),
        Some(130),
        "the agent exits as interrupted"
    );
    assert!(!alive(), "the scenario stopped before the agent exited");
    Ok(())
}

/// Stands in for the agent's Node in place of `node build/run.ts
/// testkit:launch latency-viewer`: a viewer side that publishes, waits for
/// the host's file, records what it was started with under its output
/// folder, and quits when its input closes, leaving behind a process it
/// started in a group of its own, as a launched Electron runs.
#[cfg(unix)]
const FAKE_SIDE: &str = r#"#!/bin/bash
set -m
sleep 300 &
echo $! > "$VNILINK_LATENCY_OUTPUT/started.pid"
publish() { printf '%s' "$2" > "$VNILINK_LATENCY_DIR/$1.partial" && mv "$VNILINK_LATENCY_DIR/$1.partial" "$VNILINK_LATENCY_DIR/$1"; }
printf '%s %s %s %s %s\n' "$2" "$3" "$VNILINK_LATENCY_ROLE" "$VNILINK_LATENCY_FPS" "$VNILINK_LATENCY_ACROSS" > "$VNILINK_LATENCY_OUTPUT/side.log"
publish viewer.json '{"token":"viewer"}'
while [ ! -f "$VNILINK_LATENCY_DIR/host.json" ]; do sleep 0.05; done
mkdir -p "$VNILINK_LATENCY_OUTPUT/logs"
cp "$VNILINK_LATENCY_DIR/host.json" "$VNILINK_LATENCY_OUTPUT/logs/host-seen.json"
publish viewer-streaming.json '{}'
cat > /dev/null
"#;

/// The bridge's process, killed if a test fails before it exits.
#[cfg(unix)]
struct BridgeProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

#[cfg(unix)]
impl BridgeProcess {
    fn start(file: &std::path::Path, check: &str) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_vnilink-testkit"))
            .args(["bridge", "--loopback", "--peer-check", check, "--peer"])
            .arg(file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the bridge starts");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("the bridge's output");
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

    /// The next line the bridge prints, as JSON.
    fn next(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(LINE_WAIT)
            .expect("the bridge answers");
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("the bridge prints JSON: {line}"))
    }

    /// Sends `command` and returns its answer.
    fn ask(&mut self, command: Value) -> Value {
        let stdin = self.stdin.as_mut().expect("the bridge's input");
        writeln!(stdin, "{command}").expect("the bridge reads its input");
        self.next()
    }

    /// Polls until the scenario's side published `name`, returning its bytes.
    fn polled(&mut self, name: &str) -> Vec<u8> {
        let deadline = Instant::now() + LINE_WAIT;
        loop {
            let answer = self.ask(serde_json::json!({ "op": "poll" }));
            assert_eq!(answer["ok"], true, "a running side is polled: {answer}");
            if let Some(file) = answer["files"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|file| file["name"] == name)
            {
                return vnilink_testkit::peer::decode(file["data"].as_str().unwrap_or_default())
                    .expect("base64");
            }
            assert!(Instant::now() < deadline, "the side never published {name}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(unix)]
impl Drop for BridgeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A build script drives the agent through the bridge as a measurement
/// does: it starts a side with whitelisted variables only, relays the two
/// sides' rendezvous files, fetches the side's evidence and stops it, and
/// the bridge ends when its input closes.
#[cfg(unix)]
#[test]
fn a_build_script_drives_a_launched_side_through_the_bridge() -> TestResult {
    use vnilink_testkit::peer::encode;

    let _trace = traced();
    let scratch = Scratch::new("bridge");
    let (agent, mut bridge) = paired_with_fake_side(&scratch)?;

    let across = bridge.ask(serde_json::json!({
        "op": "start", "scenario": "latency-viewer", "params": { "env": { "VNILINK_LATENCY_ACROSS": "0" } }
    }));
    assert_eq!(
        across["ok"], false,
        "that a side runs across computers is not the suite's to say: {across}"
    );
    let sneaky = bridge.ask(serde_json::json!({
        "op": "start", "scenario": "latency-viewer", "params": { "env": { "PATH": "/tmp" } }
    }));
    assert_eq!(sneaky["ok"], false, "only whitelisted variables: {sneaky}");
    let started = bridge.ask(serde_json::json!({
        "op": "start", "scenario": "latency-viewer", "params": { "env": { "VNILINK_LATENCY_FPS": "60" } }
    }));
    assert_eq!(started["ok"], true, "the side starts: {started}");
    assert_eq!(bridge.polled("viewer.json"), br#"{"token":"viewer"}"#);
    let escape = bridge
        .ask(serde_json::json!({ "op": "put", "name": "../host.json", "data": encode(b"{}") }));
    assert_eq!(
        escape["ok"], false,
        "a file goes only into the rendezvous folder: {escape}"
    );
    let program = bridge.ask(serde_json::json!({ "op": "put", "name": "read.json", "data": encode(br#"{"seconds":20,"reader":"/bin/sh"}"#) }));
    assert_eq!(
        program["ok"], false,
        "a read naming a program is refused: {program}"
    );
    let unasked = bridge
        .ask(serde_json::json!({ "op": "put", "name": "viewer.json", "data": encode(b"{}") }));
    assert_eq!(
        unasked["ok"], false,
        "a viewer takes only what a host publishes: {unasked}"
    );
    let put = bridge.ask(serde_json::json!({ "op": "put", "name": "host.json", "data": encode(br#"{"token":"host","endpoint":"e"}"#) }));
    assert_eq!(put["ok"], true, "{put}");
    bridge.polled("viewer-streaming.json");

    let evidence = scratch.join("evidence");
    let fetched = bridge.ask(serde_json::json!({ "op": "fetch", "into": evidence }));
    assert_eq!(
        fetched["files"],
        serde_json::json!([
            "agent-launch.log",
            "logs/host-seen.json",
            "side.log",
            "started.pid"
        ]),
        "every file of the side's output: {fetched}"
    );
    assert_eq!(
        std::fs::read_to_string(evidence.join("side.log"))?,
        "testkit:launch latency-viewer viewer 60 1\n",
        "the agent's own checkout task, its role, the suite's frame rate, and across set by the agent"
    );
    assert_eq!(
        std::fs::read(evidence.join("logs/host-seen.json"))?,
        br#"{"token":"host","endpoint":"e"}"#,
        "the side read what was put"
    );

    let started: u32 = std::fs::read_to_string(evidence.join("started.pid"))?
        .trim()
        .parse()?;
    // Running, not merely waiting to be reaped.
    let alive = || runs(started);
    assert!(alive(), "the side's own process runs");
    let stopped = bridge.ask(serde_json::json!({ "op": "stop" }));
    assert_eq!(
        stopped["ok"], true,
        "the side stops when its input closes: {stopped}"
    );
    assert!(!alive(), "nothing the side started outlives the stop");
    let after = bridge.ask(serde_json::json!({ "op": "poll" }));
    assert_eq!(after["ok"], false, "nothing runs after stop: {after}");

    drop(bridge.stdin.take());
    let deadline = Instant::now() + LINE_WAIT;
    let status = loop {
        if let Some(status) = bridge.child.try_wait()? {
            break status;
        }
        assert!(Instant::now() < deadline, "the bridge ends with its input");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the bridge ends cleanly");
    agent.line("the suite went away");
    Ok(())
}
