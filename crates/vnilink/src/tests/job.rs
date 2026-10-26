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
