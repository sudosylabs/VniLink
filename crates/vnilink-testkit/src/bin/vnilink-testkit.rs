//! The test kit's command: `vnilink-testkit agent` runs the host half of
//! suites and measurements on this computer for a suite on another
//! (`node build/run.ts testkit:agent`); see `vnilink_testkit::peer`.

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use vnilink_testkit::peer::agent::{Agent, Ending};
use vnilink_testkit::Reach;

const USAGE: &str = "usage: vnilink-testkit agent [--out <file.vnl>] [--ttl <seconds>] [--idle <seconds>] [--loopback]";

/// How long the pairing file admits a computer when `--ttl` does not say.
const DEFAULT_TTL: Duration = Duration::from_secs(10 * 60);
/// How long the agent waits for a request before it exits.
const DEFAULT_IDLE: Duration = Duration::from_secs(30 * 60);

struct Options {
    out: PathBuf,
    ttl: Duration,
    idle: Duration,
    reach: Reach,
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("agent") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let options = match parse(args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match agent(&options) {
        Ok(()) => ExitCode::SUCCESS,
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
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(options)
}

fn agent(options: &Options) -> Result<(), String> {
    // What the agent records goes back to its suite with each scenario.
    vnilink_log::install();
    if vnilink_log::filter().source != vnilink_log::FilterSource::Environment {
        let _ = vnilink_log::set_filter(Some("warn,vnilink=info"));
    }
    let agent = Agent::open(options.reach).map_err(|error| error.to_string())?;
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
    match agent
        .serve(&controller, options.idle)
        .map_err(|error| error.to_string())?
    {
        Ending::PeerGone => println!("the suite went away; the agent stops"),
        Ending::Idle => println!(
            "nothing was asked for {} s; the agent stops",
            options.idle.as_secs()
        ),
    }
    Ok(())
}
