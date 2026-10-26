//! What the files suites share: deterministic file contents, exact tree
//! comparison, and jobs driven as the application drives them.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use vnilink::{Core, JobSnapshot};

use crate::{wait_for, Pair};

/// `len` bytes that differ for each `seed` and compress poorly, as real
/// files mostly do.
pub fn payload(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        bytes.extend_from_slice(&state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

/// Writes `files` (relative path, size) under `folder`, each with its own
/// contents.
pub fn write_files(folder: &Path, files: &[(&str, usize)]) {
    for (index, (path, size)) in files.iter().enumerate() {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("a folder");
        std::fs::write(path, payload(*size, index as u64 + 1)).expect("a file");
    }
}

/// One entry of a tree: a folder, or a file's size and hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Folder,
    File { size: u64, hash: String },
}

/// Every folder and file under `folder`, by relative path with `/`, so two
/// trees compare exactly: names, kinds, sizes, contents, empty folders.
pub fn tree(folder: &Path) -> BTreeMap<String, Entry> {
    fn walk(base: &Path, folder: &Path, entries: &mut BTreeMap<String, Entry>) {
        for entry in std::fs::read_dir(folder).expect("a readable folder") {
            let path = entry.expect("a folder entry").path();
            let name = path
                .strip_prefix(base)
                .expect("inside the tree")
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if path.is_dir() {
                entries.insert(name, Entry::Folder);
                walk(base, &path, entries);
            } else {
                let bytes = std::fs::read(&path).expect("a readable file");
                entries.insert(
                    name,
                    Entry::File {
                        size: bytes.len() as u64,
                        hash: blake3::hash(&bytes).to_hex().to_string(),
                    },
                );
            }
        }
    }
    let mut entries = BTreeMap::new();
    walk(folder, folder, &mut entries);
    entries
}

/// Starts both engines' background job drivers, as the application does.
pub fn drive(pair: &Pair) {
    pair.host.core.start_job_driver();
    pair.controller.core.start_job_driver();
}

/// The job `core` received from its peer, once it has one.
pub fn received(core: &Core) -> u64 {
    wait_for("the peer's job to arrive", crate::DEFAULT_WAIT, || {
        core.job_list().ok()?.first().map(|job| job.id)
    })
}

/// The job once it ended.
pub fn finished(core: &Core, job: u64, within: Duration) -> JobSnapshot {
    wait_for("the job to end", within, || {
        let snapshot = core.job_snapshot(job).ok()?;
        snapshot.state.is_terminal().then_some(snapshot)
    })
}

/// The job once it ended or stopped to ask a question. A sender mirrors its
/// receiver's question until it next asks how far the receiver is, so wait
/// for a sender with [`finished`].
pub fn settled(core: &Core, job: u64, within: Duration) -> JobSnapshot {
    wait_for("the job to end or ask", within, || {
        let snapshot = core.job_snapshot(job).ok()?;
        (snapshot.state.is_terminal() || snapshot.state == vnilink::JobState::WaitingDecision)
            .then_some(snapshot)
    })
}

/// Polls `receiver`'s job until `done` files landed, within twenty polls:
/// each poll advances a job by one step. It must not end first.
pub fn poll_until_files_done(receiver: &Core, job: u64, done: u64) {
    for _ in 0..20 {
        let snapshot = receiver.job_poll(job).expect("a poll");
        assert!(
            !snapshot.state.is_terminal(),
            "the job ended early: {snapshot:?}"
        );
        if snapshot.files_done == done {
            return;
        }
    }
    panic!("{done} files did not land within twenty polls");
}

/// Drives a job by hand, as the application's drivers would, until the
/// receiver asks a question or both sides ended: a test that must stop a
/// job at an exact point polls it this way instead of starting [`drive`].
/// Returns the sender's and the receiver's last snapshots.
pub fn drive_to_end(
    sender: &Core,
    sender_job: u64,
    receiver: &Core,
    receiver_job: u64,
    within: Duration,
) -> (JobSnapshot, JobSnapshot) {
    let mut sent = sender.job_snapshot(sender_job).expect("the sender's job");
    let mut got = receiver
        .job_snapshot(receiver_job)
        .expect("the receiver's job");
    wait_for("both sides of the job to end", within, || {
        if !got.state.is_terminal() {
            got = receiver.job_poll(receiver_job).ok()?;
        }
        if !sent.state.is_terminal() {
            sent = sender.job_poll(sender_job).ok()?;
        }
        let asking = got.state == vnilink::JobState::WaitingDecision;
        (asking || (got.state.is_terminal() && sent.state.is_terminal()))
            .then(|| (sent.clone(), got.clone()))
    })
}
