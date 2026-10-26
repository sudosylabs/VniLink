//! The Files service alone: two engines on this computer moving trees
//! between their roots, with both job drivers running as in the
//! application.

#[path = "files/pacing.rs"]
mod pacing;
#[path = "files/rebinding.rs"]
mod rebinding;
#[path = "files/remote_disk.rs"]
mod remote_disk;
#[path = "files/service_jobs.rs"]
mod service_jobs;
#[path = "files/serving.rs"]
mod serving;
#[path = "files/shutdown.rs"]
mod shutdown;
#[path = "files/transfers.rs"]
mod transfers;

use expect_test::expect;
use std::time::Duration;

use vnilink::{ConflictChoice, JobState};
use vnilink_testkit::files::{drive, finished, received, settled, tree, write_files};
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
    source_tree(&pair.controller.root().join("set"));
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let job = pair
        .controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
    let sent = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(sent.state, JobState::Complete, "{sent:?}");
    let receiver = received(&pair.host.core);
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(got.files_done, TREE.len() as u64);
    assert_eq!(
        tree(&pair.host.root().join("inbox/set")),
        tree(&pair.controller.root().join("set"))
    );
    // Each side records its own job, and every file crossed whole.
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
    "#]].assert_eq(&trace.concurrent_story(&["files"]));
    let everything = trace.events().join("\n");
    assert!(!everything.contains("read me"), "a file name was recorded");
}

#[test]
fn a_download_lands_exactly() {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    source_tree(&pair.host.root().join("shared"));
    std::fs::create_dir_all(pair.controller.root().join("downloads")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let job = pair
        .controller
        .core
        .job_submit_download(
            &pair.host.endpoint(),
            pair.host.root_id(),
            "shared",
            pair.controller.root_id(),
            "downloads",
        )
        .unwrap();
    let got = finished(&pair.controller.core, job, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(
        tree(&pair.controller.root().join("downloads/shared")),
        tree(&pair.host.root().join("shared"))
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
        &pair.controller.root().join("set"),
        &[("a.txt", 100), ("b.txt", 100), ("c.txt", 100)],
    );
    let destination = pair.host.root().join("inbox/set");
    std::fs::create_dir_all(&destination).unwrap();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(destination.join(name), format!("old {name}")).unwrap();
    }
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let job = pair
        .controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
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
    let source = pair.controller.root().join("set");
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
    source_tree(&pair.controller.root().join("set"));
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    let job = pair
        .controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
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
        tree(&pair.host.root().join("inbox/set")),
        tree(&pair.controller.root().join("set"))
    );
}

/// A host that stops sharing files mid-transfer leaves the job interrupted,
/// to resume, not cancelled, after the fetch it stopped has ended. Sharing
/// again, the job resumes to the same tree.
#[test]
fn a_transfer_the_host_stops_sharing_is_interrupted_and_resumes() {
    let trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    write_files(
        &pair.controller.root().join("set"),
        &[("large.bin", 128 * 1024 * 1024)],
    );
    std::fs::create_dir_all(pair.host.root().join("inbox")).unwrap();
    pair.controller.core.connect(&pair.host.endpoint()).unwrap();
    pair.controller
        .core
        .job_submit_upload(
            &pair.host.endpoint(),
            pair.controller.root_id(),
            "set",
            pair.host.root_id(),
            "inbox",
        )
        .unwrap();
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
        stopped.state,
        JobState::Interrupted,
        "a withdrawn permission interrupts a job to resume: {stopped:?}"
    );

    pair.host.core.set_desktop_sharing(sharing).unwrap();
    pair.host.core.job_resume(receiver).unwrap();
    let got = finished(&pair.host.core, receiver, TRANSFER);
    assert_eq!(got.state, JobState::Complete, "{got:?}");
    assert_eq!(
        tree(&pair.host.root().join("inbox/set")),
        tree(&pair.controller.root().join("set"))
    );
}
