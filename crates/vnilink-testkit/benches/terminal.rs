//! How the Terminal service feels on this computer: how long a typed key
//! takes to come back as its echo, and how fast a busy shell's output
//! arrives.

use std::time::{Duration, Instant};

use vnilink_testkit::bench::{median, percentile, Better, Report};
use vnilink_testkit::terminal::{TerminalSession, ENTER};

const KEYS: usize = 200;

fn main() {
    let mut report = Report::new("terminal");
    let session = TerminalSession::open();
    session.input(&format!("echo VNI_READY{ENTER}")).unwrap();
    session.read_until("the shell", Duration::from_secs(10), |output| {
        String::from_utf8_lossy(output).contains("VNI_READY")
    });

    // Each key alone, waiting for its echo before the next, as a person types.
    let mut echoes = Vec::with_capacity(KEYS);
    for _ in 0..KEYS {
        let typed = Instant::now();
        session.input("x").unwrap();
        session.read_until("a key's echo", Duration::from_secs(5), |output| {
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
    #[cfg(unix)]
    {
        const BYTES: usize = 32 * 1024 * 1024;
        // Clear the typed line first.
        session.input("\u{15}").unwrap();
        let started = Instant::now();
        session
            // The shell echoes the command line: the marker it prints is
            // computed, so only its output ends with it.
            .input(&format!(
                "head -c {BYTES} /dev/zero | tr '\\0' a; echo; echo VNI_$((6*7))\n"
            ))
            .unwrap();
        let mut seen = 0usize;
        let (output, _) =
            session.read_until("the output to end", Duration::from_secs(120), |output| {
                seen = output.len();
                // The prompt follows the marker.
                let tail = &output[output.len().saturating_sub(64)..];
                tail.windows(8).any(|window| window == b"VNI_42\r\n")
            });
        let took = started.elapsed().as_secs_f64();
        assert!(output.len() >= BYTES, "only {seen} bytes arrived");
        report.record(
            "output throughput",
            output.len() as f64 / took / 1e6,
            "MB/s",
            Better::Higher,
        );
    }

    drop(session);
    let regressed = report.finish();
    if !regressed.is_empty() {
        eprintln!("regressed: {}", regressed.join(", "));
        std::process::exit(1);
    }
}
