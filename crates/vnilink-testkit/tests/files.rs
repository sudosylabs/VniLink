// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The Files service alone: two engines on this computer moving trees
//! between their own folders on their shared disks, with both job drivers
//! running as in the application.

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
use std::time::Duration;

use vnilink::{ConflictChoice, JobState};
use vnilink_testkit::files::{
    download, drive, finished, received, settled, tree, upload, write_files,
};
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, Pair, DEFAULT_WAIT};

/// A tree with what trips transfers up: nesting, an empty folder, a name
/// outside ASCII, an empty file, and a file of several segments.
const TREE: &[(&str, usize)] = &[
    ("top.txt", 1024),
    ("docs/read me-ß.txt", 4096),
    ("docs/2026/big.bin", 9 * 1024 * 1024 + 7),
    ("docs/empty.txt", 0),
    ("pack/sub/a.txt", 512),
];

const TRANSFER: Duration = Duration::from_secs(60);

fn source_tree(folder: &std::path::Path) {
    write_files(folder, TREE);
    std::fs::create_dir_all(folder.join("empty")).unwrap();
}

#[test]
fn an_upload_lands_exactly() {
    let trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    source_tree(&pair.controller.folder().join("set"));
    std::fs::create_dir_all(pair.host.folder().join("inbox")).unwrap();
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "inbox").unwrap();
    let sent = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(sent.state, JobState::Complete, "{sent:?}");
    let receiver = received(&pair.host.core);
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(got.files_done, TREE.len() as u64);
    assert_eq!(
        tree(&pair.host.folder().join("inbox/set")),
        tree(&pair.controller.folder().join("set"))
    );
    // Each side records its own job, and every file crossed whole.
    vnilink_testkit::assert_story(
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
          files: job state done_bytes=9442823 files_done=5 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.concurrent_story(&["files"]),
    );
    trace.assert_unrecorded("a file's name", &["read me"]);
}

#[test]
fn a_download_lands_exactly() {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    source_tree(&pair.host.folder().join("shared"));
    std::fs::create_dir_all(pair.controller.folder().join("downloads")).unwrap();
    let files = pair.open_files();
    let job = download(&pair, &files, "shared", "downloads").unwrap();
    let got = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(
        tree(&pair.controller.folder().join("downloads/shared")),
        tree(&pair.host.folder().join("shared"))
    );
}

/// Each name already at the destination waits for a choice, and each
/// choice does what it says: replace, skip, or keep both under a new name.
#[test]
fn conflicts_wait_for_a_choice_and_follow_it() {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(
        &pair.controller.folder().join("set"),
        &[("a.txt", 100), ("b.txt", 100), ("c.txt", 100)],
    );
    let destination = pair.host.folder().join("inbox/set");
    std::fs::create_dir_all(&destination).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(destination.join(name), format!("old {name}")).unwrap();
    }
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "inbox").unwrap();
    let receiver = received(&pair.host.core);
    for (name, choice) in [
        ("a.txt", ConflictChoice::Replace),
        ("b.txt", ConflictChoice::Skip),
        ("c.txt", ConflictChoice::KeepBoth),
    ] {
        let waiting = settled(&pair.host.core, receiver, TRANSFER);
        assert_eq!(waiting.state, JobState::WaitingDecision, "{waiting:?}");
        assert_eq!(
            waiting.waiting_file.as_deref(),
            Some(format!("set/{name}").as_str())
        );
        pair.host.core.job_resolve(receiver, choice, false).unwrap();
        wait_for("the choice to be taken", DEFAULT_WAIT, || {
            let now = pair.host.core.job_snapshot(receiver).ok()?;
            (now.waiting_file.as_deref() != Some(format!("set/{name}").as_str())).then_some(())
        });
    }
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    let sent = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(sent.state, JobState::Complete, "{sent:?}");
    let source = pair.controller.folder().join("set");
    assert_eq!(
        std::fs::read(destination.join("a.txt")).unwrap(),
        std::fs::read(source.join("a.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read(destination.join("b.txt")).unwrap(),
        b"old b.txt"
    );
    assert_eq!(
        std::fs::read(destination.join("c.txt")).unwrap(),
        b"old c.txt"
    );
    let kept = std::fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("c (2)")
        })
        .expect("the second c.txt is kept under a new name");
    assert_eq!(
        std::fs::read(kept).unwrap(),
        std::fs::read(source.join("c.txt")).unwrap()
    );
}

/// A cancelled upload ends on both sides, and resuming it finishes the
/// same tree.
#[test]
fn a_cancelled_upload_resumes_to_the_same_tree() {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    source_tree(&pair.controller.folder().join("set"));
    std::fs::create_dir_all(pair.host.folder().join("inbox")).unwrap();
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "inbox").unwrap();
    let receiver = received(&pair.host.core);
    pair.controller.core.job_cancel(job).unwrap();
    assert_eq!(
        finished(&pair.controller.core, job, TRANSFER).state,
        JobState::Cancelled
    );
    assert_eq!(
        finished(&pair.host.core, receiver, TRANSFER).state,
        JobState::Cancelled
    );
    pair.host.core.job_resume(receiver).unwrap();
    // The sender follows its receiver; resuming it too is harmless.
    let _ = pair.controller.core.job_resume(job);
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(
        tree(&pair.host.folder().join("inbox/set")),
        tree(&pair.controller.folder().join("set"))
    );
}

/// A host that stops sharing files mid-transfer leaves the job interrupted,
/// to resume, not cancelled, after the fetch it stopped has ended, and ends
/// the Files service it rode. Sharing again, a new Files service on the
/// same connection adopts the job and resumes it to the same tree.
#[test]
fn a_transfer_the_host_stops_sharing_is_interrupted_and_resumes() {
    let trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(
        &pair.controller.folder().join("set"),
        &[("large.bin", 128 * 1024 * 1024)],
    );
    std::fs::create_dir_all(pair.host.folder().join("inbox")).unwrap();
    let files = pair.open_files();
    let job = upload(&pair, &files, "set", "inbox").unwrap();
    let receiver = received(&pair.host.core);
    wait_for("the first bytes to arrive", DEFAULT_WAIT, || {
        (pair.host.core.job_snapshot(receiver).ok()?.done_bytes > 0).then_some(())
    });
    let (sharing, _) = vnilink_testkit::FILES;
    pair.host
        .core
        .set_desktop_sharing(vnilink::Grants {
            transfer: false,
            ..sharing
        })
        .unwrap();
    // The stopped fetch records its end after the job was interrupted.
    trace.wait_for_records("file received", 1);
    let stopped = pair.host.core.job_snapshot(receiver).unwrap();
    assert_eq!(
        (stopped.state, stopped.error.as_deref()),
        (
            JobState::Interrupted,
            Some("file transfer is no longer allowed")
        ),
        "a withdrawn permission interrupts a job to resume, and says why: {stopped:?}"
    );

    pair.host.core.set_desktop_sharing(sharing).unwrap();
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
    let (fresh, _pipe) = pair.controller.open_files(&files.connection).unwrap();
    let controller = &pair.controller.core;
    // The old service's last step on the job may still hold it: it is busy
    // until that step ends.
    wait_for(
        "the job to be free to adopt",
        DEFAULT_WAIT,
        || match controller.files_service_adopt_job(fresh.id, fresh.generation, job) {
            Ok(_) => Some(()),
            Err(vnilink::Error::Busy) => None,
            Err(error) => panic!("the fresh service cannot adopt the job: {error}"),
        },
    );
    controller
        .files_service_job_resume(fresh.id, fresh.generation, job)
        .unwrap();
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(
        tree(&pair.host.folder().join("inbox/set")),
        tree(&pair.controller.folder().join("set"))
    );
}
