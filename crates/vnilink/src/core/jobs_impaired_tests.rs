//! Transfer matrix for the impaired-network harness.
//!
//! `npm run measure:impaired` runs `impaired_transfer_matrix` inside a Linux
//! container after it has joined two network namespaces with a `tc netem`
//! veth pair. Each core is booted on a thread that first enters its namespace:
//! the runtime, its workers and the endpoint socket are created there, so all
//! engine traffic crosses the impaired link. The namespaces keep `127.0.0.1`
//! on loopback and route locally sent loopback traffic over the veth, which is
//! why the ordinary direct-mode fixtures work unchanged.
//!
//! Without the namespace variables both cores share this process's network.
//! The small `transfer_matrix_completes_on_loopback` case keeps the scenario
//! itself covered by ordinary test runs.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::RelayMode;
use serde_json::{json, Value};

use crate::core::{Core, CoreConfig, Listen};
use crate::job::JobState;
use crate::manifest;
use crate::relay::{CustomRelay, RelaySettings};
use crate::test_support::{pair_for_transfer, FilesystemLocation};

/// Sizes of one matrix run.
struct Matrix {
    small_files: u64,
    small_bytes: u64,
    large_bytes: u64,
    mixed_files: u64,
    mixed_max_bytes: u64,
    timeout: Duration,
    driver: Driver,
}

#[derive(Clone, Copy, PartialEq)]
enum Driver {
    /// The receiving core calls `drive_jobs` back to back and the sender every
    /// 100 ms: the engine and transport limit without a timer between files.
    Tight,
    /// Only `start_job_driver`, the background driver the app starts.
    Background,
}

impl Driver {
    fn label(self) -> &'static str {
        match self {
            Self::Tight => "tight",
            Self::Background => "background",
        }
    }
}

fn env_u64(name: &str, default: u64) -> u64 {
    match std::env::var(name) {
        Ok(text) => text
            .parse()
            .unwrap_or_else(|_| panic!("{name} must be a whole number, got {text:?}")),
        Err(_) => default,
    }
}

impl Matrix {
    fn from_env() -> Self {
        Self {
            small_files: env_u64("VNILINK_IMPAIRED_SMALL_FILES", 1_000),
            small_bytes: env_u64("VNILINK_IMPAIRED_SMALL_BYTES", 4_096),
            large_bytes: env_u64("VNILINK_IMPAIRED_LARGE_BYTES", 1 << 30),
            mixed_files: env_u64("VNILINK_IMPAIRED_MIXED_FILES", 200),
            mixed_max_bytes: env_u64("VNILINK_IMPAIRED_MIXED_MAX_BYTES", 4 << 20),
            timeout: Duration::from_secs(env_u64("VNILINK_IMPAIRED_TRANSFER_TIMEOUT_S", 300)),
            driver: match std::env::var("VNILINK_IMPAIRED_DRIVER").as_deref() {
                Ok("background") => Driver::Background,
                Ok("tight") | Err(_) => Driver::Tight,
                Ok(other) => {
                    panic!("VNILINK_IMPAIRED_DRIVER must be tight or background, got {other:?}")
                }
            },
        }
    }
}

/// Boots one loopback core, inside `namespace` when one is named.
///
/// Direct-only by default. With `VNILINK_IMPAIRED_RELAY_URL` the core uses
/// that relay alone; the harness serves it and blocks direct UDP.
fn boot_in(namespace: Option<String>) -> Arc<Core> {
    let relay = std::env::var("VNILINK_IMPAIRED_RELAY_URL").ok();
    std::thread::spawn(move || {
        if let Some(name) = namespace {
            enter_namespace(&name);
        }
        let listen = Listen::Loopback;
        let core = match relay {
            Some(url) => Core::start(CoreConfig {
                listen,
                relay: RelaySettings::custom(vec![CustomRelay::new(url).unwrap()]).unwrap(),
                ..CoreConfig::default()
            }),
            None => Core::boot(
                CoreConfig {
                    listen,
                    ..CoreConfig::default()
                },
                RelayMode::Disabled,
            ),
        }
        .expect("loopback core");
        // Admission negotiates video even for a Files-only connection.
        core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .expect("codec capabilities");
        Arc::new(core)
    })
    .join()
    .expect("boot thread")
}

/// The selected transport path of each of `core`'s connections, as its
/// diagnostics report it: `direct`, `relay` or `none`.
fn selected_paths(core: &Core) -> Value {
    let snapshot = core.diagnostic_snapshot().expect("diagnostic snapshot");
    snapshot
        .connections
        .iter()
        .map(|connection| json!({ "role": connection.role, "path": connection.path, "rtt_ms": connection.rtt_ms }))
        .collect()
}

#[cfg(target_os = "linux")]
fn enter_namespace(name: &str) {
    let path = Path::new("/run/netns").join(name);
    let file = std::fs::File::open(&path)
        .unwrap_or_else(|error| panic!("cannot open {}: {error}", path.display()));
    nix::sched::setns(&file, nix::sched::CloneFlags::CLONE_NEWNET)
        .unwrap_or_else(|error| panic!("cannot enter network namespace {name}: {error}"));
}

#[cfg(not(target_os = "linux"))]
fn enter_namespace(name: &str) {
    panic!("network namespace {name} needs Linux");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::var_os("VNILINK_IMPAIRED_SCRATCH")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        // Tests in one process run concurrently: each matrix gets its own tree.
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = base.join(format!(
            "vnilink-impaired-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("root")).expect("scratch directory");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Incompressible, repeatable bytes.
fn write_file(path: &Path, bytes: u64, seed: u64) {
    use std::io::Write;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path).expect("fixture file"));
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut left = bytes;
    let mut block = vec![0u8; 1 << 20];
    while left > 0 {
        let take = left.min(block.len() as u64) as usize;
        for chunk in block[..take].chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
        }
        file.write_all(&block[..take]).expect("fixture write");
        left -= take as u64;
    }
    file.flush().expect("fixture flush");
}

/// Writes the three cases below `root` and returns their directory names.
fn write_cases(root: &Path, matrix: &Matrix) -> Vec<&'static str> {
    let small = root.join("small");
    std::fs::create_dir_all(&small).unwrap();
    for index in 0..matrix.small_files {
        write_file(
            &small.join(format!("f{index:05}.bin")),
            matrix.small_bytes,
            index + 1,
        );
    }
    let large = root.join("large");
    std::fs::create_dir_all(&large).unwrap();
    write_file(&large.join("large.bin"), matrix.large_bytes, 0x1A26E);
    // Log-uniform sizes from 1 KiB to the maximum, up to four levels deep,
    // with a few empty folders.
    let mixed = root.join("mixed");
    for empty in ["empty", "a/empty", "b/c/empty"] {
        std::fs::create_dir_all(mixed.join(empty)).unwrap();
    }
    let min = 1_024f64.ln();
    let span = (matrix.mixed_max_bytes.max(1_024) as f64).ln() - min;
    for index in 0..matrix.mixed_files {
        let fraction = (index.wrapping_mul(0x9E37_79B9) % 1_000) as f64 / 1_000.0;
        let bytes = (min + span * fraction).exp() as u64;
        let folder = match index % 5 {
            0 => mixed.clone(),
            1 => mixed.join("a"),
            2 => mixed.join("a/d"),
            3 => mixed.join("b/c"),
            _ => mixed.join("b/c/e/f"),
        };
        std::fs::create_dir_all(&folder).unwrap();
        write_file(
            &folder.join(format!("m{index:04}.dat")),
            bytes,
            0x313D + index,
        );
    }
    vec!["small", "large", "mixed"]
}

/// Drives until the receiver record ends or `deadline` passes, and returns
/// the receiver's snapshot with the time it was taken, and what QUIC saw on
/// each core's files connections once a second meanwhile. A job still running at
/// the deadline is cancelled on both sides before the drivers are joined: a
/// receiver poll blocks for a whole file, and cancelling is what interrupts it.
fn drive(
    matrix: &Matrix,
    controller: &Arc<Core>,
    host: &Arc<Core>,
    job: u64,
    deadline: Instant,
) -> (crate::job::JobSnapshot, Instant, Vec<Value>) {
    let done = Arc::new(AtomicBool::new(false));
    // The receiver fetches, so it advances back to back. The sender only asks
    // the receiver for its state; spinning it would flood the link with
    // status round trips.
    let workers: Vec<_> = if matrix.driver == Driver::Tight {
        [
            (Arc::clone(host), Duration::ZERO),
            (Arc::clone(controller), Duration::from_millis(100)),
        ]
        .into_iter()
        .map(|(core, pause)| {
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    let idle = core.drive_jobs().map_or(true, |jobs| jobs.is_empty());
                    std::thread::sleep(if idle {
                        pause.max(Duration::from_millis(1))
                    } else {
                        pause
                    });
                }
            })
        })
        .collect()
    } else {
        Vec::new()
    };
    let started = Instant::now();
    let mut files_paths = Vec::new();
    let mut next_sample = started;
    let (snapshot, at) = loop {
        let snapshot = host.job_snapshot(job).expect("receiver record");
        let now = Instant::now();
        if snapshot.state.is_terminal() || now >= deadline {
            break (snapshot, now);
        }
        if now >= next_sample {
            files_paths.push(json!({
                "t_ms": (now - started).as_millis() as u64,
                "done_bytes": snapshot.done_bytes,
                "host": host.files_path_stats(),
                "viewer": controller.files_path_stats(),
            }));
            next_sample += Duration::from_secs(1);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if !snapshot.state.is_terminal() {
        for core in [host, controller] {
            let _ = core.job_cancel(job);
        }
    }
    done.store(true, Ordering::Release);
    for worker in workers {
        worker.join().expect("driver thread");
    }
    (snapshot, at, files_paths)
}

fn run_case(
    matrix: &Matrix,
    controller: &Arc<Core>,
    host: &Arc<Core>,
    from: &FilesystemLocation,
    to: &FilesystemLocation,
    name: &str,
    controller_root: &Path,
    host_root: &Path,
) -> Value {
    let started = Instant::now();
    let submitted = controller.job_submit_upload(
        &host.endpoint_id(),
        from.root,
        &from.path(name),
        to.root,
        &to.path(&format!("arrived-{name}")),
    );
    let submit_ms = started.elapsed().as_millis() as u64;
    let job = match submitted {
        Ok(job) => job,
        Err(error) => {
            return json!({ "case": name, "outcome": "refused", "error": error.to_string(), "submit_ms": submit_ms });
        }
    };
    let (snapshot, ended, files_paths) =
        drive(matrix, controller, host, job, started + matrix.timeout);
    let elapsed = ended - started;
    let outcome = match snapshot.state {
        JobState::Complete => {
            manifest::assert_trees_equal(
                &controller_root.join(name),
                &host_root.join(format!("arrived-{name}/{name}")),
            );
            "complete"
        }
        state if state.is_terminal() => "failed",
        _ => "timed-out",
    };
    for core in [controller, host] {
        let _ = core.job_forget(job);
    }
    let seconds = elapsed.as_secs_f64().max(1e-9);
    json!({
        "case": name,
        "outcome": outcome,
        "state": format!("{:?}", snapshot.state),
        "error": snapshot.error,
        "submit_ms": submit_ms,
        "elapsed_ms": elapsed.as_millis() as u64,
        "total_bytes": snapshot.total_bytes,
        "done_bytes": snapshot.done_bytes,
        "files_total": snapshot.files_total,
        "files_done": snapshot.files_done,
        "mb_per_s": snapshot.done_bytes as f64 / 1e6 / seconds,
        "files_per_s": snapshot.files_done as f64 / seconds,
        "files_paths": files_paths,
    })
}

fn run_matrix(matrix: &Matrix) -> Value {
    let host_scratch = Scratch::new("host");
    let controller_scratch = Scratch::new("controller");
    let host = boot_in(std::env::var("VNILINK_IMPAIRED_HOST_NS").ok());
    let controller = boot_in(std::env::var("VNILINK_IMPAIRED_VIEW_NS").ok());
    for (core, scratch) in [(&host, &host_scratch), (&controller, &controller_scratch)] {
        core.set_transfer_directory(scratch.0.join("transfer"))
            .unwrap();
        core.enable_filesystem_access().unwrap();
    }
    let cases = write_cases(&controller_scratch.0.join("root"), matrix);
    pair_for_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let connected = json!({ "host": selected_paths(&host), "viewer": selected_paths(&controller) });
    if matrix.driver == Driver::Background {
        host.start_job_driver();
        controller.start_job_driver();
    }
    let from = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.0.join("root"),
    );
    let to = FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.0.join("root"));
    let results: Vec<Value> = cases
        .iter()
        .map(|name| {
            run_case(
                matrix,
                &controller,
                &host,
                &from,
                &to,
                name,
                &controller_scratch.0.join("root"),
                &host_scratch.0.join("root"),
            )
        })
        .collect();
    // Files run on their own connection, which the diagnostics do not list;
    // with direct UDP blocked it, too, can only cross the relay.
    let finished = json!({ "host": selected_paths(&host), "viewer": selected_paths(&controller) });
    json!({
        "scenario": "transfer",
        "direction": "upload",
        "driver": matrix.driver.label(),
        "small_files": matrix.small_files,
        "small_bytes": matrix.small_bytes,
        "large_bytes": matrix.large_bytes,
        "mixed_files": matrix.mixed_files,
        "mixed_max_bytes": matrix.mixed_max_bytes,
        "timeout_s": matrix.timeout.as_secs(),
        "cases": results,
        "paths": { "connected": connected, "finished": finished },
    })
}

#[test]
#[ignore = "impaired-network harness: run through `npm run measure:impaired`"]
fn impaired_transfer_matrix() {
    let report = run_matrix(&Matrix::from_env());
    let text = serde_json::to_string_pretty(&report).unwrap();
    match std::env::var_os("VNILINK_IMPAIRED_OUT") {
        Some(path) => std::fs::write(path, text).expect("result file"),
        None => eprintln!("{text}"),
    }
}

#[test]
fn transfer_matrix_completes_on_loopback() {
    let report = run_matrix(&Matrix {
        small_files: 20,
        small_bytes: 4_096,
        large_bytes: 2 << 20,
        mixed_files: 12,
        mixed_max_bytes: 64 << 10,
        timeout: Duration::from_secs(60),
        driver: Driver::Tight,
    });
    let cases = report["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 3);
    for case in cases {
        assert_eq!(case["outcome"], "complete", "{case}");
        assert_eq!(case["done_bytes"], case["total_bytes"]);
        assert!(case["mb_per_s"].as_f64().unwrap() > 0.0);
    }
    assert_eq!(cases[0]["files_total"], 20);
}

#[test]
fn a_case_past_its_timeout_is_cancelled_at_the_deadline() {
    let started = Instant::now();
    // A deadline already past when driving starts: no case can finish
    // before it however fast the machine, so every one must time out.
    let report = run_matrix(&Matrix {
        small_files: 300,
        small_bytes: 4_096,
        large_bytes: 64 << 20,
        mixed_files: 40,
        mixed_max_bytes: 1 << 20,
        timeout: Duration::ZERO,
        driver: Driver::Tight,
    });
    for case in report["cases"].as_array().unwrap() {
        assert_eq!(case["outcome"], "timed-out", "{case}");
        // Submission scans and hashes synchronously, so it passes the
        // deadline itself; the transfer never runs past it by more than a poll.
        let budget = case["submit_ms"].as_u64().unwrap();
        assert!(
            case["elapsed_ms"].as_u64().unwrap() <= budget + 100,
            "{case}"
        );
    }
    // The receiver starts fetching as the drivers start, and a poll blocks
    // for a whole file; cancelling must interrupt it rather than wait for the
    // large file to finish.
    assert!(started.elapsed() < Duration::from_secs(60));
}
