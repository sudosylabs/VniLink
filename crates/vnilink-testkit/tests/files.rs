// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The Files service alone: two engines on this computer moving trees
//! between their own folders on their shared disks, with both job drivers
//! running as in the application.

#[path = "files/compression.rs"]
mod compression;
#[path = "files/drives.rs"]
mod drives;
#[path = "files/owners.rs"]
mod owners;
#[path = "files/pacing.rs"]
mod pacing;
#[path = "files/publishing.rs"]
mod publishing;
#[path = "files/remote_disk.rs"]
mod remote_disk;
#[path = "files/service_jobs.rs"]
mod service_jobs;
#[path = "files/serving.rs"]
mod serving;
#[path = "files/shared.rs"]
mod shared;
#[path = "files/shutdown.rs"]
mod shutdown;
#[path = "files/transfers.rs"]
mod transfers;

use expect_test::expect;

use vnilink::{ConflictChoice, JobState};
use vnilink_testkit::files::{
    check, drive, finished, received, settled, tree, upload, write_files, Job, Moving, Way,
    TRANSFER,
};
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// A tree with what trips transfers up: nesting, an empty folder, a name
/// outside ASCII, an empty file, and a file of several segments.
const TREE: &[(&str, usize)] = &[
    ("top.txt", 1024),
    ("docs/read me-ß.txt", 4096),
    ("docs/2026/big.bin", 9 * 1024 * 1024 + 7),
    ("docs/empty.txt", 0),
    ("pack/sub/a.txt", 512),
];

/// The folder of [`TREE`] that holds no file.
const EMPTY: &[&str] = &["empty"];

/// [`TREE`] moved from `from` to `to` by the engines' own job drivers.
fn tree_by_drivers<'a>(way: Way, from: &'a str, to: &'a str) -> Moving<'a> {
    Moving {
        way,
        files: TREE,
        folders: EMPTY,
        from,
        to,
        drivers: true,
    }
}

#[test]
fn an_upload_lands_exactly() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::create_dir_all(pair.host.folder().join("inbox"))?;
    // Each side records its own job, and every file crossed whole.
    let moved = check(
        &trace,
        &pair,
        tree_by_drivers(Way::Upload, "set", "inbox"),
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=0 size=0
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=4096 size=4096
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=9437191 size=9437191
          files: job state done_bytes=9442823 files_done=5 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=0 took=0
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=4096 took=4096
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=9437191 took=9437191
          files: received files flushed job=1
          files: job state done_bytes=9442823 files_done=5 from=transferring job=1 sender=false to=complete
    "#]],
    )?;
    assert_eq!(
        moved.got.files_done,
        TREE.len() as u64,
        "the receiver counts every file it landed"
    );
    Ok(())
}

#[test]
fn a_download_lands_exactly() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::create_dir_all(pair.controller.folder().join("downloads"))?;
    check(
        &trace,
        &pair,
        tree_by_drivers(Way::Download, "shared", "downloads"),
        expect![[r#"
            controller:
              files: file received outcome=received resumed_at=0 size=0 took=0
              files: file received outcome=received resumed_at=0 size=1024 took=1024
              files: file received outcome=received resumed_at=0 size=4096 took=4096
              files: file received outcome=received resumed_at=0 size=512 took=512
              files: file received outcome=received resumed_at=0 size=9437191 took=9437191
              files: received files flushed job=1
              files: job state done_bytes=9442823 files_done=5 from=transferring job=1 sender=false to=complete
            host:
              files: job state from=preparing job=1 sender=true to=serving
              files: file served ended=sent from=0 sent=0 size=0
              files: file served ended=sent from=0 sent=1024 size=1024
              files: file served ended=sent from=0 sent=4096 size=4096
              files: file served ended=sent from=0 sent=512 size=512
              files: file served ended=sent from=0 sent=9437191 size=9437191
              files: job state done_bytes=9442823 files_done=5 from=serving job=1 sender=true to=complete
        "#]],
    )?;
    Ok(())
}

/// Each name already at the destination waits for a choice, and each
/// choice does what it says: replace, skip, or keep both under a new name.
#[test]
fn conflicts_wait_for_a_choice_and_follow_it() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(
        &pair.controller.folder().join("set"),
        &[("a.txt", 100), ("b.txt", 100), ("c.txt", 100)],
    );
    let destination = pair.host.folder().join("inbox/set");
    std::fs::create_dir_all(&destination)?;
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(destination.join(name), format!("old {name}"))?;
    }
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "inbox")?;
    let receiver = received(&pair.host.core);
    for (name, choice) in [
        ("a.txt", ConflictChoice::Replace),
        ("b.txt", ConflictChoice::Skip),
        ("c.txt", ConflictChoice::KeepBoth),
    ] {
        let waiting = settled(&pair.host.core, receiver, TRANSFER);
        assert_eq!(
            waiting.state,
            JobState::WaitingDecision,
            "a name already there waits for a choice: {waiting:?}"
        );
        assert_eq!(
            waiting.waiting_file.as_deref(),
            Some(format!("set/{name}").as_str()),
            "the job asks about one file at a time, in order"
        );
        pair.host.core.job_resolve(receiver, choice, false)?;
        wait_for("the choice to be taken", DEFAULT_WAIT, || {
            let now = pair.host.core.job_snapshot(receiver).ok()?;
            (now.waiting_file.as_deref() != Some(format!("set/{name}").as_str())).then_some(())
        });
    }
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver completes: {got:?}"
    );
    let sent = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    let source = pair.controller.folder().join("set");
    assert_eq!(
        std::fs::read(destination.join("a.txt"))?,
        std::fs::read(source.join("a.txt"))?,
        "Replace lands the new file"
    );
    assert_eq!(
        std::fs::read(destination.join("b.txt"))?,
        b"old b.txt",
        "Skip keeps the original"
    );
    assert_eq!(
        std::fs::read(destination.join("c.txt"))?,
        b"old c.txt",
        "Keep both keeps the original under its name"
    );
    let kept = std::fs::read_dir(&destination)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("c (2)"))
        })
        .ok_or("the second c.txt is kept under a new name")?;
    assert_eq!(
        std::fs::read(kept)?,
        std::fs::read(source.join("c.txt"))?,
        "Keep both lands the new file under a new name"
    );
    Ok(())
}

/// A cancelled upload ends on both sides, and resuming it finishes the
/// same tree.
#[test]
fn a_cancelled_upload_resumes_to_the_same_tree() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(&pair.controller.folder().join("set"), TREE);
    std::fs::create_dir_all(pair.controller.folder().join("set/empty"))?;
    std::fs::create_dir_all(pair.host.folder().join("inbox"))?;
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "set", "inbox")?;
    pair.controller.core.job_cancel(job.controller)?;
    let (sent, got) = job.finished(&pair, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Cancelled, JobState::Cancelled),
        "a cancelled upload ends on both sides"
    );
    pair.host.core.job_resume(job.host)?;
    // The sender follows its receiver; resuming it too is harmless.
    let _ = pair.controller.core.job_resume(job.controller);
    let got = finished(&pair.host.core, job.host, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the resumed receiver completes: {got:?}"
    );
    assert_eq!(
        tree(&pair.host.folder().join("inbox/set")),
        tree(&pair.controller.folder().join("set")),
        "resuming finishes the same tree"
    );
    Ok(())
}

/// A host that stops sharing files mid-transfer leaves the job interrupted,
/// to resume, not cancelled, after the fetch it stopped has ended, and ends
/// the Files service it rode. Sharing again, a new Files service on the
/// same connection adopts the job and resumes it to the same tree.
#[test]
fn a_transfer_the_host_stops_sharing_is_interrupted_and_resumes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(
        &pair.controller.folder().join("set"),
        &[("large.bin", 128 * 1024 * 1024)],
    );
    std::fs::create_dir_all(pair.host.folder().join("inbox"))?;
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "set", "inbox")?;
    let receiver = job.host;
    wait_for("the first bytes to arrive", DEFAULT_WAIT, || {
        (pair.host.core.job_snapshot(receiver).ok()?.done_bytes > 0).then_some(())
    });
    let (sharing, _) = vnilink_testkit::FILES;
    pair.host.core.set_desktop_sharing(vnilink::Grants {
        transfer: false,
        ..sharing
    })?;
    // The stopped fetch records its end after the job was interrupted.
    trace.wait_for_records("file received", 1);
    let stopped = pair.host.core.job_snapshot(receiver)?;
    assert_eq!(
        (stopped.state, stopped.error.as_deref()),
        (
            JobState::Interrupted,
            Some("file transfer is no longer allowed")
        ),
        "a withdrawn permission interrupts a job to resume, and says why: {stopped:?}"
    );

    pair.host.core.set_desktop_sharing(sharing)?;
    pair.host.wait_released(Some(vnilink::ServiceKind::Files));
    // A connection has one Files owner at a time: the old one learns of its
    // end just after the host lets it go.
    wait_for("the old Files owner to end", DEFAULT_WAIT, || {
        let old = pair
            .controller
            .core
            .controller_service(files.service.id)
            .ok()?;
        service_ended(&old.phase).then_some(())
    });
    let (fresh, _pipe) = pair.controller.open_files(&files.connection)?;
    let controller = &pair.controller.core;
    // The old service's last step on the job may still hold it: it is busy
    // until that step ends.
    wait_for(
        "the job to be free to adopt",
        DEFAULT_WAIT,
        || match controller.files_service_adopt_job(fresh.id, fresh.generation, job.controller) {
            Ok(_) => Some(()),
            Err(vnilink::Error::Busy) => None,
            Err(error) => panic!("the fresh service cannot adopt the job: {error}"),
        },
    );
    controller.files_service_job_resume(fresh.id, fresh.generation, job.controller)?;
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the adopted job resumes and completes: {got:?}"
    );
    assert_eq!(
        tree(&pair.host.folder().join("inbox/set")),
        tree(&pair.controller.folder().join("set")),
        "the resumed job lands the same tree"
    );
    Ok(())
}
