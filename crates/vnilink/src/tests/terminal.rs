use crate::terminal::{HostTerminalFactory, NativeTerminalFactory, TerminalEvent, TerminalSize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
const SIZE: TerminalSize = TerminalSize {
    rows: 24,
    columns: 80,
};
fn wait(mut ready: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while !ready() {
        assert!(Instant::now() < end, "terminal condition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn native() -> Arc<NativeTerminalFactory> {
    Arc::new(NativeTerminalFactory::new("/bin/sh".into(), std::env::temp_dir()).unwrap())
}
#[test]
fn terminal_validates_before_starting_and_close_reaps_shell() {
    assert!(SIZE.validate().is_ok());
    assert!(TerminalSize {
        rows: 0,
        columns: 80
    }
    .validate()
    .is_err());
    assert!(NativeTerminalFactory::new("sh".into(), std::env::temp_dir()).is_err());
    let mut backend = native()
        .open(SIZE, crate::BackendStartup::new(Duration::from_secs(5)))
        .unwrap();
    backend
        .commands
        .send(crate::terminal::TerminalCommand::Input(
            b"echo VNI_PID:$$; sleep 60\n".to_vec(),
        ))
        .unwrap();
    let mut bytes = Vec::new();
    let mut pid = None;
    wait(|| {
        while let Ok(event) = backend.output.try_recv() {
            if let TerminalEvent::Output(data) = event {
                bytes.extend(data);
            }
        }
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            if let Some(value) = line.trim().rsplit_once("VNI_PID:").map(|(_, value)| value) {
                pid = value.parse::<i32>().ok();
            }
        }
        pid.is_some()
    });
    drop(backend);
    wait(|| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid.unwrap()), None).is_err());
}

/// A tty writes each line of `seq` on its own, so a worker that read once per
/// pass delivered a few lines every 5 ms (about 2,000 lines a second). Draining
/// the PTY each pass, with a short wait to fill a chunk, keeps a busy shell at
/// the consumer's pace.
#[test]
fn native_terminal_drains_busy_output_without_waiting_between_reads() {
    const LINES: usize = 200_000;
    let mut backend = native()
        .open(SIZE, crate::BackendStartup::new(Duration::from_secs(5)))
        .unwrap();
    backend
        .commands
        .send(crate::terminal::TerminalCommand::Input(
            format!("seq 1 {LINES}; exit 0\n").into_bytes(),
        ))
        .unwrap();
    let started = Instant::now();
    let mut output = Vec::new();
    let mut chunks = 0usize;
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "seq output did not finish"
        );
        match backend.output.blocking_recv() {
            Some(TerminalEvent::Output(bytes)) => {
                chunks += 1;
                output.extend(bytes);
            }
            Some(TerminalEvent::Exited(code)) => {
                assert_eq!(code, 0);
                break;
            }
            Some(event) => panic!("unexpected terminal event {event:?}"),
            None => panic!("terminal output ended without an exit"),
        }
    }
    let elapsed = started.elapsed();
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains(&format!("\n{LINES}\r\n")),
        "seq output is incomplete"
    );
    let rate = LINES as f64 / elapsed.as_secs_f64();
    eprintln!(
        "native terminal: {LINES} lines, {} bytes in {chunks} chunks, {elapsed:.2?} ({rate:.0} lines/s)",
        text.len()
    );
    // Reading once per 5 ms pass did not finish 200,000 lines in minutes; this
    // Mac does about 87,000 a second and a shared CI runner about 13,000.
    assert!(
        rate > 5_000.0,
        "busy terminal output is throttled: {rate:.0} lines/s"
    );
    // Each chunk crosses the network and the viewer's acknowledgement on its
    // own, so busy output arrives in chunks, not line by line.
    assert!(
        chunks < LINES / 10,
        "busy terminal output was not gathered: {chunks} chunks"
    );
}
