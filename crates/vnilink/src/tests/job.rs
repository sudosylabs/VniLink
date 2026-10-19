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
use crate::job::{ConflictChoice, JobState};
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

    /// Scratch on a RAM disk when one is usable, else the temp dir.
    fn tagged_fast(tag: &str) -> Self {
        let base = std::path::PathBuf::from("/dev/shm");
        let probe = base.join(format!(".vnilink-probe-{}", std::process::id()));
        let ram = std::fs::create_dir_all(&probe)
            .and_then(|_| std::fs::remove_dir(&probe))
            .is_ok();
        Self::tagged_in(tag, if ram { base } else { std::env::temp_dir() })
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

/// Serializes the multi-megabyte job tests. Each `Core` plus each blob
/// store brings its own multi-threaded runtimes; running several bulk
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

fn write_tree(root: &Path) {
    std::fs::create_dir_all(root.join("docs/2026")).unwrap();
    std::fs::create_dir_all(root.join("empty")).unwrap();
    std::fs::create_dir_all(root.join("pack/sub")).unwrap();
    std::fs::write(root.join("top.txt"), payload(1024, 11)).unwrap();
    std::fs::write(root.join("docs/read me-ß.txt"), payload(4096, 22)).unwrap();
    std::fs::write(root.join("docs/2026/big.bin"), payload(9 * 1024 * 1024, 33)).unwrap();
    std::fs::write(root.join("pack/sub/a.txt"), payload(512, 44)).unwrap();
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
        core.set_blob_directory(scratch.join("blobs")).unwrap();
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
    let mut last_sender = sender.job_snapshot(sender_id).unwrap();
    let mut last_receiver = receiver.job_snapshot(receiver_id).unwrap();
    for _ in 0..200 {
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
        hash: [7; 32],
        size: 10,
        source_mtime_ms: 0,
    };
    let dir = |path: &str| ManifestEntry {
        path: path.to_owned(),
        is_dir: true,
        hash: [0; 32],
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

#[test]
fn upload_tree_matches_exactly() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup("upload");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    write_tree(&controller_scratch.join("root"));

    let session = controller.connect(&host.endpoint_id()).unwrap();
    let _ = session;
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path(""),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.done_bytes, receiver.total_bytes);
    assert!(receiver.total_bytes > 9 * 1024 * 1024);
    // Queue rows show the first path plus a count, never raw internals.
    assert!(
        receiver.label.starts_with("root/docs/2026/big.bin")
            || receiver.label.starts_with("root/docs/read")
    );
    assert!(receiver.label.contains("more"));
    assert!(!format!("{:?}", receiver).contains("arrived"));

    manifest::assert_trees_equal(
        &controller_scratch.join("root"),
        &host_scratch.join("root/arrived/root"),
    );
    // The source is a copy: sending leaves every source file in place.
    assert!(controller_scratch.join("root/docs/2026/big.bin").exists());
}

#[test]
fn download_tree_matches_exactly() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup("download");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    write_tree(&host_scratch.join("root"));

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
    // The host's sender record mirrors completion through status polls.
    let host_jobs = host.job_list().unwrap();
    assert_eq!(host_jobs.len(), 1);
    let host_job = host_jobs[0].id;
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);

    manifest::assert_trees_equal(
        &host_scratch.join("root"),
        &controller_scratch.join("root/got/root"),
    );
}

#[test]
fn conflicts_follow_explicit_choices() {
    let (host, controller, host_scratch, controller_scratch) = setup("conflict");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::create_dir_all(controller_scratch.join("root")).unwrap();
    std::fs::write(controller_scratch.join("root/a.txt"), payload(100, 1)).unwrap();
    std::fs::write(controller_scratch.join("root/b.txt"), payload(100, 2)).unwrap();
    std::fs::write(controller_scratch.join("root/c.txt"), payload(100, 3)).unwrap();

    let dest = host_scratch.join("root/dest/root");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("a.txt"), b"old-a").unwrap();
    std::fs::write(dest.join("b.txt"), b"old-b").unwrap();
    std::fs::write(dest.join("c.txt"), b"old-c").unwrap();
    std::fs::write(dest.join("unrelated.txt"), b"unrelated").unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path(""),
            host_files.root,
            &host_files.path("dest"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);

    // First conflict waits; Replace with apply-to-remaining would hide the
    // per-file paths, so answer the first file only.
    let waiting = host.job_poll(rx).unwrap();
    assert_eq!(waiting.state, JobState::WaitingDecision);
    assert_eq!(waiting.waiting_file.as_deref(), Some("root/a.txt"));
    host.job_resolve(rx, ConflictChoice::Replace, false)
        .unwrap();
    // Second conflict: skip it without touching the original.
    let waiting = (0..10)
        .map(|_| host.job_poll(rx).unwrap())
        .find(|snap| snap.state == JobState::WaitingDecision)
        .expect("second conflict waits");
    assert_eq!(waiting.waiting_file.as_deref(), Some("root/b.txt"));
    host.job_resolve(rx, ConflictChoice::Skip, false).unwrap();
    // Third conflict: keep both, preserving the original name.
    let waiting = (0..10)
        .map(|_| host.job_poll(rx).unwrap())
        .find(|snap| snap.state == JobState::WaitingDecision)
        .expect("third conflict waits");
    assert_eq!(waiting.waiting_file.as_deref(), Some("root/c.txt"));
    host.job_resolve(rx, ConflictChoice::KeepBoth, false)
        .unwrap();
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);

    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), payload(100, 1));
    assert_eq!(std::fs::read(dest.join("b.txt")).unwrap(), b"old-b");
    assert_eq!(
        std::fs::read(dest.join("unrelated.txt")).unwrap(),
        b"unrelated"
    );
    let kept: Vec<String> = std::fs::read_dir(&dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(kept.iter().any(|name| name.starts_with("c (2)")));
    assert_eq!(
        std::fs::read(dest.join(kept.iter().find(|name| name.starts_with("c (2)")).unwrap()))
            .unwrap(),
        payload(100, 3)
    );
}

#[test]
fn interrupted_download_resumes_with_byte_reuse() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let host = start_direct();
    let receiver_a = start_direct();
    // RAM-backed scratch where available: store open/close does
    // synchronous file IO whose latency spikes on overlay filesystems
    // under suite load. The 9 MiB fixture plus store overhead fits
    // easily in /dev/shm; elsewhere this falls back to the temp dir.
    let scratch = Scratch::tagged_fast("blob-resume");
    host.set_blob_directory(scratch.join("host-blobs")).unwrap();
    receiver_a
        .set_blob_directory(scratch.join("rx-blobs"))
        .unwrap();
    // Blob transfers need pairing plus the transfer grant, not a control
    // session or hosting.
    receiver_a
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &receiver_a
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    host.set_desktop_sharing(crate::session::Grants {
        view: false,
        control: false,
        clipboard: false,
        transfer: true,
    })
    .unwrap();

    // 9 MiB clears the legacy cap with room to spare: it must stream.
    // Smaller keeps the suite fast; the reuse proof needs only a
    // multi-megabyte verified prefix, gated below.
    let data = payload(9 * 1024 * 1024, 77);
    let (hash, size) = host.blob_offer(&receiver_a.endpoint_id(), &data).unwrap();

    // Fetch in a thread. The main thread waits until eight verified
    // megabytes are present (or the fetch wins the race outright), then
    // cancels or joins: either branch leaves verified ranges behind and
    // the byte count below distinguishes reuse from a full re-download.
    // No sleeps anywhere; every wait has a 30 s bound.
    let (progress_tx, progress_rx) = std::sync::mpsc::channel();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let host_id = host.endpoint_id();
    let worker_host = host_id.clone();
    let shared = std::sync::Arc::new(receiver_a);
    let worker_shared = std::sync::Arc::clone(&shared);
    let worker = std::thread::spawn(move || {
        worker_shared.blob_fetch_streamed(&worker_host, hash, Some(progress_tx), Some(cancel_rx))
    });
    loop {
        // Cancel once a megabyte is verified: early enough to land
        // mid-flight, large enough to prove reuse on resume.
        match shared.blob_verified(hash).unwrap_or(None) {
            Some(n) if n >= 1 * 1024 * 1024 => {
                // The fetch may complete in the same instant; a
                // disconnected cancel means everything is verified.
                let _ = cancel_tx.send(true);
                break;
            }
            _ => {}
        }
        // Bound every wait; progress proves the fetch is alive.
        if progress_rx.recv_timeout(Duration::from_secs(30)).is_err() {
            break;
        }
        // A completed fetch needs no cancel: everything is verified.
        if shared.blob_has(hash).unwrap_or(false) {
            break;
        }
    }
    // Either outcome leaves verified ranges behind: a cancelled fetch
    // stopped past the gate, a finished fetch verified everything (and
    // the resume below then reads nothing at all).
    match worker.join().expect("worker joins") {
        Ok(_) => {}
        Err(error) => panic!("interrupted fetch must not fail hard: {error}"),
    }
    // Resume on the same store: verified ranges persist without any
    // reopen, so the resumed fetch must read fewer bytes than the full
    // size. Restart across processes is covered by
    // `job_restart_resumes_unfinished_files`; reopening one directory
    // twice in a single process races the store's asynchronous teardown
    // and is not a supported pattern.
    let resumed = shared
        .blob_fetch_streamed(&host_id, hash, None, None)
        .unwrap();
    match resumed {
        crate::core::blob::FetchResult::Done { downloaded } => {
            assert!(
                downloaded < size,
                "resumed fetch must reuse verified ranges"
            );
        }
        crate::core::blob::FetchResult::Cancelled => panic!("resume must complete"),
    }
    let got = shared.blob_fetch(&host_id, hash, size).unwrap();
    assert_eq!(got, data);
}

#[test]
fn job_restart_resumes_unfinished_files() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, _host_scratch, controller_scratch) = setup("restart");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &_host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let host_root = _host_scratch.join("root");
    std::fs::write(host_root.join("one.txt"), payload(1024, 5)).unwrap();
    std::fs::write(host_root.join("two.txt"), payload(1024, 6)).unwrap();
    std::fs::write(host_root.join("three.txt"), payload(1024, 7)).unwrap();

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

    // Two files land, the third is pending: kill the receiver process here.
    controller.job_poll(job).unwrap();
    controller.job_poll(job).unwrap();
    let mid = controller.job_snapshot(job).unwrap();
    assert_eq!(mid.files_done, 2);
    let secret = controller.export_identity();
    let blob_dir = controller_scratch.join("blobs");
    controller.shutdown();
    drop(controller);

    // Same installation, reopened: durable records and verified store
    // bytes survive; the control session is re-established by dialing.
    let controller = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            identity: Some(secret),
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    controller
        .set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller.set_blob_directory(&blob_dir).unwrap();
    controller.enable_filesystem_access().unwrap();
    let jobs = controller.job_list().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, JobState::Interrupted);
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    controller.job_resume(job).unwrap();
    let host_job = host.job_list().unwrap()[0].id;
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.files_done, 3);
    manifest::assert_trees_equal(&host_root, &controller_scratch.join("root/got/root"));
}

#[test]
fn cancel_preserves_the_destination() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_scratch, controller_scratch) = setup("cancel");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(host_scratch.join("root/one.txt"), payload(2048, 8)).unwrap();
    std::fs::write(
        host_scratch.join("root/two.txt"),
        payload(12 * 1024 * 1024, 9),
    )
    .unwrap();

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
    controller.job_poll(job).unwrap();
    // Cancel while the large file is in flight: the 20 MiB fetch takes
    // orders of magnitude longer than thread spawn plus cancel delivery,
    // so the cancel always lands on a live fetch or a pending poll.
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| controller.job_poll(job));
        controller.job_cancel(job).unwrap();
        let snap = handle.join().expect("poll joins").unwrap();
        assert_eq!(snap.state, JobState::Cancelled);
    });
    let snap = controller.job_poll(job).unwrap();
    assert_eq!(snap.state, JobState::Cancelled);
    // The finished file stays; the in-flight destination never appears,
    // not even partially.
    assert_eq!(
        std::fs::read(controller_scratch.join("root/got/root/one.txt")).unwrap(),
        payload(2048, 8)
    );
    assert!(!controller_scratch.join("root/got/root/two.txt").exists());
    for entry in std::fs::read_dir(controller_scratch.join("root/got/root")).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(
            !name.ends_with(".part"),
            "staging leaked into the destination"
        );
    }
    // Resume finishes exactly.
    controller.job_resume(job).unwrap();
    let host_job = host.job_list().unwrap()[0].id;
    let (receiver, _) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    manifest::assert_trees_equal(
        &host_scratch.join("root"),
        &controller_scratch.join("root/got/root"),
    );
}

#[test]
fn export_failure_keeps_the_original() {
    let (host, controller, host_scratch, controller_scratch) = setup("readonly");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(host_scratch.join("root/new.txt"), payload(512, 10)).unwrap();
    let dest = controller_scratch.join("root/got/root");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("new.txt"), b"original").unwrap();

    // Windows enforces read-only on the target file, rather than Unix
    // directory permission bits. Restore it even if an assertion panics.
    #[cfg(windows)]
    let writable = {
        struct RestorePermissions(PathBuf, std::fs::Permissions);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let path = dest.join("new.txt");
        let original = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        RestorePermissions(path, original)
    };

    // Read-only destination: publication must fail without touching it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Running as root would sail through permissions; detect and skip.
        if std::fs::write(dest.join("probe.txt"), b"p").is_ok() {
            let _ = std::fs::remove_file(dest.join("probe.txt"));
            std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
    }

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_download(
            &host.endpoint_id(),
            host_files.root,
            &host_files.path(""),
            controller_files.root,
            &controller_files.path("got"),
        )
        .unwrap();
    let host_job = host.job_list().unwrap()[0].id;
    // Resolve the conflict with Replace so publication is attempted.
    let mut waiting_seen = false;
    for _ in 0..10 {
        let snap = controller.job_poll(job).unwrap();
        if snap.state == JobState::WaitingDecision {
            waiting_seen = true;
            controller
                .job_resolve(job, ConflictChoice::Replace, false)
                .unwrap();
        } else if snap.state.is_terminal() {
            break;
        }
    }
    assert!(waiting_seen);
    let (receiver, _) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Failed);
    assert_eq!(std::fs::read(dest.join("new.txt")).unwrap(), b"original");

    #[cfg(windows)]
    drop(writable);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    controller.job_resume(job).unwrap();
    let (receiver, _) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(
        std::fs::read(dest.join("new.txt")).unwrap(),
        payload(512, 10)
    );
}

#[test]
fn revoke_pauses_and_reauthorize_resumes() {
    let (host, controller, _host_scratch, controller_scratch) = setup("revoke");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &_host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let _ = &controller_scratch;
    std::fs::write(_host_scratch.join("root/one.txt"), payload(512, 12)).unwrap();
    std::fs::write(_host_scratch.join("root/two.txt"), payload(512, 13)).unwrap();

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
    controller.job_poll(job).unwrap();
    // Revoke mid-job: the next poll stops instead of fetching. The
    // record only settles once polled; the snapshot before that still
    // shows the in-flight state.
    host.set_service_sharing(crate::ServiceKind::Files, false)
        .unwrap();
    let next = controller.job_poll(job).unwrap();
    assert_eq!(next.state, JobState::Interrupted);
    assert_eq!(next.files_done, 1);
    assert!(!controller_scratch.join("root/got/root/two.txt").exists());

    // Re-authorize and resume to an exact tree. Revocation dropped the
    // host's offers, so the host sender re-offers before the receiver
    // retries its fetch.
    host.set_desktop_sharing(crate::session::Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: true,
    })
    .unwrap();
    let host_job = host.job_list().unwrap()[0].id;
    host.job_resume(host_job).unwrap();
    controller.job_resume(job).unwrap();
    let (receiver, _) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    manifest::assert_trees_equal(
        &_host_scratch.join("root"),
        &controller_scratch.join("root/got/root"),
    );
}

#[test]
fn submit_needs_a_session_dirs_and_known_roots() {
    let (host, controller, _host_scratch, _controller_scratch) = setup("gates");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &_host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &_controller_scratch.join("root"),
    );
    // No control session yet: submission is refused before any hashing.
    assert!(matches!(
        controller.job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path(""),
            host_files.root,
            &host_files.path("x")
        ),
        Err(Error::NoSession)
    ));
    controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("missing"),
            host_files.root,
            &host_files.path("x")
        ),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        controller.job_submit_download(
            &host.endpoint_id(),
            host_files.root,
            &host_files.path(""),
            u64::MAX,
            &controller_files.path("x")
        ),
        Err(Error::InvalidSetting(_))
    ));
}

#[test]
fn empty_folder_transfer_preserves_hierarchy() {
    let (host, controller, host_scratch, controller_scratch) = setup("empty-dir");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    // Nested empty folders plus one file elsewhere so the manifest is not
    // empty-only edge but the empty branch must still land.
    std::fs::create_dir_all(controller_scratch.join("root/empty/nested")).unwrap();
    std::fs::create_dir_all(controller_scratch.join("root/keep")).unwrap();
    std::fs::write(controller_scratch.join("root/keep/a.txt"), b"a").unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("empty"),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    // The empty hierarchy lands even though it carries no files.
    assert!(host_scratch.join("root/arrived/empty/nested").is_dir());
    assert!(host_scratch.join("root/arrived/empty").is_dir());
}

#[test]
fn sender_cancel_prevents_delivery() {
    let (host, controller, host_scratch, controller_scratch) = setup("sender-cancel");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(controller_scratch.join("root/a.txt"), payload(1024, 21)).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("a.txt"),
            host_files.root,
            &host_files.path("got"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    // Sender cancels before the receiver polls: the destination must never
    // appear and the receiver must observe cancellation (symmetric).
    controller.job_cancel(job).unwrap();
    // Give the cancel message a chance to arrive (best-effort round-trip
    // already happened inside `job_cancel`; one more poll settles).
    let after = host.job_poll(rx).unwrap();
    assert_eq!(after.state, JobState::Cancelled);
    assert!(!host_scratch.join("root/got/a.txt").exists());
    assert!(!host_scratch.join("root/got").join("a.txt").exists());
    let sender = controller.job_snapshot(job).unwrap();
    assert_eq!(sender.state, JobState::Cancelled);
}

#[test]
fn revoke_before_publish_prevents_file_creation() {
    let (host, controller, host_scratch, _controller_scratch) = setup("revoke-publish");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &_controller_scratch.join("root"),
    );
    std::fs::write(host_scratch.join("root/one.txt"), payload(512, 31)).unwrap();
    std::fs::write(host_scratch.join("root/two.txt"), payload(512, 32)).unwrap();

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
    // First file lands.
    controller.job_poll(job).unwrap();
    // Revoke before the second file publishes: no new file may appear.
    host.revoke(&controller.endpoint_id()).unwrap();
    let next = controller.job_poll(job).unwrap();
    assert_eq!(next.state, JobState::Interrupted);
    // Only the first file landed; the second was refused at publication.
    assert!(controller.job_snapshot(job).unwrap().files_done <= 1);
}

#[test]
fn rename_during_active_transfer_is_refused() {
    let (host, controller, host_scratch, controller_scratch) = setup("rename-block");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(controller_scratch.join("root/a.txt"), payload(512, 41)).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("a.txt"),
            host_files.root,
            &host_files.path("dest"),
        )
        .unwrap();
    let _rx = receiver_id(&host, job);
    // The sender source is pinned while the job is active.
    assert!(controller
        .rename_entry(
            controller_files.root,
            &controller_files.path(""),
            "a.txt",
            None,
            "b.txt"
        )
        .is_err());
    // The receiver destination base is pinned while the job is active.
    controller.job_cancel(job).unwrap();
    // After cancellation the rename is allowed again.
    controller
        .rename_entry(
            controller_files.root,
            &controller_files.path(""),
            "a.txt",
            None,
            "b.txt",
        )
        .unwrap();
    assert!(controller_scratch.join("root/b.txt").exists());
    let _ = &host_scratch;
}

#[test]
fn drive_jobs_advances_without_window_polls() {
    let (host, controller, host_scratch, controller_scratch) = setup("drive-all");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(controller_scratch.join("root/a.txt"), payload(1024, 51)).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("a.txt"),
            host_files.root,
            &host_files.path("got"),
        )
        .unwrap();
    // Background-style driving: no direct `job_poll` on the receiver window,
    // only `drive_jobs` on each installation (the background driver path).
    for _ in 0..50 {
        let _ = controller.drive_jobs().unwrap();
        let _ = host.drive_jobs().unwrap();
        let sender = controller.job_snapshot(job).unwrap();
        let receiver = host.job_list().unwrap()[0].clone();
        if sender.state.is_terminal() && receiver.state.is_terminal() {
            break;
        }
    }
    let sender = controller.job_snapshot(job).unwrap();
    let receiver = host.job_list().unwrap()[0].clone();
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(
        std::fs::read(host_scratch.join("root/got/a.txt")).unwrap(),
        payload(1024, 51)
    );
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
fn upload_conflict_forwards_to_sender_and_resolves() {
    let (host, controller, host_scratch, controller_scratch) = setup("conflict-fwd");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(controller_scratch.join("root/a.txt"), payload(100, 61)).unwrap();
    let dest = host_scratch.join("root/dest");
    std::fs::create_dir_all(&dest).unwrap();
    std::fs::write(dest.join("a.txt"), b"old").unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("a.txt"),
            host_files.root,
            &host_files.path("dest"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    // Receiver hits the conflict first.
    let waiting = host.job_poll(rx).unwrap();
    assert_eq!(waiting.state, JobState::WaitingDecision);
    // Sender learns it via status (no receiver window needed on controller).
    let sender_waiting = controller.job_poll(job).unwrap();
    assert_eq!(sender_waiting.state, JobState::WaitingDecision);
    assert_eq!(sender_waiting.waiting_file.as_deref(), Some("a.txt"));
    // Controller resolves; the decision forwards to the host receiver.
    controller
        .job_resolve(job, ConflictChoice::Replace, false)
        .unwrap();
    // Drive both to completion (receiver applies forwarded choice).
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(std::fs::read(dest.join("a.txt")).unwrap(), payload(100, 61));
}

#[test]
fn coordinated_resume_after_symmetric_cancel() {
    let (host, controller, host_scratch, controller_scratch) = setup("resume-coord");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    std::fs::write(controller_scratch.join("root/a.txt"), payload(512, 71)).unwrap();
    std::fs::write(controller_scratch.join("root/b.txt"), payload(512, 72)).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path(""),
            host_files.root,
            &host_files.path("got"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    // Symmetric cancel from the sender: both sides Cancelled.
    controller.job_cancel(job).unwrap();
    assert_eq!(
        controller.job_snapshot(job).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(host.job_poll(rx).unwrap().state, JobState::Cancelled);
    // Resume only the receiver: coordination resumes the sender and reoffers.
    // (Control session still active after cancel; no reconnect needed.)
    host.job_resume(rx).unwrap();
    // Sender auto-resumes via notification (no explicit sender resume needed
    // in this path? Actually receiver resume notifies sender; sender still
    // Cancelled until it resumes? Our coordination notifies, so sender should
    // be Serving now. Poll to confirm, then drive.)
    // Drive both (sender may need explicit resume if notification raced;
    // our handler auto-resumes, so driving should complete).
    // To be deterministic, resume the sender explicitly as well (idempotent).
    let _ = controller.job_resume(job);
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert!(host_scratch.join("root/got/root/a.txt").exists());
    assert!(host_scratch.join("root/got/root/b.txt").exists());
    let _ = &controller_scratch;
}

#[test]
fn nested_sender_resume_resolves_correct_file() {
    // Sender restart with a nested directory source must re-offer the right
    // files (root/docs/pack/..., not root/pack/...). Exercises `resume_sender`
    // path reconstruction via durable restart, like the flat restart proof.
    let (host, controller, _host_scratch, controller_scratch) = setup("nested-resume");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &_host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let host_root = _host_scratch.join("root");
    std::fs::create_dir_all(controller_scratch.join("root/docs/pack/sub")).unwrap();
    std::fs::write(
        controller_scratch.join("root/docs/pack/sub/a.txt"),
        payload(512, 81),
    )
    .unwrap();
    std::fs::write(
        controller_scratch.join("root/docs/pack/b.txt"),
        payload(512, 82),
    )
    .unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("docs/pack"),
            host_files.root,
            &host_files.path("got"),
        )
        .unwrap();
    let rx = receiver_id(&host, job);
    // One file lands, then kill the sender process here (receiver keeps its
    // record; sender restarts with same identity and blob dir).
    host.job_poll(rx).unwrap();
    let secret = controller.export_identity();
    let blob_dir = controller_scratch.join("blobs");
    controller.shutdown();
    drop(controller);

    let controller = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            identity: Some(secret),
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    controller
        .set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .unwrap();
    controller.set_blob_directory(&blob_dir).unwrap();
    controller.enable_filesystem_access().unwrap();
    controller
        .accept_invitation(&host.create_invitation(Duration::from_secs(60)).unwrap())
        .unwrap();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    // Sender resume with nested source must not mis-resolve to root/pack/...
    // (the old stripping bug failed here with `InvalidSetting`).
    controller.job_resume(job).unwrap();
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    // Top folder preserved, nested file lands under it.
    assert!(host_root.join("got/pack/sub/a.txt").exists());
    assert!(host_root.join("got/pack/b.txt").exists());
}

#[test]
#[cfg(unix)]
fn anchored_import_refuses_swapped_parent() {
    let (host, controller, host_scratch, controller_scratch) = setup_confined("import-swap");
    std::fs::create_dir_all(controller_scratch.join("root/docs")).unwrap();
    std::fs::write(controller_scratch.join("root/docs/public.txt"), b"public").unwrap();
    let outside = controller_scratch.join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    // Swap `docs` for a symlink to outside (equal sizes bypass size guards:
    // `public` and `secret` are both 6 bytes).
    std::fs::remove_dir_all(controller_scratch.join("root/docs")).unwrap();
    std::os::unix::fs::symlink(&outside, controller_scratch.join("root/docs")).unwrap();
    // Anchored import of `docs/public.txt` must fail, never import outside.
    // Dummy scan identity (open fails on symlink parent before dev/ino checks).
    let dummy = crate::manifest::ScannedFile {
        rel: "public.txt".to_owned(),
        absolute: controller_scratch.join("root/docs/public.txt"),
        root_rel: "docs/public.txt".to_owned(),
        size: 6,
        mtime_ms: 0,
        dev: 0,
        ino: 0,
    };
    let pin = controller.pin_file_root(0).unwrap();
    let err = controller.blob_offer_file_at(&host.endpoint_id(), 0, &dummy, u64::MAX, &pin);
    assert!(err.is_err());
    // Restore honest dir so temp cleanup works.
    std::fs::remove_file(controller_scratch.join("root/docs")).unwrap();
    std::fs::create_dir_all(controller_scratch.join("root/docs")).unwrap();
    std::fs::write(controller_scratch.join("root/docs/public.txt"), b"public").unwrap();
    let _ = &host_scratch;
    let _ = &host;
}

#[test]
fn sender_store_retention_matches_platform_import_mode() {
    // Senders reference their sources (TryReference) on every platform, so a
    // sender's store holds the entry's outboard but no payload copy. Checked
    // while each serve is parked: a terminal job releases its blobs to garbage
    // collection, after which a copy would be gone too and prove nothing.
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
    // (the store has the entry, a payload copy exists) for one hash. A
    // referenced entry keeps only metadata and its outboard, inline in
    // blobs.db; an owned copy would be data/<hash>.data, as receivers keep.
    let store_files = |core: &Core, blobs: &Path, bytes: &[u8]| {
        let hash = iroh_blobs::Hash::new(bytes);
        (
            core.blob_has(hash).unwrap(),
            blobs
                .join("data")
                .join(format!("{}.data", hash.to_hex()))
                .exists(),
        )
    };
    // Observed while the serve is parked; the serve is released before any
    // assertion, so a failure reports instead of leaving the transfer parked.
    let assert_referenced = |(entry, copy): (bool, bool), leg: &str| {
        assert!(entry, "{leg} sender has no store entry for its source");
        assert!(
            !copy,
            "{leg} sender must reference its source, not keep a payload copy"
        );
    };

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
    let probe = arm_serve_probe(*blake3::hash(&up_bytes).as_bytes());
    let (up_files, (up_sender, up_receiver)) = std::thread::scope(|scope| {
        let drive = scope.spawn(|| drive_to_end(&controller, up, &host, up_rx));
        let parked = probe.parked();
        let files = store_files(&controller, &controller_scratch.join("blobs"), &up_bytes);
        probe.release();
        assert!(parked, "the upload serve never parked");
        (files, drive.join().unwrap())
    });
    drop(probe);
    assert_referenced(up_files, "upload");
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
    let probe = arm_serve_probe(*blake3::hash(&down_bytes).as_bytes());
    let (down_files, (down_receiver, down_sender)) = std::thread::scope(|scope| {
        let drive = scope.spawn(|| drive_to_end(&controller, down, &host, host_down));
        let parked = probe.parked();
        let files = store_files(&host, &host_scratch.join("blobs"), &down_bytes);
        probe.release();
        assert!(parked, "the download serve never parked");
        (files, drive.join().unwrap())
    });
    drop(probe);
    assert_referenced(down_files, "download");
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
fn serve_refuses_a_source_replaced_after_its_offer() {
    // The sender references its source by path. A file renamed over that
    // path after the offer, with the same size and different bytes, must
    // fail serve-time verification rather than reach the receiver.
    let (host, controller, host_scratch, controller_scratch) = setup("replaced-after-offer");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let source = controller_scratch.join("root/source.bin");
    std::fs::write(&source, payload(64 * 1024, 1701)).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("source.bin"),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let decoy = payload(64 * 1024, 1702);
    let staged = controller_scratch.join("root/staged.bin");
    std::fs::write(&staged, &decoy).unwrap();
    std::fs::rename(&staged, &source).unwrap();

    let rx = receiver_id(&host, job);
    let (_sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_ne!(receiver.state, JobState::Complete);
    assert!(
        files_with_bytes(&host_scratch.join("root"), &decoy).is_empty(),
        "replacement bytes reached the receiver"
    );
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
    let hash = *blake3::hash(&original).as_bytes();

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

    let probe = arm_serve_probe(hash);
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

#[test]
fn source_reoffer_rejects_changed_bytes_before_authorizing_the_hash() {
    let (host, controller, _host_scratch, controller_scratch) = setup("reoffer-changed");
    let files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let original = b"original";
    let replacement = b"replaced";
    assert_eq!(original.len(), replacement.len());
    let path = controller_scratch.join("root/source.bin");
    let mtime_ms = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    };
    std::fs::write(&path, replacement).unwrap();
    let pin = controller.pin_file_root(files.root).unwrap();
    let expected = *blake3::hash(original).as_bytes();
    // Same size and the replacement's own mtime: only the recorded hash can
    // refuse the swapped bytes.
    assert!(controller
        .blob_reoffer_file_at(
            &host.endpoint_id(),
            files.root,
            &files.path("source.bin"),
            expected,
            original.len() as u64,
            mtime_ms(&path),
            original.len() as u64,
            &pin,
        )
        .is_err());
    assert!(!controller
        .blob_job_offer_contains(&host.endpoint_id(), blake3::hash(replacement).as_bytes(),));
    std::fs::write(&path, original).unwrap();
    let (hash, size) = controller
        .blob_reoffer_file_at(
            &host.endpoint_id(),
            files.root,
            &files.path("source.bin"),
            expected,
            original.len() as u64,
            mtime_ms(&path),
            original.len() as u64,
            &pin,
        )
        .unwrap();
    assert_eq!(hash.as_bytes(), &expected);
    assert_eq!(size, original.len() as u64);
}

#[test]
fn concurrent_jobs_sharing_content_preserve_access() {
    let (host, controller, host_scratch, controller_scratch) = setup("share-hash");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    // Same bytes in two distinct source files (same hash, different paths).
    let bytes = payload(1024, 303);
    std::fs::write(controller_scratch.join("root/a.txt"), &bytes).unwrap();
    std::fs::write(controller_scratch.join("root/b.txt"), &bytes).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job_a = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("a.txt"),
            host_files.root,
            &host_files.path("got-a"),
        )
        .unwrap();
    let job_b = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("b.txt"),
            host_files.root,
            &host_files.path("got-b"),
        )
        .unwrap();
    // Host adopts sender ids, so receiver ids equal sender ids.
    let rx_a = job_a;
    // Cancel A: B sharing the same hash must still complete (orphan check).
    controller.job_cancel(job_a).unwrap();
    let _ = host.job_poll(rx_a).unwrap();
    let rx_b = {
        let jobs = host.job_list().unwrap();
        jobs.into_iter()
            .find(|j| j.id != rx_a)
            .expect("second receiver job")
            .id
    };
    let (sender_b, receiver_b) = drive_to_end(&controller, job_b, &host, rx_b);
    assert_eq!(receiver_b.state, JobState::Complete);
    assert_eq!(sender_b.state, JobState::Complete);
    assert_eq!(
        std::fs::read(host_scratch.join("root/got-b/b.txt")).unwrap(),
        bytes
    );
}

#[test]
fn cross_filesystem_publication_succeeds() {
    // Staging (blob dir) and destination (file root) on different mounts when
    // available (/dev/shm tmpfs vs temp dir); otherwise same-filesystem rename
    // path still proves publication. EXDEV fallback (copy+atomic rename) is
    // exercised when mounts differ.
    let host = start_direct();
    let controller = start_direct();
    let host_scratch = Scratch::tagged("xfs-host");
    let controller_scratch = Scratch::tagged("xfs-controller");
    // Blob dirs: try /dev/shm for one side when usable (different mount).
    let shm = std::path::PathBuf::from("/dev/shm");
    // /dev/shm is a separate tmpfs only on Linux; elsewhere the path would be
    // created at the drive root. Windows cross-volume publication has its own test.
    let shm_usable = cfg!(target_os = "linux")
        && std::fs::create_dir_all(shm.join(format!(".vnilink-xfs-{}", std::process::id())))
            .and_then(|_| {
                std::fs::remove_dir(shm.join(format!(".vnilink-xfs-{}", std::process::id())))
            })
            .is_ok();
    // Host blobs on shm (if usable) to force cross-mount staging->dest when
    // host root stays on temp dir (different filesystems on Linux Docker).
    for (core, scratch) in [(&host, &host_scratch), (&controller, &controller_scratch)] {
        std::fs::create_dir_all(scratch.join("root")).unwrap();
        core.set_blob_directory(scratch.join("blobs")).unwrap();
        core.enable_filesystem_access().unwrap();
    }
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    // If shm usable, move controller blobs to shm (cross-mount for upload:
    // staging in controller blobs (shm)?? Actually staging is on receiver (host blobs).
    // For upload (controller sender, host receiver), staging on host blobs.
    // Put host blobs on shm to force cross-mount when host root on temp.
    if shm_usable {
        let shm_blobs = shm.join(format!(
            "vnilink-xfs-host-blobs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&shm_blobs).unwrap();
        host.set_blob_directory(&shm_blobs).unwrap();
        // Remember to clean shm dir on drop via scratch? Host scratch drop removes
        // its own temp blobs dir, not shm_blobs. Clean manually at end.
        std::fs::write(controller_scratch.join("root/a.txt"), payload(2048, 404)).unwrap();
        pair_with_transfer(&host, &controller);
        controller
            .set_desktop_sharing(crate::session::Grants {
                view: false,
                control: false,
                clipboard: false,
                transfer: true,
            })
            .unwrap();
        controller.connect(&host.endpoint_id()).unwrap();
        let job = controller
            .job_submit_upload(
                &host.endpoint_id(),
                controller_files.root,
                &controller_files.path("a.txt"),
                host_files.root,
                &host_files.path("got"),
            )
            .unwrap();
        let rx = receiver_id(&host, job);
        let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
        assert_eq!(receiver.state, JobState::Complete);
        assert_eq!(sender.state, JobState::Complete);
        assert_eq!(
            std::fs::read(host_scratch.join("root/got/a.txt")).unwrap(),
            payload(2048, 404)
        );
        let _ = std::fs::remove_dir_all(&shm_blobs);
    } else {
        std::fs::write(controller_scratch.join("root/a.txt"), payload(2048, 404)).unwrap();
        pair_with_transfer(&host, &controller);
        controller
            .set_desktop_sharing(crate::session::Grants {
                view: false,
                control: false,
                clipboard: false,
                transfer: true,
            })
            .unwrap();
        controller.connect(&host.endpoint_id()).unwrap();
        let job = controller
            .job_submit_upload(
                &host.endpoint_id(),
                controller_files.root,
                &controller_files.path("a.txt"),
                host_files.root,
                &host_files.path("got"),
            )
            .unwrap();
        let rx = receiver_id(&host, job);
        let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
        assert_eq!(receiver.state, JobState::Complete);
        assert_eq!(sender.state, JobState::Complete);
    }
}

#[test]
fn completed_and_forgotten_receivers_reclaim_payloads_without_deleting_destinations() {
    let _heavy = HEAVY_JOB
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let (host, controller, host_disk, controller_disk) = setup("reclaim");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &host_disk.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_disk.join("root"),
    );
    let bytes = payload(64 * 1024, 707);
    std::fs::write(controller_disk.join("root/payload.bin"), &bytes).unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let id = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("payload.bin"),
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    let hash = iroh_blobs::Hash::new(&bytes);
    let (_, received) = drive_to_end(&controller, id, &host, id);
    assert_eq!(received.state, JobState::Complete);
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while host.blob_verified(hash).unwrap().is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(host.blob_verified(hash).unwrap(), None);
    assert_eq!(
        std::fs::read(host_disk.join("root/payload.bin")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(controller_disk.join("root/payload.bin")).unwrap(),
        bytes
    );
    host.job_forget(id).unwrap();
    let second = payload(64 * 1024, 708);
    std::fs::write(controller_disk.join("root/partial.bin"), &second).unwrap();
    let id = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("partial.bin"),
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    let hash = iroh_blobs::Hash::new(&second);
    host.blob_fetch_streamed(&controller.endpoint_id(), hash, None, None)
        .unwrap();
    host.job_cancel(id).unwrap();
    assert!(host.blob_has(hash).unwrap());
    host.job_forget(id).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    while host.blob_verified(hash).unwrap().is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(host.blob_verified(hash).unwrap(), None);
    assert!(!host_disk.join("root/partial.bin").exists());
    host.shutdown();
    controller.shutdown();
    let reopened = start_direct();
    reopened
        .set_blob_directory(host_disk.join("blobs"))
        .unwrap();
    assert!(reopened
        .blob_export(hash, &host_disk.join("unexpected.bin"))
        .is_err());
    assert!(!host_disk.join("unexpected.bin").exists());
    assert_eq!(
        std::fs::read(host_disk.join("root/payload.bin")).unwrap(),
        bytes
    );
    reopened.shutdown();
}

#[test]
fn receiver_side_keep_both_unblocks_a_sender_already_mirroring_the_conflict() {
    let (host, controller, host_disk, controller_disk) = setup("receiver-choice");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &host_disk.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_disk.join("root"),
    );
    std::fs::write(controller_disk.join("root/file.txt"), b"incoming").unwrap();
    std::fs::write(host_disk.join("root/file.txt"), b"original").unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let id = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("file.txt"),
            host_files.root,
            &host_files.path(""),
        )
        .unwrap();
    assert_eq!(host.job_poll(id).unwrap().state, JobState::WaitingDecision);
    assert_eq!(
        controller.job_poll(id).unwrap().state,
        JobState::WaitingDecision
    );
    host.job_resolve(id, ConflictChoice::KeepBoth, false)
        .unwrap();
    let (sent, received) = drive_to_end(&controller, id, &host, id);
    assert_eq!(received.state, JobState::Complete);
    assert_eq!(sent.state, JobState::Complete);
    assert_eq!(
        std::fs::read(host_disk.join("root/file.txt")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(host_disk.join("root/file (2).txt")).unwrap(),
        b"incoming"
    );
    host.shutdown();
    controller.shutdown();
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

#[test]
fn service_jobs_complete_both_directions_and_keep_sources() {
    service_jobs_complete_both_directions(false);
}

#[test]
fn neutral_service_jobs_complete_both_directions_without_desktop() {
    service_jobs_complete_both_directions(true);
}

fn service_jobs_complete_both_directions(neutral: bool) {
    let (host, controller, hs, cs) = setup("owned-both");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let upload_bytes = payload(8192, 113);
    let download_bytes = payload(16384, 117);
    std::fs::write(cs.join("root/up.bin"), &upload_bytes).unwrap();
    std::fs::write(hs.join("root/down.bin"), &download_bytes).unwrap();
    let (owner, _pipe) = if neutral {
        host.set_service_sharing(crate::ServiceKind::Desktop, false)
            .unwrap();
        host.set_video_codecs(crate::VideoCodecCaps::default())
            .unwrap();
        controller
            .set_video_codecs(crate::VideoCodecCaps::default())
            .unwrap();
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        await_jobs(|| {
            controller.controller_session(pending.id).unwrap().phase == crate::SessionPhase::Active
        });
        let session = controller.controller_session(pending.id).unwrap();
        assert!(session.quality.is_none());
        let owner = controller
            .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
            .unwrap();
        let pipe = controller
            .open_files_service(owner.id, owner.generation)
            .unwrap();
        (owner, pipe)
    } else {
        owned_files(&controller, &host)
    };
    let up = controller
        .files_service_upload(
            owner.id,
            owner.generation,
            controller_files.root,
            &controller_files.path("up.bin"),
            host_files.root,
            &host_files.path("uploads"),
        )
        .unwrap();
    if !neutral {
        let desktop = controller
            .attach_controller_service(
                owner.connection_id,
                owner.generation,
                crate::ServiceKind::Desktop,
            )
            .unwrap();
        let handle = controller
            .open_desktop_service(desktop.id, desktop.generation)
            .unwrap();
        handle.close();
    }
    let rx = receiver_id(&host, up);
    let (sender, receiver) = drive_to_end(&controller, up, &host, rx);
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete)
    );
    let down = controller
        .files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            &host_files.path("down.bin"),
            controller_files.root,
            &controller_files.path("downloads"),
        )
        .unwrap();
    let tx = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|j| j.sender)
        .unwrap()
        .id;
    let (receiver, sender) = drive_to_end(&controller, down, &host, tx);
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(
        std::fs::read(hs.join("root/uploads/up.bin")).unwrap(),
        upload_bytes
    );
    assert_eq!(
        std::fs::read(cs.join("root/downloads/down.bin")).unwrap(),
        download_bytes
    );
    assert_eq!(std::fs::read(cs.join("root/up.bin")).unwrap(), upload_bytes);
    assert_eq!(
        std::fs::read(hs.join("root/down.bin")).unwrap(),
        download_bytes
    );
    assert_eq!(
        controller
            .files_service_jobs(owner.id, owner.generation)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn service_jobs_close_preserves_legacy_job_sharing_the_same_hash() {
    let (host, controller, hs, cs) = setup("owned-shared");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let bytes = payload(32768, 123);
    std::fs::write(cs.join("root/same.bin"), &bytes).unwrap();
    let (owner, pipe) = owned_files(&controller, &host);
    let desktop = controller
        .attach_controller_service(
            owner.connection_id,
            owner.generation,
            crate::ServiceKind::Desktop,
        )
        .unwrap();
    let scoped = controller
        .files_service_upload(
            owner.id,
            owner.generation,
            controller_files.root,
            &controller_files.path("same.bin"),
            host_files.root,
            &host_files.path("cancelled"),
        )
        .unwrap();
    let scoped_rx = host.job_list().unwrap()[0].id;
    let legacy = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path("same.bin"),
            host_files.root,
            &host_files.path("kept"),
        )
        .unwrap();
    let legacy_rx = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|j| j.id != scoped_rx)
        .unwrap()
        .id;
    controller
        .close_controller_service(owner.id, owner.generation)
        .unwrap();
    assert_eq!(
        controller.job_snapshot(scoped).unwrap().state,
        JobState::Cancelled
    );
    await_jobs(|| host.job_snapshot(scoped_rx).unwrap().state == JobState::Cancelled);
    assert!(pipe.is_cancelled());
    assert!(matches!(
        controller.job_resume(scoped),
        Err(Error::NoSession)
    ));
    assert!(matches!(host.job_resume(scoped_rx), Err(Error::NoSession)));
    let (sender, receiver) = drive_to_end(&controller, legacy, &host, legacy_rx);
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(std::fs::read(hs.join("root/kept/same.bin")).unwrap(), bytes);
    assert!(!hs.join("root/cancelled").exists());
    assert!(!controller
        .service_lease(desktop.id, desktop.generation)
        .unwrap()
        .is_cancelled());
    assert_eq!(
        controller
            .controller_session(owner.connection_id)
            .unwrap()
            .phase,
        crate::SessionPhase::Active
    );
}

#[test]
fn service_jobs_host_close_stops_download_and_rejects_new_work() {
    let (host, controller, hs, cs) = setup("owned-host-close");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    std::fs::write(hs.join("root/a.bin"), payload(8192, 137)).unwrap();
    let (owner, pipe) = owned_files(&controller, &host);
    let job = controller
        .files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            &host_files.path("a.bin"),
            controller_files.root,
            &controller_files.path("cancelled"),
        )
        .unwrap();
    let tx = host.job_list().unwrap()[0].id;
    let hosted = host.hosted_services().unwrap()[0].clone();
    host.close_hosted_service(hosted.id, hosted.generation)
        .unwrap();
    assert_eq!(host.job_snapshot(tx).unwrap().state, JobState::Cancelled);
    await_jobs(|| pipe.is_cancelled());
    await_jobs(|| controller.job_snapshot(job).unwrap().state == JobState::Cancelled);
    assert!(matches!(
        controller.files_service_download(
            owner.id,
            owner.generation,
            host_files.root,
            &host_files.path("a.bin"),
            controller_files.root,
            &controller_files.path("later")
        ),
        Err(Error::NoSession)
    ));
    assert!(!cs.join("root/cancelled").exists());
    assert_eq!(
        controller
            .controller_session(owner.connection_id)
            .unwrap()
            .phase,
        crate::SessionPhase::Active
    );
}

#[test]
fn service_jobs_require_peer_admission_and_explicit_adoption_after_restart() {
    let (host, controller, _hs, cs) = setup("owned-admit-restart");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &_hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    std::fs::write(cs.join("root/a.bin"), payload(4096, 139)).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
        .unwrap();
    assert!(controller
        .files_service_upload(
            owner.id,
            owner.generation,
            controller_files.root,
            &controller_files.path("a.bin"),
            host_files.root,
            &host_files.path("refused")
        )
        .is_err());
    assert!(host.job_list().unwrap().is_empty());
    assert!(controller.job_list().unwrap().is_empty());
    // An ambiguous remote refusal ends only this owner, preventing orphaned
    // admitted jobs if a reply were lost. Open a fresh owner for the retry.
    if !controller
        .controller_service(owner.id)
        .unwrap()
        .phase
        .finished()
    {
        controller
            .close_controller_service(owner.id, owner.generation)
            .unwrap();
    }
    let owner = controller
        .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(owner.id, owner.generation)
        .unwrap();
    let job = controller
        .files_service_upload(
            owner.id,
            owner.generation,
            controller_files.root,
            &controller_files.path("a.bin"),
            host_files.root,
            &host_files.path("cancelled"),
        )
        .unwrap();
    pipe.close();
    await_jobs(|| controller.job_snapshot(job).unwrap().state == JobState::Cancelled);
    drop(pipe);
    controller.shutdown();
    drop(controller);
    let restarted = start_direct();
    restarted.set_blob_directory(cs.join("blobs")).unwrap();
    assert_eq!(
        restarted.job_snapshot(job).unwrap().state,
        JobState::Cancelled
    );
    assert!(matches!(restarted.job_resume(job), Err(Error::NoSession)));
}

#[test]
fn service_jobs_closing_one_computer_keeps_another_transfer_active() {
    let (first, controller, first_disk, cs) = setup("owned-multiple");
    let host_files = FilesystemLocation::new(first.file_roots().unwrap(), &first_disk.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let second = start_direct();
    let second_disk = Scratch::tagged("owned-second-host");
    std::fs::create_dir(second_disk.join("root")).unwrap();
    second
        .set_blob_directory(second_disk.join("blobs"))
        .unwrap();
    second.enable_filesystem_access().unwrap();
    let second_files =
        FilesystemLocation::new(second.file_roots().unwrap(), &second_disk.join("root"));
    pair_with_transfer(&second, &controller);
    controller
        .set_desktop_sharing(crate::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    let bytes = payload(65536, 149);
    std::fs::write(cs.join("root/shared.bin"), &bytes).unwrap();
    let (one, _one_pipe) = owned_files(&controller, &first);
    let (two, _two_pipe) = owned_files(&controller, &second);
    let cancelled = controller
        .files_service_upload(
            one.id,
            one.generation,
            controller_files.root,
            &controller_files.path("shared.bin"),
            host_files.root,
            &host_files.path("cancelled"),
        )
        .unwrap();
    let kept = controller
        .files_service_upload(
            two.id,
            two.generation,
            controller_files.root,
            &controller_files.path("shared.bin"),
            second_files.root,
            &second_files.path("kept"),
        )
        .unwrap();
    // A tab cannot use another tab's job ID, even though both jobs live
    // in the same engine and share the same source hash.
    let before = controller.job_snapshot(kept).unwrap();
    assert_eq!(
        controller
            .files_service_jobs(one.id, one.generation)
            .unwrap()
            .iter()
            .map(|j| j.id)
            .collect::<Vec<_>>(),
        vec![cancelled]
    );
    assert_eq!(
        controller
            .files_service_jobs(two.id, two.generation)
            .unwrap()
            .iter()
            .map(|j| j.id)
            .collect::<Vec<_>>(),
        vec![kept]
    );
    assert!(controller
        .files_service_job_snapshot(one.id, one.generation, kept)
        .is_err());
    assert!(controller
        .files_service_job_poll(one.id, one.generation, kept)
        .is_err());
    assert!(controller
        .files_service_job_cancel(one.id, one.generation, kept)
        .is_err());
    assert!(controller
        .files_service_job_resume(one.id, one.generation, kept)
        .is_err());
    assert!(controller
        .files_service_job_resolve(
            one.id,
            one.generation,
            kept,
            crate::ConflictChoice::Skip,
            true
        )
        .is_err());
    assert!(controller
        .files_service_job_forget(one.id, one.generation, kept)
        .is_err());
    assert!(matches!(
        controller.files_service_job_snapshot(two.id, two.generation + 1, kept),
        Err(Error::StaleSession)
    ));
    assert_eq!(controller.job_snapshot(kept).unwrap(), before);
    let second_job = second.job_list().unwrap()[0].id;
    controller
        .close_controller_service(one.id, one.generation)
        .unwrap();
    assert_eq!(
        controller.job_snapshot(cancelled).unwrap().state,
        JobState::Cancelled
    );
    assert_eq!(
        controller
            .files_service_job_snapshot(one.id, one.generation, cancelled)
            .unwrap()
            .state,
        JobState::Cancelled
    );
    assert!(controller
        .files_service_job_resume(one.id, one.generation, cancelled)
        .is_err());
    controller
        .files_service_job_forget(one.id, one.generation, cancelled)
        .unwrap();
    assert!(controller
        .files_service_jobs(one.id, one.generation)
        .unwrap()
        .is_empty());
    let (sent, received) = drive_to_end(&controller, kept, &second, second_job);
    assert_eq!(
        (sent.state, received.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(
        std::fs::read(second_disk.join("root/kept/shared.bin")).unwrap(),
        bytes
    );
    assert!(!first_disk.join("root/cancelled").exists());
    assert!(!controller
        .service_lease(two.id, two.generation)
        .unwrap()
        .is_cancelled());
}

#[test]
fn files_job_explicit_adoption_rebinds_both_sides_and_fences_old_handle() {
    let (host, controller, host_scratch, controller_scratch) = setup("adopt-job");
    let host_files =
        FilesystemLocation::new(host.file_roots().unwrap(), &host_scratch.join("root"));
    let controller_files = FilesystemLocation::new(
        controller.file_roots().unwrap(),
        &controller_scratch.join("root"),
    );
    let bytes = payload(1024 * 1024, 903);
    std::fs::write(controller_scratch.join("root/adopt.bin"), &bytes).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    let scoped_files = |core: &Core, session: &crate::SessionInfo| {
        let owner = core
            .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
            .unwrap();
        let pipe = core.open_files_service(owner.id, owner.generation).unwrap();
        (owner, pipe)
    };
    let (old, pipe) = scoped_files(&controller, &session);
    let job = controller
        .files_service_upload(
            old.id,
            old.generation,
            controller_files.root,
            &controller_files.path("adopt.bin"),
            host_files.root,
            &host_files.path("adopted"),
        )
        .unwrap();
    let receiver = host.job_list().unwrap()[0].id;
    controller
        .close_controller_service(old.id, old.generation)
        .unwrap();
    drop(pipe);
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host.hosted_services().unwrap().is_empty() {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    let (fresh, _pipe) = scoped_files(&controller, &session);
    assert_eq!(
        controller
            .files_service_adopt_job(fresh.id, fresh.generation, job)
            .unwrap()
            .state,
        JobState::Cancelled
    );
    assert!(controller
        .files_service_job_resume(old.id, old.generation, job)
        .is_err());
    assert!(controller
        .files_service_job_forget(old.id, old.generation, job)
        .is_err());
    assert!(controller
        .files_service_jobs(old.id, old.generation)
        .unwrap()
        .is_empty());
    assert_eq!(
        controller
            .files_service_jobs(fresh.id, fresh.generation)
            .unwrap()
            .len(),
        1
    );
    controller
        .files_service_job_resume(fresh.id, fresh.generation, job)
        .unwrap();
    let (sender, receiver) = drive_to_end(&controller, job, &host, receiver);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(
        std::fs::read(host_scratch.join("root/adopted/adopt.bin")).unwrap(),
        bytes
    );
}

#[test]
fn files_download_adoption_after_parent_replacement_preserves_verified_progress() {
    let (host, controller, hs, cs) = setup("adopt-download-reconnect");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    std::fs::create_dir_all(hs.join("root/source")).unwrap();
    let first = payload(65536, 912);
    let second = payload(131072, 913);
    std::fs::write(hs.join("root/source/a.bin"), &first).unwrap();
    std::fs::write(hs.join("root/source/b.bin"), &second).unwrap();
    let connect = || {
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let session = controller.controller_session(pending.id).unwrap();
            if session.phase == crate::SessionPhase::Active {
                break session;
            }
            assert!(std::time::Instant::now() < end);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let session = connect();
    let old = controller
        .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
        .unwrap();
    let pipe = controller
        .open_files_service(old.id, old.generation)
        .unwrap();
    let job = controller
        .files_service_download(
            old.id,
            old.generation,
            host_files.root,
            &host_files.path("source"),
            controller_files.root,
            &controller_files.path("received"),
        )
        .unwrap();
    let sender = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|j| j.sender)
        .unwrap()
        .id;
    let progress = controller
        .files_service_job_poll(old.id, old.generation, job)
        .unwrap();
    assert_eq!(progress.done_bytes, first.len() as u64);
    assert_ne!(progress.state, JobState::Complete);
    assert_eq!(
        std::fs::read(cs.join("root/received/source/a.bin")).unwrap(),
        first
    );
    controller.disconnect_session(session.id).unwrap();
    drop(pipe);
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !host.hosted_services().unwrap().is_empty() {
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Stream retirement on the host and parent retirement on the controller
    // are independent observations. A Stopping parent still reserves its peer.
    loop {
        match controller.controller_session(session.id) {
            Ok(info)
                if matches!(
                    info.phase,
                    crate::SessionPhase::Closed | crate::SessionPhase::Failed(_)
                ) =>
            {
                break
            }
            Err(Error::NoSession) => break,
            Ok(_) => {}
            Err(error) => panic!("parent retirement failed: {error}"),
        }
        assert!(
            std::time::Instant::now() < end,
            "old controller parent remained live"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let fresh_session = connect();
    let fresh = controller
        .attach_controller_service(
            fresh_session.id,
            fresh_session.generation,
            crate::ServiceKind::Files,
        )
        .unwrap();
    let _pipe = controller
        .open_files_service(fresh.id, fresh.generation)
        .unwrap();
    let adopted = controller
        .files_service_adopt_job(fresh.id, fresh.generation, job)
        .unwrap();
    assert_eq!(adopted.done_bytes, progress.done_bytes);
    assert_eq!(adopted.id, job);
    assert!(controller
        .files_service_job_resume(old.id, old.generation, job)
        .is_err());
    controller
        .files_service_job_resume(fresh.id, fresh.generation, job)
        .unwrap();
    let (receiver, sender) = drive_to_end(&controller, job, &host, sender);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(
        std::fs::read(cs.join("root/received/source/a.bin")).unwrap(),
        first
    );
    assert_eq!(
        std::fs::read(cs.join("root/received/source/b.bin")).unwrap(),
        second
    );
}

#[test]
fn files_adoption_survives_both_core_restarts_with_verified_download() {
    let (host, controller, hs, cs) = setup("adopt-both-restart");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    std::fs::create_dir(hs.join("root/pack")).unwrap();
    let first = payload(32768, 940);
    let second = payload(65536, 941);
    std::fs::write(hs.join("root/pack/a.bin"), &first).unwrap();
    std::fs::write(hs.join("root/pack/b.bin"), &second).unwrap();
    let connect_files = |controller: &Core, host: &Core| {
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        await_jobs(|| {
            controller.controller_session(pending.id).unwrap().phase == crate::SessionPhase::Active
        });
        let session = controller.controller_session(pending.id).unwrap();
        let owner = controller
            .attach_controller_service(session.id, session.generation, crate::ServiceKind::Files)
            .unwrap();
        let pipe = controller
            .open_files_service(owner.id, owner.generation)
            .unwrap();
        (owner, pipe)
    };
    let (old, pipe) = connect_files(&controller, &host);
    let job = controller
        .files_service_download(
            old.id,
            old.generation,
            host_files.root,
            &host_files.path("pack"),
            controller_files.root,
            &controller_files.path("received"),
        )
        .unwrap();
    let peer_job = host
        .job_list()
        .unwrap()
        .into_iter()
        .find(|j| j.sender)
        .unwrap()
        .id;
    assert_eq!(
        controller
            .files_service_job_poll(old.id, old.generation, job)
            .unwrap()
            .files_done,
        1
    );
    let host_key = host.export_identity();
    let controller_key = controller.export_identity();
    controller.shutdown();
    host.shutdown();
    drop(pipe);
    drop(controller);
    drop(host);
    let reopen = |key, scratch: &Scratch| {
        let core = Core::boot(
            CoreConfig {
                identity: Some(key),
                listen: Listen::Loopback,
                ..CoreConfig::default()
            },
            RelayMode::Disabled,
        )
        .unwrap();
        core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
            .unwrap();
        core.set_blob_directory(scratch.join("blobs")).unwrap();
        core.enable_filesystem_access().unwrap();
        core
    };
    let host = reopen(host_key, &hs);
    let controller = reopen(controller_key, &cs);
    pair_with_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    let (fresh, _pipe) = connect_files(&controller, &host);
    assert!(controller
        .files_service_jobs(fresh.id, fresh.generation)
        .unwrap()
        .is_empty());
    let adopted = controller
        .files_service_adopt_job(fresh.id, fresh.generation, job)
        .unwrap();
    assert_eq!(adopted.files_done, 1);
    assert_eq!(adopted.done_bytes, first.len() as u64);
    controller
        .files_service_job_resume(fresh.id, fresh.generation, job)
        .unwrap();
    let (receiver, sender) = drive_to_end(&controller, job, &host, peer_job);
    assert_eq!(
        (receiver.state, sender.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(
        std::fs::read(cs.join("root/received/pack/a.bin")).unwrap(),
        first
    );
    assert_eq!(
        std::fs::read(cs.join("root/received/pack/b.bin")).unwrap(),
        second
    );
}

#[test]
fn files_adoption_local_save_failure_can_retry_remote_acknowledged_binding() {
    let (host, controller, hs, cs) = setup("adopt-save-retry");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let expected = payload(65536, 942);
    std::fs::write(cs.join("root/source.bin"), &expected).unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
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
    let (old, pipe) = open();
    let job = controller
        .files_service_upload(
            old.id,
            old.generation,
            controller_files.root,
            &controller_files.path("source.bin"),
            host_files.root,
            &host_files.path("received"),
        )
        .unwrap();
    let peer_job = host.job_list().unwrap()[0].id;
    controller
        .close_controller_service(old.id, old.generation)
        .unwrap();
    drop(pipe);
    await_jobs(|| host.hosted_services().unwrap().is_empty());
    let (fresh, _pipe) = open();
    // An existing directory refuses atomic replacement on every target without
    // relying on permissions (the Docker validation account may be root).
    let index = cs.join("blobs/jobs.json");
    let saved = cs.join("blobs/jobs.saved");
    std::fs::rename(&index, &saved).unwrap();
    std::fs::create_dir(&index).unwrap();
    assert!(controller
        .files_service_adopt_job(fresh.id, fresh.generation, job)
        .is_err());
    assert!(controller
        .files_service_jobs(fresh.id, fresh.generation)
        .unwrap()
        .is_empty());
    assert!(controller
        .files_service_job_resume(old.id, old.generation, job)
        .is_err());
    std::fs::remove_dir(&index).unwrap();
    std::fs::rename(&saved, &index).unwrap();
    let adopted = controller
        .files_service_adopt_job(fresh.id, fresh.generation, job)
        .unwrap();
    assert_eq!(adopted.state, JobState::Cancelled);
    controller
        .files_service_job_resume(fresh.id, fresh.generation, job)
        .unwrap();
    let (sender, receiver) = drive_to_end(&controller, job, &host, peer_job);
    assert_eq!(
        (sender.state, receiver.state),
        (JobState::Complete, JobState::Complete)
    );
    assert_eq!(
        std::fs::read(hs.join("root/received/source.bin")).unwrap(),
        expected
    );
}

#[test]
#[cfg(unix)]
fn resume_reoffer_enforces_size_limit_before_import_and_accepts_exact_boundary() {
    let (host, controller, _host_scratch, controller_scratch) = setup_confined("resume-limit");
    let bytes = payload(64 * 1024, 99);
    let path = controller_scratch.join("root/resume.bin");
    std::fs::write(&path, &bytes).unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let mtime = metadata
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let hash = *blake3::hash(&bytes).as_bytes();
    let pin = controller.pin_file_root(0).unwrap();
    assert!(matches!(
        controller.blob_reoffer_file_at(
            &host.endpoint_id(),
            0,
            "resume.bin",
            hash,
            bytes.len() as u64,
            mtime,
            bytes.len() as u64 - 1,
            &pin,
        ),
        Err(Error::InvalidSetting("the source file is too large"))
    ));
    // Rejection does not mutate the source, and a limit equal to the actual
    // verified size can still import a reference and return the expected hash.
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let (offered, size) = controller
        .blob_reoffer_file_at(
            &host.endpoint_id(),
            0,
            "resume.bin",
            hash,
            bytes.len() as u64,
            mtime,
            bytes.len() as u64,
            &pin,
        )
        .unwrap();
    assert_eq!(offered.as_bytes(), &hash);
    assert_eq!(size, bytes.len() as u64);
}

#[test]
fn receiver_resource_budget_refuses_before_fetch_and_preserves_source_and_destination() {
    let (host, controller, hs, cs) = setup("resource-admission");
    let host_files = FilesystemLocation::new(host.file_roots().unwrap(), &hs.join("root"));
    let controller_files =
        FilesystemLocation::new(controller.file_roots().unwrap(), &cs.join("root"));
    let bytes = payload(64 * 1024, 314);
    std::fs::write(cs.join("root/send.bin"), &bytes).unwrap();
    let hash = iroh_blobs::Hash::new(&bytes);
    host.set_file_resource_policy(crate::job::FileResourcePolicy {
        max_concurrent_receivers: 1,
        max_reserved_bytes: 3 * bytes.len() as u64 - 1,
        minimum_free_bytes: 0,
    })
    .unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let sent = controller
        .job_submit_upload(
            &host.endpoint_id(),
            controller_files.root,
            &controller_files.path(""),
            host_files.root,
            &host_files.path("arrived"),
        )
        .unwrap();
    let rx = receiver_id(&host, sent);
    let failed = host.job_poll(rx).unwrap();
    assert_eq!(failed.state, JobState::Failed);
    assert_eq!(
        failed.error.as_deref(),
        Some("file exceeds the receiver resource budget")
    );
    assert_eq!(failed.done_bytes, 0);
    assert!(!host.blob_has(hash).unwrap());
    assert!(!hs.join("root/arrived/root/send.bin").exists());
    assert_eq!(std::fs::read(cs.join("root/send.bin")).unwrap(), bytes);
    let sender = controller.job_poll(sent).unwrap();
    assert_eq!(sender.state, JobState::Failed);
    assert!(host
        .blob_fetch_streamed(&controller.endpoint_id(), hash, None, None)
        .is_err());
}

#[test]
#[cfg(unix)]
fn restart_reconciles_managed_orphans_but_keeps_interrupted_staging() {
    let (host, controller, hs, cs) = setup_confined("stage-crash");
    std::fs::write(cs.join("root/source.bin"), b"source survives crash").unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let sent = controller
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "received")
        .unwrap();
    let rx = receiver_id(&host, sent);
    host.shutdown();
    controller.shutdown();
    drop(host);
    drop(controller);
    let staged = hs.join(&format!("blobs/staging/job-{rx}"));
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("verified"), b"resume data").unwrap();
    let orphan = hs.join("blobs/staging/job-18446744073709551615");
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("unused"), b"orphan").unwrap();
    std::fs::write(hs.join("blobs/jobs.json.tmp"), b"incomplete atomic save").unwrap();
    let reopened = start_direct();
    reopened.set_blob_directory(hs.join("blobs")).unwrap();
    assert_eq!(
        reopened.job_snapshot(rx).unwrap().state,
        JobState::Interrupted
    );
    assert_eq!(
        std::fs::read(staged.join("verified")).unwrap(),
        b"resume data"
    );
    assert!(!orphan.exists());
    assert!(!hs.join("blobs/jobs.json.tmp").exists());
    assert_eq!(
        std::fs::read(cs.join("root/source.bin")).unwrap(),
        b"source survives crash"
    );
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

/// Polls until the receiver interrupts. A rebound leg surfaces as a
/// snapshot, never as a poll error, so the job stays resumable.
fn poll_until_interrupted(receiver: &Core, job: u64) {
    for _ in 0..10 {
        let snap = receiver.job_poll(job).unwrap();
        if snap.state == JobState::Interrupted {
            return;
        }
        assert!(
            !snap.state.is_terminal(),
            "job ended without interrupting: {:?}",
            snap.state
        );
    }
    panic!("receiver never interrupted");
}

#[test]
#[cfg(unix)]
fn rebound_source_interrupts_and_restores() {
    // The controller uploads: the dialed side cannot rebind mid-session
    // (Busy), so the sender leg under test is the dialer's own.
    let (host, controller, host_scratch, controller_scratch) = setup_confined("rebind-src");
    let controller_root = controller_scratch.join("root");
    let one = payload(512, 1603);
    let two = payload(512, 1604);
    std::fs::write(controller_root.join("one.txt"), &one).unwrap();
    std::fs::write(controller_root.join("two.txt"), &two).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_upload(&host.endpoint_id(), 0, "", 0, "arrived")
        .unwrap();
    let rx = receiver_id(&host, job);
    poll_until_files_done(&host, rx, 1);
    let arrived = host_scratch.join("root/arrived");
    assert_eq!(std::fs::read(arrived.join("one.txt")).unwrap(), one);

    // Rebind the sender to identical copies: content checks alone would
    // pass, so only the leg pin can stop the second file.
    let rebound = controller_scratch.join("rebound");
    std::fs::create_dir_all(&rebound).unwrap();
    std::fs::write(rebound.join("one.txt"), &one).unwrap();
    std::fs::write(rebound.join("two.txt"), &two).unwrap();
    controller.set_file_roots(vec![rebound]).unwrap();

    poll_until_interrupted(&host, rx);
    assert!(
        !arrived.join("two.txt").exists(),
        "rebound bytes must not land"
    );
    // The sender mirrors receiver state only when polled; nothing here
    // polls it, so it still shows the pre-rebind state.
    assert_eq!(
        controller.job_snapshot(job).unwrap().state,
        JobState::Serving
    );

    // Restoring the original roots resumes with the original bytes: the
    // epoch advanced twice but the pinned identity matches again.
    controller.set_file_roots(vec![controller_root]).unwrap();
    host.job_resume(rx).unwrap();
    let (sender, receiver) = drive_to_end(&controller, job, &host, rx);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(std::fs::read(arrived.join("two.txt")).unwrap(), two);
}

#[test]
#[cfg(unix)]
fn rebound_destination_interrupts_without_writing() {
    let (host, controller, _host_scratch, controller_scratch) = setup_confined("rebind-dst");
    let host_root = _host_scratch.join("root");
    let one = payload(512, 1605);
    let two = payload(512, 1606);
    std::fs::write(host_root.join("one.txt"), &one).unwrap();
    std::fs::write(host_root.join("two.txt"), &two).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    controller.create_dir(0, "", "got").unwrap();
    let job = controller
        .job_submit_download(&host.endpoint_id(), 0, "", 0, "got")
        .unwrap();
    poll_until_files_done(&controller, job, 1);
    let host_job = host.job_list().unwrap()[0].id;
    let got = controller_scratch.join("root/got");
    assert_eq!(std::fs::read(got.join("one.txt")).unwrap(), one);

    // Rebind the receiver to an empty directory mid-transfer: the
    // conflict check precedes the fetch, so no fetch runs and no
    // skeleton or file is created under the new roots.
    let rebound = controller_scratch.join("rebound");
    std::fs::create_dir_all(&rebound).unwrap();
    controller.set_file_roots(vec![rebound.clone()]).unwrap();

    poll_until_interrupted(&controller, job);
    assert_eq!(
        std::fs::read_dir(&rebound).unwrap().count(),
        0,
        "rebound directory must stay empty"
    );
    assert!(!got.join("two.txt").exists(), "second file must not land");

    controller
        .set_file_roots(vec![controller_scratch.join("root")])
        .unwrap();
    controller.job_resume(job).unwrap();
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(std::fs::read(got.join("two.txt")).unwrap(), two);
}

#[test]
#[cfg(unix)]
fn replace_approval_stays_bound_to_original_across_rebind() {
    let (host, controller, host_scratch, controller_scratch) = setup_confined("rebind-choice");
    let remote = payload(512, 1610);
    std::fs::write(host_scratch.join("root/one.txt"), &remote).unwrap();
    let got = controller_scratch.join("root/got");
    std::fs::create_dir_all(&got).unwrap();
    std::fs::write(got.join("one.txt"), b"original-local-content").unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_download(&host.endpoint_id(), 0, "", 0, "got")
        .unwrap();
    // Conflict waits; approve Replace for the ORIGINAL destination.
    let waiting = controller.job_poll(job).unwrap();
    assert_eq!(waiting.state, JobState::WaitingDecision);
    controller
        .job_resolve(job, ConflictChoice::Replace, false)
        .unwrap();

    // Rebind the receiver to a directory holding a valuable same-name file.
    let rebound = controller_scratch.join("rebound");
    std::fs::create_dir_all(rebound.join("got")).unwrap();
    let valuable = payload(512, 1611);
    std::fs::write(rebound.join("got/one.txt"), &valuable).unwrap();
    controller.set_file_roots(vec![rebound.clone()]).unwrap();

    // The stored Replace cannot fire under the new roots: neither the
    // valuable file nor the original conflict target is touched.
    poll_until_interrupted(&controller, job);
    assert_eq!(
        std::fs::read(rebound.join("got/one.txt")).unwrap(),
        valuable
    );
    assert_eq!(
        std::fs::read(got.join("one.txt")).unwrap(),
        b"original-local-content"
    );

    // Restore: the old approval applies to the original object only.
    controller
        .set_file_roots(vec![controller_scratch.join("root")])
        .unwrap();
    controller.job_resume(job).unwrap();
    let host_job = host.job_list().unwrap()[0].id;
    let (receiver, sender) = drive_to_end(&controller, job, &host, host_job);
    assert_eq!(receiver.state, JobState::Complete);
    assert_eq!(sender.state, JobState::Complete);
    assert_eq!(std::fs::read(got.join("one.txt")).unwrap(), remote);
    assert_eq!(
        std::fs::read(rebound.join("got/one.txt")).unwrap(),
        valuable
    );
}

#[test]
#[cfg(unix)]
fn persisted_replace_refuses_replaced_root_at_same_path() {
    let (host, controller, host_scratch, controller_scratch) = setup_confined("same-path-replace");
    std::fs::write(host_scratch.join("root/one.txt"), payload(512, 1701)).unwrap();
    let root = controller_scratch.join("root");
    std::fs::create_dir_all(root.join("got")).unwrap();
    std::fs::write(root.join("got/one.txt"), b"original").unwrap();
    controller.connect(&host.endpoint_id()).unwrap();
    let job = controller
        .job_submit_download(&host.endpoint_id(), 0, "", 0, "got")
        .unwrap();
    assert_eq!(
        controller.job_poll(job).unwrap().state,
        JobState::WaitingDecision
    );
    controller
        .job_resolve(job, ConflictChoice::Replace, false)
        .unwrap();
    let secret = controller.export_identity();
    controller.shutdown();
    drop(controller);
    std::fs::rename(&root, controller_scratch.join("original-root")).unwrap();
    std::fs::create_dir_all(root.join("got")).unwrap();
    std::fs::write(root.join("got/one.txt"), b"valuable-replacement").unwrap();
    let controller = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            identity: Some(secret),
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    controller
        .set_blob_directory(controller_scratch.join("blobs"))
        .unwrap();
    controller.set_file_roots(vec![root.clone()]).unwrap();
    pair_with_transfer(&host, &controller);
    controller
        .set_desktop_sharing(crate::session::Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: true,
        })
        .unwrap();
    controller.connect_files(&host.endpoint_id()).unwrap();
    controller.job_resume(job).unwrap();
    let resumed = controller.job_poll(job).unwrap();
    assert_eq!(resumed.state, JobState::Interrupted);
    assert_eq!(
        std::fs::read(root.join("got/one.txt")).unwrap(),
        b"valuable-replacement"
    );
    assert_eq!(
        std::fs::read(controller_scratch.join("original-root/got/one.txt")).unwrap(),
        b"original"
    );
}

#[test]
#[cfg(unix)]
fn resume_after_source_rebind_fails_with_rebound_cause() {
    let (host, controller, host_scratch, controller_scratch) = setup_confined("rebind-resume");
    let host_root = host_scratch.join("root");
    let one = payload(512, 1607);
    let two = payload(512, 1608);
    std::fs::write(host_root.join("one.txt"), &one).unwrap();
    std::fs::write(host_root.join("two.txt"), &two).unwrap();

    controller.connect(&host.endpoint_id()).unwrap();
    controller.create_dir(0, "", "got").unwrap();
    let job = controller
        .job_submit_download(&host.endpoint_id(), 0, "", 0, "got")
        .unwrap();
    poll_until_files_done(&controller, job, 1);
    let sender_job = host.job_list().unwrap()[0].id;

    // Kill the sender; reopen the same installation with the source
    // root rebound to identical copies. The persisted pin survives the
    // restart, so only it can fail the resume.
    let secret = host.export_identity();
    let blob_dir = host_scratch.join("blobs");
    host.shutdown();
    drop(host);
    let rebound = host_scratch.join("rebound");
    std::fs::create_dir_all(&rebound).unwrap();
    std::fs::write(rebound.join("one.txt"), &one).unwrap();
    std::fs::write(rebound.join("two.txt"), &two).unwrap();
    let host = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            identity: Some(secret),
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    host.set_blob_directory(&blob_dir).unwrap();
    host.set_file_roots(vec![rebound]).unwrap();

    let jobs = host.job_list().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, JobState::Interrupted);
    let resumed = host.job_resume(sender_job).unwrap();
    assert_eq!(resumed.state, JobState::Failed);
    assert_eq!(resumed.error.as_deref(), Some(crate::fs::ROOTS_REBOUND));
    let got = controller_scratch.join("root/got");
    assert_eq!(std::fs::read(got.join("one.txt")).unwrap(), one);
    assert!(!got.join("two.txt").exists(), "rebound bytes must not land");
    let _ = job;
}

/// Target hash plus hit/release barriers for the in-flight serve probe.
/// The probe parks only the armed hash; every other serve passes
/// straight through, so parallel suites are unaffected.
static REBIND_TARGET: std::sync::Mutex<Option<[u8; 32]>> = std::sync::Mutex::new(None);
static REBIND_HIT: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());
static REBIND_RELEASE: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());

fn rebind_serve_probe(hash: [u8; 32]) {
    if *REBIND_TARGET.lock().unwrap() != Some(hash) {
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

/// Parks serves of one hash until released, and disarms the probe even when
/// the test panics. Payload seeds are unique per test, so a leaked arm could
/// never match another test's content hash anyway.
struct RebindProbeGuard {
    _owner: std::sync::MutexGuard<'static, ()>,
}

fn arm_serve_probe(hash: [u8; 32]) -> RebindProbeGuard {
    let owner = SERVE_PROBE_OWNER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *REBIND_TARGET.lock().unwrap() = Some(hash);
    *REBIND_HIT.0.lock().unwrap() = false;
    *REBIND_RELEASE.0.lock().unwrap() = false;
    Core::set_serve_source_probe(Some(rebind_serve_probe));
    RebindProbeGuard { _owner: owner }
}

impl RebindProbeGuard {
    /// Waits up to 30 s for the armed hash's serve to open its verified
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
    let controller_root = controller_scratch.join("root");
    let one = payload(512, 1601);
    let two = payload(512, 1602);
    std::fs::write(controller_root.join("one.txt"), &one).unwrap();
    std::fs::write(controller_root.join("two.txt"), &two).unwrap();
    let two_hash = *blake3::hash(&two).as_bytes();

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

    let probe = arm_serve_probe(two_hash);
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
