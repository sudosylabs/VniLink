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
