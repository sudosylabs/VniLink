//! Loopback file-transfer throughput, for comparing builds by hand.
//!
//! Boots two cores on loopback, pairs them with the Transfer grant, and
//! uploads a set of files from one to the other with both background job
//! drivers running, as the application runs them. It reports how long the
//! upload took to be accepted and to complete, with bytes and files per
//! second. Built as an example, not a test, so it measures the shipped
//! configuration rather than test settings.
//!
//! Run: `cargo run --release -p vnilink --features test-probes --example transfer_bench [files] [bytes_per_file]`
//! Defaults: 1,000 files of 4 KiB, then one file of 512 MiB, when no
//! arguments are given.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use vnilink::{Core, Grants, JobState, MemoryStore, RelaySettings};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let runs: Vec<(usize, usize)> = match (args.get(1), args.get(2)) {
        (Some(files), Some(size)) => vec![(
            files.parse().expect("a file count"),
            size.parse().expect("a byte count"),
        )],
        _ => vec![(1000, 4096), (1, 512 * 1024 * 1024)],
    };
    for (files, size) in runs {
        run(files, size);
    }
}

fn run(files: usize, size: usize) {
    let scratch = Scratch::new();
    let host = start(&scratch.0.join("host"));
    let controller = start(&scratch.0.join("controller"));
    pair(&host, &controller);
    host.start_job_driver();
    controller.start_job_driver();

    let source = scratch.0.join("controller/root/set");
    std::fs::create_dir_all(&source).unwrap();
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for n in 0..files {
        let bytes: Vec<u8> = (0..size)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        std::fs::write(source.join(format!("f{n:05}.bin")), bytes).unwrap();
    }
    let _session = controller.connect(&host.endpoint_id()).unwrap();
    let started = Instant::now();
    let job = controller
        .job_submit_upload(&host.endpoint_id(), 0, "set", 0, "")
        .unwrap();
    let accepted = started.elapsed();
    loop {
        let snapshot = controller.job_snapshot(job).unwrap();
        if snapshot.state.is_terminal() {
            assert_eq!(snapshot.state, JobState::Complete, "{:?}", snapshot.error);
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(1200), "too slow");
        std::thread::sleep(Duration::from_millis(2));
    }
    let total = started.elapsed().as_secs_f64();
    println!(
        "{files} x {size} bytes: accepted in {:.0} ms, done in {:.0} ms, {:.1} MB/s, {:.0} files/s",
        accepted.as_secs_f64() * 1e3,
        total * 1e3,
        (files * size) as f64 / total / 1e6,
        files as f64 / total,
    );
    host.shutdown();
    controller.shutdown();
}

fn start(dir: &Path) -> Arc<Core> {
    std::fs::create_dir_all(dir.join("root")).unwrap();
    // Direct loopback only: no relay, so nothing leaves this computer.
    let core =
        Core::open_direct(Arc::new(MemoryStore::empty()), RelaySettings::public_n0()).unwrap();
    // A session needs codecs to open, though this one carries no video.
    core.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "bench"))
        .unwrap();
    core.set_transfer_directory(dir.join("transfers")).unwrap();
    core.set_file_roots(vec![std::fs::canonicalize(dir.join("root")).unwrap()])
        .unwrap();
    Arc::new(core)
}

fn pair(host: &Core, controller: &Core) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    // A session opens under view and control; files move under transfer.
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
    controller
        .set_desktop_sharing(Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("vnilink-transfer-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(std::fs::canonicalize(dir).unwrap())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
