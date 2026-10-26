//! How the Terminal service feels on this computer: how long a typed key
//! takes to come back as its echo, and how fast a busy shell's output
//! arrives.
//!
//! With `--peer` the shell is the test agent's on another computer, which
//! runs it only when its owner allowed that (`--allow terminal`). The keys
//! and the command go to that computer's shell, so they are its platform's.

use std::time::{Duration, Instant};

use serde_json::json;
use vnilink::terminal::TerminalOptions;
use vnilink::RemoteTerminalService;
use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::peer::Remote;
use vnilink_testkit::terminal::{
    output_text, read_until, TerminalSession, OUTPUT_END, OUTPUT_FILE, SIZE,
};

const KEYS: usize = 200;

/// The shell the keys go to: `cmd` on Windows, `sh` elsewhere.
#[derive(Clone, Copy)]
struct Shell {
    windows: bool,
}

impl Shell {
    fn enter(self) -> &'static str {
        if self.windows {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// The bytes of output the busy command prints: Windows consoles print
    /// far slower, and a smaller run still ends in time.
    fn output_bytes(self) -> usize {
        let mib = if self.windows { 8 } else { 32 };
        mib * 1024 * 1024
    }

    /// The command that prints [`Self::output_bytes`] at once, ending with
    /// [`OUTPUT_END`].
    fn busy_command(self) -> String {
        if self.windows {
            // cmd has no quick generator: it types a file of full lines in
            // its folder ([`output_text`]).
            format!("type {OUTPUT_FILE}{}", self.enter())
        } else {
            // The shell echoes the command line: the marker it prints is
            // computed, so only its output ends with it.
            format!(
                "head -c {} /dev/zero | tr '\\0' a; echo; echo VNI_$((6*7))\n",
                self.output_bytes()
            )
        }
    }

    /// What clears the typed line.
    fn clear_line(self) -> &'static str {
        if self.windows {
            "\u{1b}"
        } else {
            "\u{15}"
        }
    }
}

/// Output of `terminal` until `done` holds, as one read.
fn read(
    terminal: &RemoteTerminalService,
    what: &str,
    within: Duration,
    done: impl FnMut(&[u8]) -> bool,
) -> Vec<u8> {
    let mut output = Vec::new();
    read_until(terminal, &mut output, what, within, done);
    output
}

/// Measures key-to-echo and output throughput on `terminal`, whose shell
/// is `shell`.
fn measure(terminal: &RemoteTerminalService, shell: Shell, report: &mut Report) {
    terminal
        .input(format!("echo VNI_READY{}", shell.enter()).as_bytes())
        .unwrap();
    read(terminal, "the shell", Duration::from_secs(10), |output| {
        String::from_utf8_lossy(output).contains("VNI_READY")
    });

    // Each key alone, waiting for its echo before the next, as a person types.
    let mut echoes = Vec::with_capacity(KEYS);
    for _ in 0..KEYS {
        let typed = Instant::now();
        terminal.input(b"x").unwrap();
        read(terminal, "a key's echo", Duration::from_secs(5), |output| {
            output.contains(&b'x')
        });
        echoes.push(typed.elapsed());
    }
    report.record(
        "key to echo p50",
        median(&mut echoes).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );
    report.record(
        "key to echo p95",
        percentile(&mut echoes, 95).as_secs_f64() * 1e3,
        "ms",
        Better::Lower,
    );

    // A command that prints a lot at once.
    terminal.input(shell.clear_line().as_bytes()).unwrap();
    let started = Instant::now();
    terminal.input(shell.busy_command().as_bytes()).unwrap();
    let mut seen = 0usize;
    let output = read(
        terminal,
        "the output to end",
        Duration::from_secs(120),
        |output| {
            seen = output.len();
            // The prompt follows the marker.
            let tail = &output[output.len().saturating_sub(4096)..];
            tail.windows(OUTPUT_END.len())
                .any(|window| window == OUTPUT_END.as_bytes())
        },
    );
    let took = started.elapsed().as_secs_f64();
    assert!(
        output.len() >= shell.output_bytes(),
        "only {seen} bytes arrived"
    );
    report.record(
        "output throughput",
        output.len() as f64 / took / 1e6,
        "MB/s",
        Better::Higher,
    );
}

/// Measures the agent's shell, which it shares only while the scenario runs.
fn measure_peer(remote: &mut Remote, report: &mut Report) {
    let shell = Shell {
        windows: remote.host.platform().os == "windows",
    };
    // The agent writes the file cmd types in its own folder for the shell.
    let output_bytes = if shell.windows {
        shell.output_bytes()
    } else {
        0
    };
    remote
        .host
        .start("terminal", json!({ "output_bytes": output_bytes }))
        .unwrap_or_else(|error| panic!("the terminal scenario: {error}"));
    let endpoint = remote.host.endpoint().to_owned();
    let connection = remote
        .suite
        .connect_services_to(&endpoint)
        .expect("a services connection to the agent");
    let terminal = remote
        .suite
        .open_terminal(
            &connection,
            TerminalOptions {
                size: SIZE,
                persistent: false,
                resume_id: None,
            },
        )
        .expect("the agent's shell");
    measure(&terminal, shell, report);
    terminal.close();
    remote
        .suite
        .core
        .disconnect_session(connection.id)
        .expect("the connection ends");
    remote.host.stop().expect("the scenario ends");
}

fn main() {
    let mut report = Report::new("terminal");
    match Remote::from_env() {
        Some(mut remote) => {
            measure_peer(&mut remote, &mut report);
            report.against(remote.bench_peer());
        }
        None => {
            let session = TerminalSession::open();
            let shell = Shell {
                windows: cfg!(windows),
            };
            if shell.windows {
                // The shell works in the host's folder.
                std::fs::write(
                    session.pair.host.root().join(OUTPUT_FILE),
                    output_text(shell.output_bytes()),
                )
                .unwrap();
            }
            measure(&session.terminal, shell, &mut report);
        }
    }
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
