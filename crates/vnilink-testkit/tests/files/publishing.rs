//! Publishing a received file across volumes: the copy, the parent folders
//! and the final sync run outside the engine's locks, and a revocation
//! while they are parked keeps the file from landing.
//!
//! A receiver's publication is forced across volumes and parked at exact
//! barriers (`vnilink::probes::PrestageHooks`); jobs are driven by hand, so
//! the revocation falls while one is parked.

use expect_test::expect;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use vnilink::probes::PrestageHooks;
use vnilink::{Core, Grants, JobState};
use vnilink_testkit::files::{only_job, payload, upload};
use vnilink_testkit::{lock, traced, wait_for, Files, Pair, TestResult, FILES};

/// How long a barrier waits to be reached.
const REACHED_WITHIN: Duration = Duration::from_secs(8);

/// How long an engine call may take while publication is parked: a call
/// that waits on a lock the publication holds fails here instead of
/// hanging.
const UNBLOCKED_WITHIN: Duration = Duration::from_secs(5);

/// One point a publication parks at, the first time it gets there, until
/// released.
struct Barrier {
    entered: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}

impl Barrier {
    const fn new() -> Self {
        Self {
            entered: AtomicBool::new(false),
            released: Mutex::new(false),
            wake: Condvar::new(),
        }
    }

    fn reset(&self) {
        self.entered.store(false, Ordering::SeqCst);
        *lock(&self.released) = false;
    }

    fn park(&self) {
        if self.entered.swap(true, Ordering::SeqCst) {
            return;
        }
        // Bounded: the test releases promptly, and the bound only frees a
        // worker a failed test left parked.
        let released = lock(&self.released);
        let _released =
            self.wake
                .wait_timeout_while(released, Duration::from_secs(30), |released| !*released);
    }

    fn reached(&self) -> bool {
        self.entered.load(Ordering::SeqCst)
    }

    fn release(&self) {
        *lock(&self.released) = true;
        self.wake.notify_all();
    }

    /// Waits until the publication parks here.
    fn wait_reached(&self, what: &str) {
        wait_for(what, REACHED_WITHIN, || self.reached().then_some(()));
    }
}

/// Inside the copy to the destination's volume.
static COPY: Barrier = Barrier::new();
/// Making the destination's parent folders.
static PARENTS: Barrier = Barrier::new();
/// Syncing the staged copy before it is renamed into place.
static SYNC: Barrier = Barrier::new();

fn copying(_job: u64, _bytes: u64) {
    COPY.park();
}

fn making_parents(_job: u64) {
    PARENTS.park();
}

fn syncing(_job: u64) {
    SYNC.park();
}

/// The barriers armed for one receiving job, whose publication is forced
/// across volumes. The hooks are one per process, which a test's turn
/// ([`traced`]) holds; dropping this releases every barrier and clears them.
struct Parked;

impl Parked {
    fn install(receiver: &Core, job: u64, hooks: PrestageHooks) -> Self {
        for barrier in [&COPY, &PARENTS, &SYNC] {
            barrier.reset();
        }
        Core::set_prestage_hooks(PrestageHooks {
            engine: receiver.prestage_key(),
            job_id: Some(job),
            force_cross_volume: true,
            ..hooks
        });
        Self
    }

    /// Parked inside the copy.
    fn in_copy(receiver: &Core, job: u64) -> Self {
        Self::install(
            receiver,
            job,
            PrestageHooks {
                probe: Some(copying),
                ..unhooked()
            },
        )
    }

    /// Parked making parent folders, then at the final sync.
    fn in_parents_and_sync(receiver: &Core, job: u64) -> Self {
        Self::install(
            receiver,
            job,
            PrestageHooks {
                mkdir_probe: Some(making_parents),
                sync_probe: Some(syncing),
                ..unhooked()
            },
        )
    }
}

impl Drop for Parked {
    fn drop(&mut self) {
        for barrier in [&COPY, &PARENTS, &SYNC] {
            barrier.release();
        }
        Core::set_prestage_hooks(unhooked());
    }
}

fn unhooked() -> PrestageHooks {
    PrestageHooks {
        engine: 0,
        job_id: None,
        force_cross_volume: false,
        probe: None,
        mkdir_probe: None,
        sync_probe: None,
    }
}

/// The controller's upload of `big.bin` to `arrived` on the host, the
/// host's job receiving it, and the Files it rides, which live as long as
/// the test does: a job whose Files closed is interrupted for that, not
/// for the revocation.
fn upload_big(pair: &Pair) -> Result<(Files, u64, u64), Box<dyn std::error::Error>> {
    std::fs::write(
        pair.controller.folder().join("big.bin"),
        payload(200 * 1024, 7),
    )?;
    let files = pair.open_files();
    let job = upload(pair, &files, "big.bin", "arrived")?;
    let receiver = only_job(&pair.host.core)?;
    Ok((files, job, receiver))
}

/// Drives the job on its own thread, within five hundred polls of each
/// side, until the receiver ends; the receiver's last state. It is not
/// scoped, so a parked publication that never returns fails the test at
/// its bound rather than hanging it.
fn drive_receiver(pair: &Pair, job: u64, receiver: u64) -> JoinHandle<JobState> {
    let host = Arc::clone(&pair.host.core);
    let controller = Arc::clone(&pair.controller.core);
    std::thread::spawn(move || {
        let mut sent = controller.job_snapshot(job).expect("the sender's job");
        let mut got = host.job_snapshot(receiver).expect("the receiver's job");
        for _ in 0..500 {
            if !got.state.is_terminal() {
                got = host.job_poll(receiver).expect("the receiver's poll");
            }
            if !sent.state.is_terminal() {
                sent = controller.job_poll(job).expect("the sender's poll");
            }
            if got.state.is_terminal() {
                break;
            }
        }
        got.state
    })
}

/// Runs `call` on `core` and fails naming `what` if it does not return
/// within [`UNBLOCKED_WITHIN`]: a lock the parked publication holds.
fn unblocked(what: &str, core: &Arc<Core>, call: impl FnOnce(&Core) + Send + 'static) {
    let core = Arc::clone(core);
    let (done, completed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        call(&core);
        let _ = done.send(());
    });
    assert!(
        completed.recv_timeout(UNBLOCKED_WITHIN).is_ok(),
        "{what} blocked while publication was parked"
    );
    worker.join().expect("the engine call");
}

/// The host stops sharing files with the controller.
fn revoke(pair: &Pair) -> Result<(), vnilink::Error> {
    let (sharing, _) = FILES;
    pair.host.core.set_desktop_sharing(Grants {
        transfer: false,
        ..sharing
    })
}

/// Every file under `folder` whose name starts with `prefix`.
fn files_named(folder: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_named(&path, prefix));
        } else if entry.file_name().to_string_lossy().starts_with(prefix) {
            found.push(path);
        }
    }
    found
}

/// What a revoked publication leaves on the host: nothing landed in
/// `arrived`, whose parent chain may be made eagerly, and no sibling copy
/// anywhere in its folders.
fn assert_nothing_landed(pair: &Pair) -> TestResult {
    let arrived = pair.host.folder().join("arrived");
    assert!(
        !arrived.join("big.bin").exists(),
        "a revoked publication never lands"
    );
    assert!(
        !arrived.exists() || std::fs::read_dir(&arrived)?.count() == 0,
        "a revoked publication lands no files"
    );
    let folders = pair
        .host
        .folder()
        .parent()
        .ok_or("the engine's folder inside its scratch folder")?
        .to_path_buf();
    let leftovers = files_named(&folders, ".vnlpub.");
    assert!(
        leftovers.is_empty(),
        "a revoked publication leaves no sibling copy: {leftovers:?}"
    );
    Ok(())
}

/// A publication across volumes copies outside the engine's locks: while
/// the copy is parked a snapshot completes and the host revokes the
/// controller's transfers, and the revoked destination is never published.
#[test]
fn revoked_cross_volume_publish_never_lands_and_locks_stay_free() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (_files, job, receiver) = upload_big(&pair)?;
    let parked = Parked::in_copy(&pair.host.core, receiver);
    let driver = drive_receiver(&pair, job, receiver);
    COPY.wait_reached("the publication's copy to park");
    unblocked(
        "a snapshot during the parked copy",
        &pair.host.core,
        |core| {
            core.snapshot().expect("a snapshot");
        },
    );
    revoke(&pair)?;
    COPY.release();
    let ended = driver.join().expect("the job's driver");
    drop(parked);
    assert_eq!(
        ended,
        JobState::Interrupted,
        "a revocation during the copy interrupts the job"
    );
    assert_nothing_landed(&pair)?;
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=204800 size=204800
          files: job state from=serving job=1 sender=true to=interrupted
        host:
          files: file received outcome=received resumed_at=0 size=204800 took=204800
          files: job state from=transferring job=1 sender=false to=interrupted
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// Making parent folders and the final sync both run outside the engine's
/// locks: while each is parked, the calls that stopping and revoking need
/// complete, and a revocation during the parked sync still keeps the file
/// from landing.
#[test]
fn parked_mkdir_and_stage_sync_leave_locks_free() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (_files, job, receiver) = upload_big(&pair)?;
    let parked = Parked::in_parents_and_sync(&pair.host.core, receiver);
    let driver = drive_receiver(&pair, job, receiver);

    PARENTS.wait_reached("the publication to park making parent folders");
    let host = &pair.host.core;
    unblocked("a snapshot while making parents", host, |core| {
        core.snapshot().expect("a snapshot");
    });
    unblocked("a job snapshot while making parents", host, move |core| {
        core.job_snapshot(receiver).expect("a job snapshot");
    });
    unblocked("the shared roots while making parents", host, |core| {
        core.file_roots().expect("the shared roots");
    });
    PARENTS.release();

    SYNC.wait_reached("the publication to park at its final sync");
    unblocked("a snapshot during the final sync", host, |core| {
        core.snapshot().expect("a snapshot");
    });
    revoke(&pair)?;
    SYNC.release();
    let ended = driver.join().expect("the job's driver");
    drop(parked);
    assert_eq!(
        ended,
        JobState::Interrupted,
        "a revocation during the final sync interrupts the job"
    );
    assert_nothing_landed(&pair)?;
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=204800 size=204800
          files: job state from=serving job=1 sender=true to=interrupted
        host:
          files: file received outcome=received resumed_at=0 size=204800 took=204800
          files: job state from=transferring job=1 sender=false to=interrupted
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
