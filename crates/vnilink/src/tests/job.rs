//! Stage C: transfer job proofs.
//!
//! Tree comparisons are exact (paths, types, sizes, sha256, empty dirs,
//! no extras). Interruption and restart are deterministic: job polls
//! advance one file, kills land between polls or mid-fetch through the
//! cancel watch, never through sleeps.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use iroh::RelayMode;

use super::filesystem_location::FilesystemLocation;
use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::job::JobState;
use crate::manifest;

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    // These correctness fixtures may live on a small /dev/shm volume. Resource
    // admission has separate exact-boundary tests; don't require production
    // free-space headroom for these small temporary fixtures.
    core.set_file_resource_policy(crate::job::FileResourcePolicy {
        minimum_free_bytes: 0,
        ..Default::default()
    })
    .unwrap();
    core
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn tagged(tag: &str) -> Self {
        Self::tagged_in(tag, std::env::temp_dir())
    }

    fn tagged_in(tag: &str, base: std::path::PathBuf) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = base.join(format!(
            "vnilink-job-{}-{}-{}",
            tag,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("scratch directory");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Serializes the multi-megabyte job tests. Each `Core` brings its own
/// multi-threaded runtimes; running several bulk
/// transfers at once oversubscribes the machine and turns timing bounds
/// into noise. Light tests stay parallel.
static HEAVY_JOB: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut state: u64 = seed;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let word = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn pair_with_transfer(host: &Core, controller: &Core) {
    host.start_hosting().unwrap();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&token).unwrap();
    let back = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&back).unwrap();
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
}

fn setup(tag: &str) -> (Core, Core, Scratch, Scratch) {
    setup_with_access(tag, |core, _| core.enable_filesystem_access().unwrap())
}

#[cfg(unix)]
fn setup_confined(tag: &str) -> (Core, Core, Scratch, Scratch) {
    setup_with_access(tag, |core, root| {
        core.set_file_roots(vec![root.to_path_buf()]).unwrap()
    })
}

fn setup_with_access(
    tag: &str,
    configure: impl Fn(&Core, &Path),
) -> (Core, Core, Scratch, Scratch) {
    let host = start_direct();
    let controller = start_direct();
    let host_scratch = Scratch::tagged(&format!("{tag}-host"));
    let controller_scratch = Scratch::tagged(&format!("{tag}-controller"));
    for (core, scratch) in [(&host, &host_scratch), (&controller, &controller_scratch)] {
        std::fs::create_dir_all(scratch.join("root")).unwrap();
        core.set_transfer_directory(scratch.join("blobs")).unwrap();
        configure(core, &scratch.join("root"));
    }
    pair_with_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    (host, controller, host_scratch, controller_scratch)
}

/// Drives both sides until the sender and receiver records terminate.
/// Returns the final snapshots.
fn drive_to_end(
    sender: &Core,
    sender_id: u64,
    receiver: &Core,
    receiver_id: u64,
) -> (crate::job::JobSnapshot, crate::job::JobSnapshot) {
    // Bounded by time, not by polls: a poll made while another caller
    // drives the same job returns without advancing it, so on a loaded
    // machine many polls can pass before the transfer moves.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last_sender = sender.job_snapshot(sender_id).unwrap();
    let mut last_receiver = receiver.job_snapshot(receiver_id).unwrap();
    while std::time::Instant::now() < deadline {
        let before = (last_sender.clone(), last_receiver.clone());
        if !last_receiver.state.is_terminal() {
            last_receiver = receiver.job_poll(receiver_id).unwrap();
        }
        if !last_sender.state.is_terminal() {
            last_sender = sender.job_poll(sender_id).unwrap();
        }
        if last_receiver.state.is_terminal() && last_sender.state.is_terminal() {
            break;
        }
        if last_receiver.state == JobState::WaitingDecision {
            break;
        }
        if (&last_sender, &last_receiver) == (&before.0, &before.1) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    (last_sender, last_receiver)
}

fn receiver_id(peer: &Core, sender_id: u64) -> u64 {
    // Initiators name the id; receivers adopt it.
    let _ = sender_id;
    let jobs = peer.job_list().unwrap();
    assert_eq!(jobs.len(), 1);
    jobs[0].id
}

/// A destination on another volume (a root on a second drive): the rename
/// fails with CrossesDevices, so the bytes go through a synced sibling and a
/// same-volume rename. Windows publishes this way; renames cannot cross drives.
#[test]
fn publication_across_volumes_copies_through_a_sibling() {
    use crate::manifest::{publish_file_with, Commits, PublishPolicy};
    use std::path::Path;
    let scratch = Scratch::tagged("cross-volume");
    let dest = scratch.join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let staged = scratch.join("staged.bin");
    let leftovers = || {
        std::fs::read_dir(&dest)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with(".vnlpub."))
            .count()
    };
    let crossing = |rename: fn(&Path, &Path) -> std::io::Result<()>| {
        let staged = staged.clone();
        move |from: &Path, to: &Path| {
            if from == staged {
                Err(std::io::Error::from(std::io::ErrorKind::CrossesDevices))
            } else {
                rename(from, to)
            }
        }
    };
    let replace = crossing(|from, to| std::fs::rename(from, to));
    let no_replace = crossing(|from, to| vnilink_rename::rename_noreplace(from, to));
    let commits = Commits {
        replace: &replace,
        no_replace: &no_replace,
    };

    std::fs::write(&staged, b"first").unwrap();
    let landed =
        publish_file_with(&dest, "a.bin", &staged, PublishPolicy::Replace, &commits).unwrap();
    assert_eq!(landed, "a.bin");
    assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
    assert!(!staged.exists(), "the staged copy outlived publication");

    std::fs::write(&staged, b"second").unwrap();
    let kept =
        publish_file_with(&dest, "a.bin", &staged, PublishPolicy::KeepBoth, &commits).unwrap();
    assert_ne!(kept, "a.bin");
    assert_eq!(std::fs::read(dest.join(&kept)).unwrap(), b"second");
    assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
    assert_eq!(leftovers(), 0, "a sibling was left behind");

    // A failed final rename removes the sibling and keeps the staged bytes.
    std::fs::write(&staged, b"third").unwrap();
    let failing = |from: &Path, _: &Path| -> std::io::Result<()> {
        Err(std::io::Error::from(if from == staged.as_path() {
            std::io::ErrorKind::CrossesDevices
        } else {
            std::io::ErrorKind::PermissionDenied
        }))
    };
    let broken = Commits {
        replace: &failing,
        no_replace: &failing,
    };
    assert!(publish_file_with(&dest, "a.bin", &staged, PublishPolicy::Replace, &broken).is_err());
    assert_eq!(std::fs::read(&staged).unwrap(), b"third");
    assert_eq!(leftovers(), 0, "a failed sibling was left behind");
    assert_eq!(std::fs::read(dest.join("a.bin")).unwrap(), b"first");
}

#[test]
fn manifest_validation_rejects_bad_shapes() {
    use crate::manifest::{Manifest, ManifestEntry};

    let file = |path: &str| ManifestEntry {
        path: path.to_owned(),
        is_dir: false,
        size: 10,
        source_mtime_ms: 0,
    };
    let dir = |path: &str| ManifestEntry {
        path: path.to_owned(),
        is_dir: true,
        size: 0,
        source_mtime_ms: 0,
    };
    let valid = Manifest {
        entries: vec![dir("sub"), file("a.txt"), file("sub/b.txt")],
    };
    manifest::validate(&valid).unwrap();

    for entries in [
        vec![],
        vec![file("a.txt"), file("a.txt")],
        vec![file("A.txt"), file("a.txt")],
        vec![file("../out.txt")],
        vec![file("/abs.txt")],
        vec![file("a\\b.txt")],
        vec![file("a/./b.txt")],
        vec![dir("d"), file("d")],
        vec![dir("bad")]
            .into_iter()
            .map(|mut entry| {
                entry.size = 1;
                entry
            })
            .collect(),
    ] {
        assert!(
            manifest::validate(&Manifest { entries }).is_err(),
            "bad manifest shape must be rejected"
        );
    }
}

/// Crashes the `job_message` fuzz target: sizes summing past `u64::MAX`
/// validated, then overflowed the job's total.
#[test]
fn manifest_validation_rejects_sizes_past_a_byte_count() {
    use crate::manifest::{Manifest, ManifestEntry};

    let file = |path: &str, size| ManifestEntry {
        path: path.to_owned(),
        is_dir: false,
        size,
        source_mtime_ms: 0,
    };
    let largest = Manifest {
        entries: vec![file("a", u64::MAX - 1), file("b", 1)],
    };
    manifest::validate(&largest).unwrap();
    assert_eq!(largest.total_bytes(), u64::MAX);
    let past = Manifest {
        entries: vec![file("a", u64::MAX), file("b", 1)],
    };
    assert!(manifest::validate(&past).is_err());
}

/// A partial resumes after its last journaled segment, without the sender
/// reading that segment again; bytes of the next segment that arrived,
/// with or without a journal, are kept and checked with the rest of their
/// segment. Either way the file lands exactly.
#[test]
fn a_partial_resumes_after_its_journaled_segments() {
    use crate::transfer::{SegmentHasher, SEGMENT_LEN};
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup("partial");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let segment = SEGMENT_LEN as usize;
    let journaled = payload(segment + 1024 * 1024 + 11, 21);
    let unjournaled = payload(2 * 1024 * 1024 + 5, 22);
    std::fs::write(host_scratch.join("root/journaled.bin"), &journaled).unwrap();
    std::fs::write(host_scratch.join("root/unjournaled.bin"), &unjournaled).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    controller
        .create_dir(controller_files.root, &controller_files.path(""), "got")
        .unwrap();
    let job = controller
        .job_submit_download(
            &host.endpoint_id(),
            host_files.root,
            &host_files.path(""),
            controller_files.root,
            &controller_files.path("got"),
        )
        .unwrap();
    // What an interrupted attempt left: one checked segment and part of the
    // next, and bytes of another file with no journal at all.
    let staging = controller_scratch.join(&format!("blobs/staging/job-{job}"));
    std::fs::create_dir_all(staging.join("files/root")).unwrap();
    std::fs::create_dir_all(staging.join("segments/root")).unwrap();
    std::fs::write(
        staging.join("files/root/journaled.bin"),
        &journaled[..segment + 4096],
    )
    .unwrap();
    let mut first = SegmentHasher::at(0);
    first.update(&journaled[..segment]);
    std::fs::write(
        staging.join("segments/root/journaled.bin"),
        first.finalize(),
    )
    .unwrap();
    std::fs::write(
        staging.join("files/root/unjournaled.bin"),
        &unjournaled[..1024 * 1024],
    )
    .unwrap();

    let host_job = host.job_list().unwrap()[0].id;
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(
        receiver.resumed_files, 2,
        "both partials resume from the bytes they hold"
    );
    manifest::assert_trees_equal(
        &host_scratch.join("root"),
        &controller_scratch.join("root/got/root"),
    );
    assert!(!staging.exists());
}

#[test]
fn scan_accepts_single_files_as_well_as_trees() {
    let scratch = Scratch::tagged("scan-file");
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/only.txt"), b"solo-bytes").unwrap();
    let mut roots = crate::fs::RootSet::default();
    roots.set_filesystem().unwrap();
    let files = FilesystemLocation::new(roots.snapshot(), &root);

    // A whole tree still scans.
    let tree = crate::manifest::scan(&roots, files.root, &files.path("")).unwrap();
    assert!(tree.files.iter().any(|f| f.rel == "sub/only.txt"));

    // A single file scans to a one-file manifest addressed by basename,
    // so selections of individual files transfer too.
    let single = crate::manifest::scan(&roots, files.root, &files.path("sub/only.txt")).unwrap();
    assert_eq!(single.files.len(), 1);
    assert_eq!(single.files[0].rel, "only.txt");
    assert_eq!(single.files[0].size, 10);
    assert!(single.dirs.is_empty());

    // A symlink to a file is refused, never followed.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("sub/only.txt"), root.join("sub/link.txt")).unwrap();
        assert!(crate::manifest::scan(&roots, files.root, &files.path("sub/link.txt")).is_err());
    }
}

/// STD05: manifest budgets apply while enumerating. A directory past the
/// file cap fails instead of allocating every entry first, and an exactly
/// full directory still scans.
#[test]
fn scan_refuses_directories_past_the_manifest_budget() {
    let scratch = Scratch::tagged("scan-budget");
    let root = scratch.join("root");
    let big = root.join("big");
    let exact = root.join("exact");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::create_dir_all(&exact).unwrap();
    for i in 0..5001 {
        std::fs::write(big.join(format!("entry-{i:06}")), []).unwrap();
    }
    for i in 0..5000 {
        std::fs::write(exact.join(format!("entry-{i:06}")), []).unwrap();
    }
    let mut roots = crate::fs::RootSet::default();
    roots.set_filesystem().unwrap();
    let files = FilesystemLocation::new(roots.snapshot(), &root);
    assert!(crate::manifest::scan(&roots, files.root, &files.path("big")).is_err());
    let fit = crate::manifest::scan(&roots, files.root, &files.path("exact")).unwrap();
    assert_eq!(fit.files.len(), 5000);
    assert!(fit.dirs.is_empty());
}

static SCAN_PARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SCAN_MAX_UNIT: AtomicU64 = AtomicU64::new(0);
static SCAN_FINISHED: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
static SCAN_FINISH_CV: std::sync::Condvar = std::sync::Condvar::new();

fn scan_park_probe(unit: u64, cancelled: &std::sync::atomic::AtomicBool) {
    use std::sync::atomic::Ordering as Seq;
    SCAN_MAX_UNIT.fetch_max(unit, Seq::SeqCst);
    if SCAN_PARKED.swap(true, Seq::SeqCst) {
        return;
    }
    while !cancelled.load(Seq::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn scan_finish_probe(cancelled: bool) {
    *SCAN_FINISHED.lock().unwrap() = Some(cancelled);
    SCAN_FINISH_CV.notify_all();
}

/// STD08: detached scanning yields the executor and holds no shared lock
/// (a sibling scan completes while the first scan is parked), and
/// dropping the future aborts the worker before anything is traversed.
#[test]
fn parked_scan_yields_sibling_and_cancels_before_traversal() {
    use std::sync::{Arc, Mutex};

    let scratch = Scratch::tagged("scan-park");
    let root = scratch.join("root");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::write(root.join("sub/a.txt"), b"parked-bytes").unwrap();
    let mut roots = crate::fs::RootSet::default();
    roots.set_filesystem().unwrap();
    let files = FilesystemLocation::new(roots.snapshot(), &root);
    // Stand-in for the shared file-roots lock: the parked scan must hold
    // no lock across its worker await, so the sibling locks, snapshots
    // and completes while the first scan stays parked.
    let shared = Arc::new(Mutex::new(roots));
    let first = shared.lock().unwrap().clone();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let hooks = crate::manifest::ScanHooks {
        dir_probe: Some(scan_park_probe),
        finish_probe: Some(scan_finish_probe),
    };
    let (handle_tx, handle_rx) = std::sync::mpsc::channel();
    let (sibling_tx, sibling_rx) = std::sync::mpsc::channel();
    let sibling_shared = shared.clone();
    // A current-thread runtime only drives tasks under `block_on`; the
    // driver thread owns it while the test observes from outside.
    let driver = std::thread::spawn(move || {
        runtime.block_on(async move {
            let sibling_files = files.clone();
            let scan = tokio::spawn(crate::manifest::scan_roots_async(
                first,
                files.root,
                files.path(""),
                hooks,
            ));
            handle_tx.send(scan).unwrap();
            let cap = std::time::Instant::now() + Duration::from_secs(30);
            while !SCAN_PARKED.load(Ordering::SeqCst) {
                if std::time::Instant::now() > cap {
                    return;
                }
                tokio::task::yield_now().await;
            }
            // Spawned only after the park is observed: it must lock the
            // shared roots, snapshot and finish while parked.
            tokio::spawn(async move {
                let snapshot = sibling_shared.lock().unwrap().clone();
                let page = crate::manifest::scan_roots_async(
                    snapshot,
                    sibling_files.root,
                    sibling_files.path(""),
                    crate::manifest::ScanHooks::none(),
                )
                .await;
                let _ = sibling_tx.send(page.map(|scan| scan.files.len()));
            });
            while SCAN_FINISHED.lock().unwrap().is_none() {
                if std::time::Instant::now() > cap {
                    return;
                }
                tokio::task::yield_now().await;
            }
        });
    });
    // The worker must reach the parked probe; on the old inline path no
    // probe exists and this times out.
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    while !SCAN_PARKED.load(Ordering::SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "scan worker never parked"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Sibling progress while parked: the async worker is free and no
    // shared lock is held, so a full second scan completes.
    let sibling = sibling_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("sibling blocked during parked scan");
    assert_eq!(
        sibling.expect("sibling scan failed"),
        1,
        "sibling scan result changed while parked"
    );
    // Cancellation aborts the worker without traversing anything: only
    // the pre-access probe fired.
    let scan = handle_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    scan.abort();
    let finished = SCAN_FINISHED.lock().unwrap();
    let (finished, timeout) = SCAN_FINISH_CV
        .wait_timeout_while(finished, Duration::from_secs(5), |done| done.is_none())
        .unwrap();
    assert!(
        !timeout.timed_out(),
        "scan worker never finished after abort"
    );
    assert_eq!(
        *finished,
        Some(true),
        "scan worker did not report cancellation"
    );
    drop(finished);
    assert_eq!(
        SCAN_MAX_UNIT.load(Ordering::SeqCst),
        0,
        "parked scan traversed directories"
    );
    driver.join().expect("driver thread panicked");
}

#[test]
fn senders_keep_no_copy_of_their_sources() {
    // A sender reads its source where it is: while a serve holds it open,
    // the sender's transfer directory holds no copy of its bytes.
    let (host, controller, host_scratch, controller_scratch) = setup("no-copy");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    // Distinct 2 MiB files (incompressible xorshift, different seeds).
    let up_bytes = payload(2 * 1024 * 1024, 101);
    let down_bytes = payload(2 * 1024 * 1024, 202);
    std::fs::write(controller_scratch.join("root/up.bin"), &up_bytes).unwrap();
    std::fs::write(host_scratch.join("root/down.bin"), &down_bytes).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    // Controller upload: the controller sends, the host receives.
    let up = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("up.bin"),
            host_files.root,
            &host_files.path("got-up"),
        )
        .unwrap();
    let up_rx = receiver_id(&host, up);
    let probe = arm_serve_probe(&controller, up);
    let (up_copies, (up_sender, up_receiver)) = std::thread::scope(|scope| {
        let drive = scope.spawn(|| drive_to_end(&controller, up, &host, up_rx));
        let parked = probe.parked();
        let copies = files_with_bytes(&controller_scratch.join("blobs"), &up_bytes);
        probe.release();
        assert!(parked, "the upload serve never parked");
        (copies, drive.join().unwrap())
    });
    drop(probe);
    assert!(up_copies.is_empty(), "the upload sender copied its source");
    assert_eq!(up_sender.state, JobState::Complete);
    assert_eq!(up_receiver.state, JobState::Complete);

    // Host download: the host sends, the controller receives.
    controller
        .create_dir(
            controller_files.root,
            &controller_files.path(""),
            "got-down",
        )
        .unwrap();
    let down = controller
        .job_submit_download(
            &host.endpoint_id(),
            host_files.root,
            &host_files.path("down.bin"),
            controller_files.root,
            &controller_files.path("got-down"),
        )
        .unwrap();
    // Host has upload-receiver (up_rx) + download-sender (new); find sender.
    let host_down = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|j| j.id != up_rx && j.sender)
        .expect("host download sender")
        .id;
    let probe = arm_serve_probe(&host, host_down);
    let (down_copies, (down_receiver, down_sender)) = std::thread::scope(|scope| {
        let drive = scope.spawn(|| drive_to_end(&controller, down, &host, host_down));
        let parked = probe.parked();
        let copies = files_with_bytes(&host_scratch.join("blobs"), &down_bytes);
        probe.release();
        assert!(parked, "the download serve never parked");
        (copies, drive.join().unwrap())
    });
    drop(probe);
    assert!(
        down_copies.is_empty(),
        "the download sender copied its source"
    );
    assert_eq!(down_receiver.state, JobState::Complete);
    assert_eq!(down_sender.state, JobState::Complete);

    // Each receiver published the other peer's bytes, exactly once.
    assert_eq!(
        files_with_bytes(&host_scratch.join("root"), &up_bytes).len(),
        1
    );
    assert_eq!(
        files_with_bytes(&controller_scratch.join("root"), &down_bytes).len(),
        1
    );
}

/// Every file under `dir` whose bytes equal `bytes`.
fn files_with_bytes(dir: &Path, bytes: &[u8]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if std::fs::read(&path).is_ok_and(|content| content == bytes) {
                found.push(path);
            }
        }
    }
    found
}

#[test]
#[cfg(windows)]
fn windows_serve_holds_its_source_against_replacement() {
    // While a serve holds its verified source, Windows refuses to rename the
    // file, rename its folder or open it for writing, so the referenced path
    // keeps naming the verified bytes for the whole request.
    let (host, controller, host_scratch, controller_scratch) = setup("held-source");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let folder = controller_scratch.join("root/folder");
    std::fs::create_dir_all(&folder).unwrap();
    let original = payload(64 * 1024, 1711);
    let source = folder.join("held.bin");
    std::fs::write(&source, &original).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("folder/held.bin"),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);

    let probe = arm_serve_probe(&controller, job);
    std::thread::scope(|scope| {
        let poll = scope.spawn(|| drive_to_end(&controller, job, &host, rx));
        probe.wait_until_parked();
        assert!(
            std::fs::rename(&source, folder.join("moved.bin")).is_err(),
            "the held source was renamed"
        );
        assert!(
            std::fs::rename(&folder, controller_scratch.join("root/moved")).is_err(),
            "the held source's folder was renamed"
        );
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .open(&source)
                .is_err(),
            "the held source was opened for writing"
        );
        probe.release();
        let (sender, receiver) = poll.join().unwrap();
        assert_eq!(sender.state, JobState::Complete);
        assert_eq!(receiver.state, JobState::Complete);
    });
    drop(probe);
    assert_eq!(
        files_with_bytes(&host_scratch.join("root"), &original).len(),
        1
    );
    // Released with the request: the source can move again. The handle closes
    // as the serve ends, and a scanner may hold a just-closed file for a moment.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while let Err(error) = std::fs::rename(&source, folder.join("moved.bin")) {
        assert!(
            std::time::Instant::now() < deadline,
            "the released source still cannot move: {error}"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn owned_files(core: &Core, host: &Core) -> (crate::ServiceInfo, crate::RemoteFilesService) {
    let pending = core.begin_connect(&host.endpoint_id()).unwrap();
    await_jobs(|| {
        core.controller_session(pending.id).unwrap().phase == crate::SessionPhase::Active
    });
    let session = core.controller_session(pending.id).unwrap();
    let owner = core
        .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
        .unwrap();
    let pipe = core.open_files_service(owner.id, owner.generation).unwrap();
    (owner, pipe)
}

fn await_jobs(mut condition: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(
            std::time::Instant::now() < until,
            "owned job condition timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Cross-volume pre-stage barrier: parked inside the copy while global
/// locks must stay free. Scoped to one job id; the guard resets the
/// process hooks on completion or panic so no other test observes them.
#[cfg(unix)]
mod prestage_gate {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};

    static ENTERED: AtomicBool = AtomicBool::new(false);
    static RELEASED: Mutex<bool> = Mutex::new(false);
    static CV: Condvar = Condvar::new();

    pub fn reset() {
        ENTERED.store(false, Ordering::SeqCst);
        *RELEASED.lock().unwrap() = false;
    }

    pub fn entered() -> bool {
        ENTERED.load(Ordering::SeqCst)
    }

    pub fn probe(_id: u64, _bytes: u64) {
        if ENTERED.swap(true, Ordering::SeqCst) {
            return;
        }
        let guard = RELEASED.lock().unwrap();
        // Bounded park: the test releases promptly; the timeout only
        // frees a worker orphaned by a failed test.
        let _ = CV.wait_timeout(guard, std::time::Duration::from_secs(30));
    }

    pub fn release() {
        *RELEASED.lock().unwrap() = true;
        CV.notify_all();
    }

    pub struct Guard;

    impl Guard {
        pub fn install(engine: usize, job: u64) -> Self {
            reset();
            crate::core::Core::set_prestage_hooks(crate::core::PrestageHooks {
                engine,
                job_id: Some(job),
                force_cross_volume: true,
                probe: Some(probe),
                mkdir_probe: None,
                sync_probe: None,
            });
            Self
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            release();
            crate::core::Core::set_prestage_hooks(crate::core::PrestageHooks {
                engine: 0,
                job_id: None,
                force_cross_volume: false,
                probe: None,
                mkdir_probe: None,
                sync_probe: None,
            });
        }
    }
}

/// STD03: a cross-volume publish copies outside global locks, so a
/// snapshot and a revocation complete while the copy is parked, and the
/// revoked destination is never published.
#[cfg(unix)]
#[test]
fn revoked_cross_volume_publish_never_lands_and_locks_stay_free() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup_confined("prestage");
    std::fs::write(
        controller_scratch.join("root/big.bin"),
        payload(200 * 1024, 7),
    )
    .unwrap();
    let host = Arc::new(host);
    let controller = Arc::new(controller);
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "arrived")
        .unwrap();
    let rx = receiver_id(&host, job);
    let _guard = prestage_gate::Guard::install(host.prestage_key(), rx);
    let driver_host = Arc::clone(&host);
    let driver_controller = Arc::clone(&controller);
    let worker = std::thread::spawn(move || {
        let mut sender = driver_controller.job_snapshot(job).unwrap();
        let mut receiver = driver_host.job_snapshot(rx).unwrap();
        for _ in 0..500 {
            if !receiver.state.is_terminal() {
                receiver = driver_host.job_poll(rx).unwrap();
            }
            if !sender.state.is_terminal() {
                sender = driver_controller.job_poll(job).unwrap();
            }
            if receiver.state.is_terminal() {
                break;
            }
        }
        receiver.state
    });
    // The worker must reach the pre-stage barrier; on the old
    // under-lock copy path no probe exists and this times out.
    let deadline = Instant::now() + Duration::from_secs(8);
    while !prestage_gate::entered() {
        assert!(Instant::now() < deadline, "pre-stage barrier never reached");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Locks are free while the copy is parked: bound the wait so a
    // regression fails instead of deadlocking.
    let snapshot_host = Arc::clone(&host);
    let (done, completion) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(snapshot_host.snapshot().map(|_| ()));
    });
    completion
        .recv_timeout(Duration::from_secs(5))
        .expect("snapshot blocked during parked copy")
        .unwrap();
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    prestage_gate::release();
    let receiver_state = worker.join().expect("driver thread panicked");
    assert_eq!(receiver_state, JobState::Interrupted);
    assert!(!host_scratch.join("root/arrived/big.bin").exists());
    // The parent chain is created eagerly; only file publication counts.
    let arrived = host_scratch.join("root/arrived");
    assert!(
        !arrived.exists() || std::fs::read_dir(&arrived).unwrap().count() == 0,
        "revoked publish landed files"
    );
    let mut leftovers = Vec::new();
    for entry in walkdir(&host_scratch.path) {
        if entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".vnlpub."))
        {
            leftovers.push(entry);
        }
    }
    assert!(leftovers.is_empty(), "sibling temps leaked: {leftovers:?}");
}

/// Parent-mkdir and final-stage-sync barriers: parked independently while
/// global locks must stay free. Scoped to one job id; the guard releases
/// both barriers and resets the process hooks on completion or panic so no
/// other test observes them.
#[cfg(unix)]
mod publish_gate {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex};

    static MKDIR_ENTERED: AtomicBool = AtomicBool::new(false);
    static MKDIR_RELEASED: Mutex<bool> = Mutex::new(false);
    static MKDIR_CV: Condvar = Condvar::new();
    static SYNC_ENTERED: AtomicBool = AtomicBool::new(false);
    static SYNC_RELEASED: Mutex<bool> = Mutex::new(false);
    static SYNC_CV: Condvar = Condvar::new();

    pub fn reset() {
        MKDIR_ENTERED.store(false, Ordering::SeqCst);
        *MKDIR_RELEASED.lock().unwrap() = false;
        SYNC_ENTERED.store(false, Ordering::SeqCst);
        *SYNC_RELEASED.lock().unwrap() = false;
    }

    pub fn mkdir_entered() -> bool {
        MKDIR_ENTERED.load(Ordering::SeqCst)
    }

    pub fn sync_entered() -> bool {
        SYNC_ENTERED.load(Ordering::SeqCst)
    }

    fn park(entered: &AtomicBool, released: &Mutex<bool>, cv: &Condvar) {
        if entered.swap(true, Ordering::SeqCst) {
            return;
        }
        let guard = released.lock().unwrap();
        // Bounded park: the test releases promptly; the timeout only
        // frees a worker orphaned by a failed test.
        let _ = cv.wait_timeout(guard, std::time::Duration::from_secs(30));
    }

    pub fn mkdir_probe(_id: u64) {
        park(&MKDIR_ENTERED, &MKDIR_RELEASED, &MKDIR_CV);
    }

    pub fn sync_probe(_id: u64) {
        park(&SYNC_ENTERED, &SYNC_RELEASED, &SYNC_CV);
    }

    pub fn release_mkdir() {
        *MKDIR_RELEASED.lock().unwrap() = true;
        MKDIR_CV.notify_all();
    }

    pub fn release_sync() {
        *SYNC_RELEASED.lock().unwrap() = true;
        SYNC_CV.notify_all();
    }

    pub struct Guard;

    impl Guard {
        pub fn install(engine: usize, job: u64) -> Self {
            reset();
            crate::core::Core::set_prestage_hooks(crate::core::PrestageHooks {
                engine,
                job_id: Some(job),
                force_cross_volume: true,
                probe: None,
                mkdir_probe: Some(mkdir_probe),
                sync_probe: Some(sync_probe),
            });
            Self
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            release_mkdir();
            release_sync();
            crate::core::Core::set_prestage_hooks(crate::core::PrestageHooks {
                engine: 0,
                job_id: None,
                force_cross_volume: false,
                probe: None,
                mkdir_probe: None,
                sync_probe: None,
            });
        }
    }
}

/// STD03 (v3 follow-up): parent mkdirs and the final stage sync both run
/// outside global locks. While each storage barrier is parked, the owner
/// operations behind the same locks stop/revoke needs settle; a
/// revocation during the parked sync still prevents the publish from
/// landing.
#[cfg(unix)]
#[test]
fn parked_mkdir_and_stage_sync_leave_locks_free() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    fn bounded(name: &'static str, task: impl FnOnce() + Send + 'static) {
        let (done, completion) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            task();
            let _ = done.send(());
        });
        if completion.recv_timeout(Duration::from_secs(5)).is_err() {
            panic!("{name} blocked while storage was parked");
        }
        worker.join().expect("bounded op panicked");
    }
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup_confined("pubgate");
    std::fs::write(
        controller_scratch.join("root/big.bin"),
        payload(200 * 1024, 7),
    )
    .unwrap();
    let host = Arc::new(host);
    let controller = Arc::new(controller);
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "arrived")
        .unwrap();
    let rx = receiver_id(&host, job);
    let _guard = publish_gate::Guard::install(host.prestage_key(), rx);
    let driver_host = Arc::clone(&host);
    let driver_controller = Arc::clone(&controller);
    let worker = std::thread::spawn(move || {
        let mut sender = driver_controller.job_snapshot(job).unwrap();
        let mut receiver = driver_host.job_snapshot(rx).unwrap();
        for _ in 0..500 {
            if !receiver.state.is_terminal() {
                receiver = driver_host.job_poll(rx).unwrap();
            }
            if !sender.state.is_terminal() {
                sender = driver_controller.job_poll(job).unwrap();
            }
            if receiver.state.is_terminal() {
                break;
            }
        }
        receiver.state
    });
    // Phase 1: parked in parent mkdirs with no global lock held.
    let deadline = Instant::now() + Duration::from_secs(8);
    while !publish_gate::mkdir_entered() {
        assert!(Instant::now() < deadline, "mkdir barrier never reached");
        std::thread::sleep(Duration::from_millis(10));
    }
    // The state, jobs and roots locks stop/revoke needs are all free.
    let snapshot_host = Arc::clone(&host);
    bounded("snapshot during parked mkdir", move || {
        snapshot_host.snapshot().unwrap();
    });
    let jobs_host = Arc::clone(&host);
    bounded("job snapshot during parked mkdir", move || {
        jobs_host.job_snapshot(rx).unwrap();
    });
    let roots_host = Arc::clone(&host);
    bounded("file roots during parked mkdir", move || {
        roots_host.file_roots().unwrap();
    });
    publish_gate::release_mkdir();
    // Phase 2: parked at the final stage sync, after the copy loop.
    let deadline = Instant::now() + Duration::from_secs(8);
    while !publish_gate::sync_entered() {
        assert!(
            Instant::now() < deadline,
            "stage-sync barrier never reached"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let snapshot_host = Arc::clone(&host);
    bounded("snapshot during parked stage sync", move || {
        snapshot_host.snapshot().unwrap();
    });
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    publish_gate::release_sync();
    let receiver_state = worker.join().expect("driver thread panicked");
    assert_eq!(receiver_state, JobState::Interrupted);
    assert!(!host_scratch.join("root/arrived/big.bin").exists());
    let arrived = host_scratch.join("root/arrived");
    assert!(
        !arrived.exists() || std::fs::read_dir(&arrived).unwrap().count() == 0,
        "revoked publish landed files"
    );
    let mut leftovers = Vec::new();
    for entry in walkdir(&host_scratch.path) {
        if entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".vnlpub."))
        {
            leftovers.push(entry);
        }
    }
    assert!(leftovers.is_empty(), "sibling temps leaked: {leftovers:?}");
}

#[cfg(unix)]
fn walkdir(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

/// Polls until `done` files land. Job polls advance one file, so the
/// bound only guards against hangs; the loop never sleeps.
fn poll_until_files_done(receiver: &Core, job: u64, done: u64) {
    for _ in 0..20 {
        let snap = receiver.job_poll(job).unwrap();
        assert!(
            !snap.state.is_terminal(),
            "job ended early: {:?}",
            snap.state
        );
        if snap.files_done == done {
            return;
        }
    }
    panic!("files_done never reached {done}");
}

/// Target engine and job plus hit/release barriers for the in-flight
/// serve probe. The probe parks only the armed job; every other serve
/// passes straight through, so parallel suites are unaffected.
static REBIND_TARGET: std::sync::Mutex<Option<(usize, u64)>> = std::sync::Mutex::new(None);
static REBIND_HIT: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());
static REBIND_RELEASE: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());

fn rebind_serve_probe(engine: usize, job: u64) {
    if *REBIND_TARGET.lock().unwrap() != Some((engine, job)) {
        return;
    }
    *REBIND_HIT.0.lock().unwrap() = true;
    REBIND_HIT.1.notify_one();
    let guard = REBIND_RELEASE.0.lock().unwrap();
    let _guard = REBIND_RELEASE
        .1
        .wait_while(guard, |released| !*released)
        .unwrap();
}

/// The serve probe is process-wide; tests that park a serve take turns.
static SERVE_PROBE_OWNER: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Parks serves of one engine's job until released, and disarms the probe
/// even when the test panics. Engines are distinct per test, so a leaked arm
/// could never match another test's job anyway.
struct RebindProbeGuard {
    _owner: std::sync::MutexGuard<'static, ()>,
}

fn arm_serve_probe(sender: &Core, job: u64) -> RebindProbeGuard {
    let owner = SERVE_PROBE_OWNER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *REBIND_TARGET.lock().unwrap() = Some((sender.engine_key(), job));
    *REBIND_HIT.0.lock().unwrap() = false;
    *REBIND_RELEASE.0.lock().unwrap() = false;
    Core::set_serve_source_probe(Some(rebind_serve_probe));
    RebindProbeGuard { _owner: owner }
}

impl RebindProbeGuard {
    /// Waits up to 30 s for the armed job's serve to open its verified
    /// source, and reports whether it did. Callers release before asserting.
    fn parked(&self) -> bool {
        let guard = REBIND_HIT.0.lock().unwrap();
        let (guard, _) = REBIND_HIT
            .1
            .wait_timeout_while(guard, Duration::from_secs(30), |hit| !*hit)
            .unwrap();
        *guard
    }

    /// Waits for the armed serve to park, releasing it if it never does.
    fn wait_until_parked(&self) {
        if !self.parked() {
            self.release();
            panic!("the armed serve never parked");
        }
    }

    fn release(&self) {
        *REBIND_RELEASE.0.lock().unwrap() = true;
        REBIND_RELEASE.1.notify_all();
    }
}

impl Drop for RebindProbeGuard {
    fn drop(&mut self) {
        Core::set_serve_source_probe(None);
        *REBIND_TARGET.lock().unwrap() = None;
        *REBIND_RELEASE.0.lock().unwrap() = true;
        REBIND_RELEASE.1.notify_all();
    }
}

#[test]
#[cfg(unix)]
fn open_handle_completes_across_source_rebind() {
    // The controller uploads: the dialed side cannot rebind mid-session
    // (Busy), so the sender leg under test is the dialer's own.
    let (host, controller, host_scratch, controller_scratch) = setup_confined("rebind-flight");
    // The host receives; this test parks one particular serve.
    host.fetch_one_file_at_a_time();
    let controller_root = controller_scratch.join("root");
    let one = payload(512, 1601);
    let two = payload(512, 1602);
    std::fs::write(controller_root.join("one.txt"), &one).unwrap();
    std::fs::write(controller_root.join("two.txt"), &two).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "arrived")
        .unwrap();
    let rx = receiver_id(&host, job);
    poll_until_files_done(&host, rx, 1);

    // Decoy second file: same size, different bytes. A handle opened
    // before the rebind still serves the original bytes; only opens
    // after the rebind are denied.
    let rebound = controller_scratch.join("rebound");
    std::fs::create_dir_all(&rebound).unwrap();
    std::fs::write(rebound.join("one.txt"), &one).unwrap();
    std::fs::write(rebound.join("two.txt"), payload(512, 1609)).unwrap();

    let probe = arm_serve_probe(&controller, job);
    std::thread::scope(|scope| {
        let poll = scope.spawn(|| host.job_poll(rx));
        // Wait for file-2's serve to open its handle, then rebind under it.
        probe.wait_until_parked();
        controller.set_file_roots(vec![rebound]).unwrap();
        probe.release();
        let snap = poll.join().unwrap().unwrap();
        assert_eq!(snap.files_done, 2);
    });
    drop(probe);

    // The in-flight handle served the original bytes, not the decoy.
    let arrived = host_scratch.join("root/arrived");
    assert_eq!(std::fs::read(arrived.join("two.txt")).unwrap(), two);
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.state, JobState::Complete);
}

/// A listing slower than its caller waits is that request's timeout: the
/// pipe stays open, reads the late answer to the end, and the next listing
/// gets its own answer. Transfers keep their owner.
#[test]
fn a_slow_listing_times_out_without_ending_the_files_service() {
    let (host, controller, host_scratch, _controller_scratch) = setup("slowlist");
    std::fs::write(host_scratch.join("root/a.txt"), b"a").unwrap();
    let files = FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let (owner, pipe) = owned_files(&controller, &host);
    let key = std::str::FromStr::from_str(&controller.endpoint_id())
        .map(|key: iroh::PublicKey| *key.as_bytes())
        .unwrap();
    // Slower than the caller waits, quicker than a stuck host.
    *crate::core::LIST_DELAY.lock().unwrap() = Some((key, 1_800));
    assert!(matches!(
        pipe.list(files.root, &files.path(""), 0, 100),
        Err(Error::Timeout)
    ));
    *crate::core::LIST_DELAY.lock().unwrap() = None;
    let page = pipe.list(files.root, &files.path(""), 0, 100).unwrap();
    assert!(page.entries.iter().any(|entry| entry.name == "a.txt"));
    assert!(!pipe.is_cancelled());
    let service = controller.controller_service(owner.id).unwrap();
    assert_eq!(service.phase, crate::ServicePhase::Active);
    host.shutdown();
    controller.shutdown();
}

/// Transfers left interrupted when their Files owner went wait to resume
/// and hold back nothing: with eight of them, a ninth is accepted, and the
/// eight stay there to be taken over.
#[test]
fn interrupted_transfers_leave_room_for_new_ones() {
    let (host, controller, hs, cs) = setup("room");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let pending = controller.begin_connect(&host.endpoint_id()).unwrap();
    await_jobs(|| {
        controller.controller_session(pending.id).unwrap().phase == crate::SessionPhase::Active
    });
    let session = controller.controller_session(pending.id).unwrap();
    let open = || {
        let owner = controller
            .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
            .unwrap();
        let pipe = controller
            .open_files_service(owner.id, owner.generation)
            .unwrap();
        (owner, pipe)
    };
    let (lost, pipe) = open();
    for index in 0..crate::core::MAX_ACTIVE_JOBS {
        let name = format!("waiting-{index}.bin");
        std::fs::write(
            cs.join("root").join(&name),
            payload(512, 600 + index as u64),
        )
        .unwrap();
        controller
            .files_service_upload(
                lost.id,
                lost.generation,
                controller_files.root,
                &controller_files.path(""),
                &[name],
                host_files.root,
                &host_files.path(""),
            )
            .unwrap();
    }
    controller
        .close_controller_service(lost.id, lost.generation)
        .unwrap();
    drop(pipe);
    await_jobs(|| {
        controller
            .job_list()
            .unwrap()
            .iter()
            .all(|job| job.state == JobState::Interrupted)
    });
    await_jobs(|| host.hosted_services().unwrap().is_empty());
    let (fresh, _pipe) = open();
    std::fs::write(cs.join("root/ninth.bin"), payload(512, 699)).unwrap();
    let ninth = controller
        .files_service_upload(
            fresh.id,
            fresh.generation,
            controller_files.root,
            &controller_files.path(""),
            &["ninth.bin".to_owned()],
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    assert_eq!(
        controller.job_snapshot(ninth).unwrap().state,
        JobState::Serving
    );
    let waiting = controller
        .job_list()
        .unwrap()
        .into_iter()
        .filter(|job| job.state == JobState::Interrupted)
        .count();
    assert_eq!(waiting, crate::core::MAX_ACTIVE_JOBS);
}

static PREPARE_PARKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PREPARE_RELEASED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Parks a host's download preparation before it reads anything, until
/// released or cancelled.
fn prepare_park_probe(unit: u64, cancelled: &std::sync::atomic::AtomicBool) {
    if unit != 0 {
        return;
    }
    PREPARE_PARKED.store(true, Ordering::Release);
    while !PREPARE_RELEASED.load(Ordering::Acquire) && !cancelled.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A download whose preparation takes long is a Preparing job at once:
/// the request is answered while the other computer has not read a byte,
/// so the Files owner is neither held for the whole preparation nor closed
/// when it outlasts the request's bound (60 seconds). Asking again changes
/// nothing until it is prepared; then it finishes. A cancel while
/// preparing stops the preparation on both sides.
#[test]
fn a_long_download_preparation_shows_and_finishes() {
    let (host, controller, hs, cs) = setup("prepare");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let bytes = payload(4096, 801);
    std::fs::write(hs.join("root/slow.bin"), &bytes).unwrap();
    std::fs::write(hs.join("root/second.bin"), payload(4096, 802)).unwrap();
    let (owner, _pipe) = owned_files(&controller, &host);
    struct Unhook;
    impl Drop for Unhook {
        fn drop(&mut self) {
            *crate::core::PREPARE_SCAN_HOOKS.lock().unwrap() = None;
        }
    }
    let _unhook = Unhook;
    *crate::core::PREPARE_SCAN_HOOKS.lock().unwrap() = Some((
        host.prestage_key(),
        manifest::ScanHooks {
            dir_probe: Some(prepare_park_probe),
            finish_probe: None,
        },
    ));
    let download = |name: &str| {
        controller.files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            &host_files.path(""),
            &[name.to_owned()],
            controller_files.root,
            &controller_files.path(""),
        )
    };
    let job = download("slow.bin").unwrap();
    assert!(
        PREPARE_PARKED.load(Ordering::Acquire),
        "answered only once prepared"
    );
    assert_eq!(
        controller.job_snapshot(job).unwrap().state,
        JobState::Preparing
    );
    let preparing = |core: &Core| {
        core.job_list()
            .unwrap()
            .into_iter()
            .filter(|job| job.state == JobState::Preparing)
            .map(|job| job.id)
            .collect::<Vec<_>>()
    };
    let host_job = preparing(&host)[0];
    std::thread::sleep(Duration::from_millis(1500));
    let asked = controller.job_poll(job).unwrap();
    assert_eq!(asked.state, JobState::Preparing);
    assert_eq!(asked.files_total, 0);
    assert_eq!(
        controller.controller_service(owner.id).unwrap().phase,
        crate::ServicePhase::Active
    );
    PREPARE_RELEASED.store(true, Ordering::Release);
    await_jobs(|| controller.job_poll(job).unwrap().state != JobState::Preparing);
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(
        (receiver.state, sender.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(std::fs::read(cs.join("root/slow.bin")).unwrap(), bytes);

    PREPARE_PARKED.store(false, Ordering::Release);
    PREPARE_RELEASED.store(false, Ordering::Release);
    let second = download("second.bin").unwrap();
    assert_eq!(
        controller.job_snapshot(second).unwrap().state,
        JobState::Preparing
    );
    let host_second = preparing(&host)[0];
    assert_eq!(
        controller.job_cancel(second).unwrap().state,
        JobState::Cancelled
    );
    await_jobs(|| host.job_snapshot(host_second).unwrap().state == JobState::Cancelled);
    // The cancelled preparation stopped: nothing is offered or lands.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        host.job_snapshot(host_second).unwrap().state,
        JobState::Cancelled
    );
    assert!(!cs.join("root/second.bin").exists());
}

/// A sender that does not answer in time costs the receiver one retry, not
/// the job: the next try, after its pause, finishes the file without
/// anyone resuming it.
#[test]
fn a_late_sender_is_retried_without_a_resume() {
    let (host, controller, host_scratch, controller_scratch) = setup("retry");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let bytes = payload(64 * 1024, 1801);
    std::fs::write(controller_scratch.join("root/late.bin"), &bytes).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("late.bin"),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);

    // Hold the first serve past the receiver's answer deadline.
    let probe = arm_serve_probe(&controller, job);
    let first = std::thread::scope(|scope| {
        let poll = scope.spawn(|| host.job_poll(rx).unwrap());
        assert!(probe.parked(), "the serve never opened");
        let first = poll.join().unwrap();
        probe.release();
        first
    });
    drop(probe);
    assert_eq!(first.state, JobState::Transferring);
    assert_eq!(
        first.error.as_deref(),
        Some("the other computer could not be reached")
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let receiver = loop {
        let snapshot = host.job_poll(rx).unwrap();
        if snapshot.state.is_terminal() {
            break snapshot;
        }
        assert_ne!(snapshot.state, JobState::Interrupted);
        assert!(std::time::Instant::now() < deadline, "the retry never ran");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(receiver.error, None);
    assert_eq!(
        files_with_bytes(&host_scratch.join("root"), &bytes).len(),
        1
    );
}

/// While a job takes one file, it fetches the next small files ahead, so a
/// folder of small files lands without a round trip per file; they still
/// land in order and exactly. Cancelling stops what is ahead.
#[test]
fn small_files_arrive_ahead_and_land_in_order() {
    let (host, controller, host_scratch, controller_scratch) = setup("ahead");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let folder = controller_scratch.join("root/many");
    std::fs::create_dir_all(&folder).unwrap();
    for n in 0..20u64 {
        std::fs::write(folder.join(format!("f{n:02}.bin")), payload(3000, 2000 + n)).unwrap();
    }
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("many"),
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    let rx = receiver_id(&host, job);

    let first = host.job_poll(rx).unwrap();
    assert_eq!(first.files_done, 1);
    assert_eq!(
        host.fetching_ahead(rx),
        7,
        "seven files ahead of the second"
    );
    // Each later poll lands exactly the next file.
    let second = host.job_poll(rx).unwrap();
    assert_eq!(second.files_done, 2);
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete)
    );
    manifest::assert_trees_equal(&folder, &host_scratch.join("root/many"));
    assert_eq!(host.fetching_ahead(rx), 0);

    // A second job, cancelled with files ahead, stops them.
    let again = controller_scratch.join("root/again");
    std::fs::create_dir_all(&again).unwrap();
    for n in 0..10u64 {
        std::fs::write(again.join(format!("g{n:02}.bin")), payload(3000, 2100 + n)).unwrap();
    }
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("again"),
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    let rx = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|listed| !listed.sender && listed.state != JobState::Complete)
        .expect("the second receiver")
        .id;
    host.job_poll(rx).unwrap();
    assert!(host.fetching_ahead(rx) > 0);
    host.job_cancel(rx).unwrap();
    assert_eq!(host.fetching_ahead(rx), 0);
    let _ = job;
}
