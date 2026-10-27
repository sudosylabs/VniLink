//! A transfer job resumes from what an interrupted attempt left staged.
//!
//! Tree comparisons are exact (paths, types, sizes, sha256, empty dirs,
//! no extras). Job polls advance one file, never through sleeps.

use std::time::Duration;

use crate::core::Core;
use crate::job::JobState;
use crate::manifest;
use crate::test_support::{transfer_pair, FilesystemLocation};

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
    let (host, controller, host_scratch, controller_scratch) = transfer_pair("partial");
    let host_files = FilesystemLocation::on(&host, &host_scratch.join("root"));
    let controller_files = FilesystemLocation::on(&controller, &controller_scratch.join("root"));
    let segment = SEGMENT_LEN as usize;
    let journaled = payload(segment + 1024 * 1024 + 11, 21);
    let unjournaled = payload(2 * 1024 * 1024 + 5, 22);
    std::fs::write(host_scratch.join("root/journaled.bin"), &journaled).unwrap();
    std::fs::write(host_scratch.join("root/unjournaled.bin"), &unjournaled).unwrap();
    controller.connect_files(&host.endpoint_id()).unwrap();
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
