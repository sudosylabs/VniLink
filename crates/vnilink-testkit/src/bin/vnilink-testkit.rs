//! The test kit's command: `vnilink-testkit agent` runs the host half of
//! suites and measurements on this computer for a suite on another
//! (`node build/run.ts testkit:agent`); `vnilink-testkit bridge` is the
//! suite's half for a build script, which drives it over JSON lines
//! (`vnilink_testkit::peer::bridge`). See `vnilink_testkit::peer`.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use vnilink_testkit::peer::agent::{Agent, Allowance, Ending};
use vnilink_testkit::{Engine, Reach};

const USAGE: &str = "usage: vnilink-testkit agent [--out <file.vnl>] [--ttl <seconds>] [--idle <seconds>] [--loopback] [--allow screen|terminal|files[,...]]
       vnilink-testkit bridge --peer <file.vnl> --peer-check <digits> [--loopback]";

/// How long the pairing file admits a computer when `--ttl` does not say.
const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);
/// How long the agent waits for a request before it exits.
const DEFAULT_IDLE: Duration = Duration::from_secs(30 * 60);

struct Options {
    out: PathBuf,
    ttl: Duration,
    idle: Duration,
    reach: Reach,
    /// What scenarios may do here beyond the agent's engine.
    allowed: Vec<Allowance>,
}

/// Whether the agent serves a suite, whose running scenario a stop request
/// must end before the agent exits.
static SERVING: AtomicBool = AtomicBool::new(false);
/// Set when the agent is asked to stop while it serves.
static STOP: AtomicBool = AtomicBool::new(false);

/// Set once serving ended and the running scenario stopped, which a
/// Windows console closing waits for before the process is ended.
#[cfg(windows)]
static SERVED: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());

/// Says serving ended and the running scenario stopped.
fn served() {
    #[cfg(windows)]
    {
        let (done, ended) = &SERVED;
        *done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        ended.notify_all();
    }
}

/// Ctrl-C, a termination request, its terminal closing (SIGHUP) or
/// Ctrl-\ (SIGQUIT): before a suite is served there is nothing to undo
/// and the agent exits at once; while one is, the running scenario is
/// stopped, removing what it made, and then the agent exits.
#[cfg(unix)]
extern "C" fn interrupted(_signal: libc::c_int) {
    if SERVING.load(Ordering::Acquire) {
        STOP.store(true, Ordering::Release);
    } else {
        // SAFETY: `_exit` is async-signal-safe and ends the process at once.
        unsafe { libc::_exit(130) };
    }
}

#[cfg(unix)]
fn on_interrupt() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
        // SAFETY: `interrupted` only touches atomics or calls `_exit`, both
        // allowed in a signal handler.
        unsafe { libc::signal(signal, interrupted as *const () as libc::sighandler_t) };
    }
}

/// How long a closing console waits for the scenario to stop: Windows
/// ends the process about 5 s after a close, logoff or shutdown.
#[cfg(windows)]
const CLOSE_WAIT: Duration = Duration::from_millis(4_500);

#[cfg(windows)]
unsafe extern "system" fn interrupted(event: u32) -> windows_sys::core::BOOL {
    use windows_sys::Win32::System::Console::{
        CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    };
    if !SERVING.load(Ordering::Acquire) {
        // Not handled: Windows ends the process as it would.
        return 0;
    }
    STOP.store(true, Ordering::Release);
    if matches!(
        event,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT
    ) {
        // Windows ends the process once this returns: the scenario stops first.
        let (done, ended) = &SERVED;
        let done = done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = ended.wait_timeout_while(done, CLOSE_WAIT, |done| !*done);
    }
    1
}

#[cfg(windows)]
fn on_interrupt() {
    // SAFETY: `interrupted` only touches atomics and waits on a condition
    // variable, from the thread Windows runs console handlers on.
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(interrupted), 1) };
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("agent") => {}
        Some("bridge") => return bridge(args),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    on_interrupt();
    // Before any shell starts: what its processes leave behind stays the
    // agent's to end (Linux).
    vnilink_testkit::peer::processes::adopt_orphans();
    match agent(&options) {
        Ok(Ending::Interrupted) => ExitCode::from(130),
        Ok(_) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        out: PathBuf::from("agent.vnl"),
        ttl: DEFAULT_TTL,
        idle: DEFAULT_IDLE,
        reach: Reach::Network,
        allowed: Vec::new(),
    };
    let seconds = |flag: &str, value: Option<String>| -> Result<Duration, String> {
        value
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .ok_or_else(|| format!("{flag} takes a number of seconds"))
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => options.out = args.next().ok_or("--out takes a file")?.into(),
            "--ttl" => options.ttl = seconds("--ttl", args.next())?,
            "--idle" => options.idle = seconds("--idle", args.next())?,
            "--loopback" => options.reach = Reach::Loopback,
            "--allow" => {
                let value = args.next().unwrap_or_default();
                for allowance in Allowance::parse(&value)? {
                    if !options.allowed.contains(&allowance) {
                        options.allowed.push(allowance);
                    }
                }
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(options)
}

fn agent(options: &Options) -> Result<Ending, String> {
    // What the agent records goes back to its suite with each scenario.
    vnilink_log::install();
    if vnilink_log::filter().source != vnilink_log::FilterSource::Environment {
        let _ = vnilink_log::set_filter(Some("warn,vnilink=info"));
    }
    let mut agent = Agent::open(options.reach).map_err(|error| error.to_string())?;
    for allowance in &options.allowed {
        agent.allow(*allowance).map_err(|error| error.to_string())?;
        println!(
            "a scenario that {} may run (--allow {})",
            allowance.effect(),
            allowance.name()
        );
    }
    let file = agent
        .export(options.ttl)
        .map_err(|error| error.to_string())?;
    if let Some(folder) = options
        .out
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        std::fs::create_dir_all(folder)
            .map_err(|error| format!("the pairing file's folder could not be made: {error}"))?;
    }
    std::fs::write(&options.out, &file.bytes)
        .map_err(|error| format!("the pairing file could not be written: {error}"))?;
    println!("pairing file: {}", options.out.display());
    println!(
        "check digits: {} (the file admits one computer for {} s)",
        file.check,
        options.ttl.as_secs()
    );
    println!(
        "on the other computer: --peer <this file> --peer-check {}",
        file.check
    );
    let mut lines = std::io::stdin().lock().lines();
    let controller = agent
        .admit(Instant::now() + options.ttl, |_request| {
            println!("a computer asks to be approved: type the digits its suite printed");
            lines.next()?.ok()
        })
        .map_err(|error| format!("no computer was approved: {error}"))?;
    println!("approved; serving its suite");
    SERVING.store(true, Ordering::Release);
    let ending = agent.serve_until(&controller, options.idle, &STOP);
    served();
    let ending = ending.map_err(|error| error.to_string())?;
    match ending {
        Ending::PeerGone => println!("the suite went away; the agent stops"),
        Ending::Idle => println!(
            "nothing was asked for {} s; the agent stops",
            options.idle.as_secs()
        ),
        Ending::Interrupted => {
            println!("asked to stop; the scenario was stopped and the agent stops")
        }
    }
    Ok(ending)
}

/// `bridge --peer <file> --peer-check <digits>`: pairs with that agent and
/// answers the build script on this process's input and output.
fn bridge(mut args: impl Iterator<Item = String>) -> ExitCode {
    let (mut file, mut check, mut reach) = (None, None, Reach::Network);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--peer" => file = args.next(),
            "--peer-check" => check = args.next(),
            "--loopback" => reach = Reach::Loopback,
            other => {
                eprintln!("unknown argument {other}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(file), Some(check)) = (file, check) else {
        eprintln!("--peer and --peer-check go together\n{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("the pairing file could not be read: {error}");
            return ExitCode::FAILURE;
        }
    };
    let engine = Engine::reachable("controller", reach);
    let stdin = std::io::stdin().lock();
    match vnilink_testkit::peer::bridge::run(
        &engine,
        &bytes,
        check.trim(),
        stdin,
        std::io::stdout().lock(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
