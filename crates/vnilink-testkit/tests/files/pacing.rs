//! How transfers pace themselves: a small file promptly, many skipped files
//! without a pass each, a waiting transfer that sleeps, small files fetched
//! ahead and landed in order, and what diagnostics count.

use expect_test::expect;
use std::time::{Duration, Instant};

use vnilink::{ConflictChoice, JobDestination, JobItem, JobState};
use vnilink_testkit::files::{
    self, drive, drive_to_end, finished, payload, received, settled, tree,
};
use vnilink_testkit::{holds, traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

const TRANSFER: Duration = Duration::from_secs(30);

/// `read`'s value once it stayed the same for `quiet`, within `within`.
fn settled_value<T: PartialEq + Clone>(
    what: &str,
    quiet: Duration,
    within: Duration,
    mut read: impl FnMut() -> T,
) -> T {
    let mut last = read();
    let mut since = Instant::now();
    wait_for(what, within, || {
        let now = read();
        if now != last {
            last = now;
            since = Instant::now();
            return None;
        }
        (since.elapsed() >= quiet).then(|| last.clone())
    })
}

/// A small file arrives, and both sides see it complete, promptly with only
/// the background drivers advancing it, as the Files tab leaves it to them;
/// the second reuses the first one's files connection.
#[test]
fn a_small_file_completes_promptly_on_its_own() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    pair.controller.core.connect(&pair.host.endpoint())?;
    for name in ["first.bin", "second.bin"] {
        std::fs::write(pair.controller.root().join(name), vec![7u8; 2500])?;
        // Submitting scans and offers the file: it counts.
        let started = std::time::Instant::now();
        let job = files::upload(&pair, name, "")?;
        wait_for(
            &format!("{name} to complete within two seconds"),
            Duration::from_secs(2),
            || {
                (pair.controller.core.job_snapshot(job).ok()?.state == JobState::Complete)
                    .then_some(())
            },
        );
        assert!(
            started.elapsed() <= Duration::from_secs(2),
            "{name} completes within two seconds of its submission: {:?}",
            started.elapsed()
        );
        assert_eq!(
            std::fs::read(pair.host.root().join(name))?.len(),
            2500,
            "a completed small file landed whole"
        );
    }
    Ok(())
}

/// A Files owner that goes leaves its transfer interrupted, not cancelled;
/// the next owner for that computer takes it over, resumes it, and it
/// finishes where it was going.
#[test]
fn a_new_files_owner_resumes_what_a_lost_one_left() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (controller, host) = (&pair.controller.core, &pair.host.core);
    let bytes = payload(512 * 1024, 917);
    std::fs::write(pair.controller.root().join("carry.bin"), &bytes)?;
    let session = pair.connect_control();
    let (lost, pipe) = pair.controller.open_files(&session)?;
    let job = controller.files_service_upload(
        lost.id,
        lost.generation,
        pair.controller.root_id(),
        "",
        &["carry.bin".to_owned()],
        pair.host.root_id(),
        "inbox",
    )?;
    assert_eq!(
        controller.job_snapshot(job)?.destination,
        Some(JobDestination {
            root: pair.host.root_id(),
            folder: "inbox".into(),
            items: vec![JobItem {
                name: "carry.bin".into(),
                folder: false,
            }],
        }),
        "a Files job names where it is going"
    );
    let receiver = received(host);
    controller.close_controller_service(lost.id, lost.generation)?;
    drop(pipe);
    assert_eq!(
        controller.job_snapshot(job)?.state,
        JobState::Interrupted,
        "a job whose owner went waits to resume, not cancelled"
    );
    pair.host.wait_released(None);
    let (fresh, _pipe) = pair.controller.open_files(&session)?;
    let resumed = controller.files_service_resume_orphans(fresh.id, fresh.generation)?;
    assert_eq!(
        resumed.iter().map(|job| job.id).collect::<Vec<_>>(),
        [job],
        "the next owner takes over the lost one's job"
    );
    assert_ne!(
        resumed[0].state,
        JobState::Interrupted,
        "a job taken over is resumed"
    );
    assert!(
        controller
            .files_service_resume_orphans(fresh.id, fresh.generation)?
            .is_empty(),
        "nothing is left to take over a second time"
    );
    let (sender, got) = drive_to_end(controller, job, host, receiver, TRANSFER);
    assert_eq!(
        (sender.state, got.state),
        (JobState::Complete, JobState::Complete),
        "a job taken over finishes"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("inbox/carry.bin"))?,
        bytes,
        "it lands where it was going"
    );
    trace.wait_for_records("file received", 1);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=524288 size=524288
          files: job state done_bytes=524288 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=524288 took=524288
          files: job state done_bytes=524288 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// The folder a pane shows on the other computer is watched over a real
/// connection: a file made there by anything raises the Files service's
/// folder revision, while the pipe keeps answering, and watching nothing
/// stops it.
#[test]
fn the_other_computers_folder_says_when_it_changes() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    let (_owner, pipe) = pair.controller.open_files(&pair.connect_control())?;
    let root = pair.host.root_id();
    pipe.watch(Some((root, String::new())));
    // The watch's first answer is the folder as it is, and only a change
    // after it counts; nothing tells when that answer came, so files are
    // made there until a change is counted. A new name, as kqueue reports
    // no writes in place.
    let before = pipe.folder_revision();
    let mut made: Option<Instant> = None;
    let mut count = 0;
    wait_for(
        "a change to the watched folder to count",
        DEFAULT_WAIT,
        || {
            if made.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) {
                count += 1;
                let name = format!("arrived-{count}.txt");
                std::fs::write(pair.host.root().join(name), b"new").ok()?;
                made = Some(Instant::now());
            }
            (pipe.folder_revision() > before).then_some(())
        },
    );
    assert!(
        pipe.list(root, "", 0, 100)?
            .entries
            .iter()
            .any(|entry| entry.name == "arrived-1.txt"),
        "the pipe is not held by the wait"
    );
    pipe.watch(None);
    let stopped = pipe.folder_revision();
    std::fs::write(pair.host.root().join("later.txt"), b"later")?;
    holds(
        "an unwatched folder counting no changes",
        Duration::from_millis(1500),
        || pipe.folder_revision() == stopped,
    );
    Ok(())
}

/// A thousand files the receiver already has, skipped once asked, take
/// no pass each: its worker skips one after another until the job is
/// done, where the driver's 50 ms between passes needed 50 seconds.
#[test]
fn a_thousand_skipped_files_take_no_pass_each() -> TestResult {
    const FILES: usize = 1000;
    let trace = traced();
    let pair = Pair::for_files();
    for side in [&pair.controller, &pair.host] {
        let folder = side.root().join("many");
        std::fs::create_dir_all(&folder)?;
        for index in 0..FILES {
            std::fs::write(folder.join(format!("f-{index:04}.txt")), b"x")?;
        }
    }
    pair.controller.core.connect(&pair.host.endpoint())?;
    drive(&pair);
    let job = files::upload(&pair, "many", "")?;
    let receiver = received(&pair.host.core);
    let asking = settled(&pair.host.core, receiver, Duration::from_secs(5));
    assert_eq!(
        asking.state,
        JobState::WaitingDecision,
        "a name the receiver has asks: {asking:?}"
    );
    // The sender mirrors the question once it next asks how far the
    // receiver is; the answer is given after that, as a person gives it.
    wait_for("the sender to mirror the question", DEFAULT_WAIT, || {
        (pair.controller.core.job_snapshot(job).ok()?.state == JobState::WaitingDecision)
            .then_some(())
    });
    // Answering may do the skipping itself: it counts.
    let started = std::time::Instant::now();
    pair.host
        .core
        .job_resolve(receiver, ConflictChoice::Skip, true)?;
    // A pass each would take a quarter of this bound.
    let bound = Duration::from_millis(FILES as u64 * 50 / 4);
    let done = wait_for("every file to be skipped", bound, || {
        let now = pair.host.core.job_snapshot(receiver).ok()?;
        (now.state == JobState::Complete).then_some(now)
    });
    assert!(
        started.elapsed() <= bound,
        "skipping every name finishes within {bound:?} of the answer: {:?}",
        started.elapsed()
    );
    assert_eq!(
        done.skipped_files, FILES as u64,
        "skip for all skips every name the receiver has"
    );
    let sent = finished(&pair.controller.core, job, DEFAULT_WAIT);
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender follows: {sent:?}"
    );
    // The receiver's story: the sender mirrors it whenever it next asks,
    // through serving or straight to complete.
    wait_for(
        "the receiver to record its completion",
        DEFAULT_WAIT,
        || {
            trace
                .named("job state")
                .iter()
                .any(|event| event["engine"] == "host" && event["to"] == "complete")
                .then_some(())
        },
    );
    expect![[r#"
        host:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state done_bytes=1000 files_done=0 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.engine_story("host", &["files"]));
    Ok(())
}

/// A sender whose receiver waits on a question asks how far it got ever
/// more seldom, and an answer that changes nothing is neither saved nor
/// told: neither side's jobs change while the question stands.
#[test]
fn a_waiting_transfer_does_not_wake_itself() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (controller, host) = (&pair.controller.core, &pair.host.core);
    std::fs::write(pair.controller.root().join("same.txt"), b"theirs")?;
    std::fs::write(pair.host.root().join("same.txt"), b"mine")?;
    controller.connect(&pair.host.endpoint())?;
    drive(&pair);
    let job = files::upload(&pair, "same.txt", "")?;
    wait_for(
        "the sender to mirror the question",
        Duration::from_secs(5),
        || (controller.job_snapshot(job).ok()?.state == JobState::WaitingDecision).then_some(()),
    );
    // The save that told of the question may land just after it shows.
    let revisions = settled_value(
        "the jobs to settle on the question",
        Duration::from_millis(300),
        DEFAULT_WAIT,
        || (controller.job_revision(), host.job_revision()),
    );
    let journal = pair
        .controller
        .transfers()
        .join(format!("jobs/{job}.jsonl"));
    let written = std::fs::metadata(&journal)?.len();
    holds(
        "a standing question changing neither side's jobs nor the journal",
        Duration::from_millis(2500),
        || {
            (controller.job_revision(), host.job_revision()) == revisions
                && std::fs::metadata(&journal).is_ok_and(|now| now.len() == written)
        },
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("same.txt"))?,
        b"mine",
        "the question stands before the file is touched"
    );
    controller.job_resolve(job, ConflictChoice::Replace, false)?;
    wait_for(
        "the answered job to complete",
        Duration::from_secs(5),
        || (controller.job_snapshot(job).ok()?.state == JobState::Complete).then_some(()),
    );
    let receiver = received(host);
    let got = finished(host, receiver, DEFAULT_WAIT);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver completes: {got:?}"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("same.txt"))?,
        b"theirs",
        "the answer replaced the file"
    );
    // Each side tells of the question once, and nothing until the answer.
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=waiting-decision
          files: job state from=waiting-decision job=1 sender=true to=transferring
          files: file served ended=sent from=0 sent=6 size=6
          files: job state done_bytes=6 files_done=1 from=transferring job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=6 took=6
          files: job state done_bytes=6 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A controller may hold both a control session and a services-only link
/// with one peer. Control of a job no Files service owns rides the control
/// session every time, the only one that accepts it, and a Files job rides
/// its own service's link, whichever the sessions were stored first.
#[test]
fn job_control_rides_the_session_that_carries_the_job() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    let (controller, host) = (&pair.controller.core, &pair.host.core);
    controller.connect(&pair.host.endpoint())?;
    let (owner, _pipe) = pair.controller.open_files(&pair.connect_services())?;
    let receiver_of = |name: &str| {
        wait_for("the host's job", DEFAULT_WAIT, || {
            host.job_list()
                .ok()?
                .into_iter()
                .find(|record| record.label == name)
                .map(|record| record.id)
        })
    };
    for index in 0..4u64 {
        let name = format!("plain-{index}.bin");
        let bytes = payload(2048, 300 + index);
        std::fs::write(pair.controller.root().join(&name), &bytes)?;
        let job = files::upload(&pair, &name, "")?;
        let (sender, got) = drive_to_end(controller, job, host, receiver_of(&name), TRANSFER);
        assert_eq!(
            (sender.state, got.state),
            (JobState::Complete, JobState::Complete),
            "a job no Files service owns is driven over the control session"
        );
        assert_eq!(
            std::fs::read(pair.host.root().join(&name))?,
            bytes,
            "{name} landed exactly"
        );
    }
    let bytes = payload(2048, 399);
    std::fs::write(pair.controller.root().join("owned.bin"), &bytes)?;
    let job = controller.files_service_upload(
        owner.id,
        owner.generation,
        pair.controller.root_id(),
        "",
        &["owned.bin".to_owned()],
        pair.host.root_id(),
        "",
    )?;
    let (sender, got) = drive_to_end(controller, job, host, receiver_of("owned.bin"), TRANSFER);
    assert_eq!(
        (sender.state, got.state),
        (JobState::Complete, JobState::Complete),
        "a Files job is driven over its own service's link"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("owned.bin"))?,
        bytes,
        "the owned file landed exactly"
    );
    Ok(())
}

/// Several files and folders chosen in one folder are one transfer: one
/// manifest holding each at its own name, landing side by side, both ways.
/// Its destination names each item and whether it is a folder; only the
/// chosen entries, not their siblings, are held against renaming; and an
/// item chosen twice is refused.
#[test]
fn a_selection_is_one_transfer_both_ways() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (controller, host) = (&pair.controller.core, &pair.host.core);
    let (controller_root, host_root) = (pair.controller.root_id(), pair.host.root_id());
    let pick = pair.controller.root().join("pick");
    std::fs::create_dir_all(pick.join("Photos"))?;
    std::fs::write(pick.join("a.txt"), payload(700, 501))?;
    std::fs::write(pick.join("Photos/one.jpg"), payload(900, 502))?;
    std::fs::write(pick.join("Photos/two.jpg"), payload(1100, 503))?;
    std::fs::write(pick.join("left.txt"), b"not chosen")?;
    let (owner, _pipe) = pair.controller.open_files(&pair.connect_control())?;
    let names = ["a.txt".to_owned(), "Photos".to_owned()];
    assert!(
        controller
            .files_service_upload(
                owner.id,
                owner.generation,
                controller_root,
                "pick",
                &["a.txt".to_owned(), "a.txt".to_owned()],
                host_root,
                "",
            )
            .is_err(),
        "an item chosen twice is refused"
    );
    let up = controller.files_service_upload(
        owner.id,
        owner.generation,
        controller_root,
        "pick",
        &names,
        host_root,
        "",
    )?;
    let sent = controller.job_snapshot(up)?;
    assert_eq!(sent.files_total, 3, "one manifest holds every chosen file");
    assert_eq!(
        sent.label, "Photos/one.jpg +2 more",
        "a selection is labelled by its first file"
    );
    assert_eq!(
        sent.destination.map(|destination| destination.items),
        Some(vec![
            JobItem {
                name: "a.txt".into(),
                folder: false
            },
            JobItem {
                name: "Photos".into(),
                folder: true
            },
        ]),
        "the destination names each item and whether it is a folder"
    );
    let rename =
        |name: &str, to: &str| controller.rename_entry(controller_root, "pick", name, None, to);
    assert!(rename("a.txt", "b.txt").is_err(), "a chosen file moved");
    assert!(
        rename("Photos", "Pictures").is_err(),
        "a chosen folder moved"
    );
    rename("left.txt", "still-left.txt")?;
    let receiver = received(host);
    let (sender, got) = drive_to_end(controller, up, host, receiver, TRANSFER);
    assert_eq!(
        (sender.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the upload completes"
    );
    assert_eq!(got.files_done, 3, "every chosen file landed");
    assert_eq!(sender.files_done, 3, "the sender heard what landed");
    assert_eq!(
        std::fs::read(pair.host.root().join("a.txt"))?,
        payload(700, 501),
        "a chosen file lands at its own name"
    );
    assert_eq!(
        tree(&pair.host.root().join("Photos")),
        tree(&pick.join("Photos")),
        "a chosen folder lands whole"
    );
    assert!(
        !pair.host.root().join("still-left.txt").exists(),
        "a sibling not chosen stays behind"
    );

    std::fs::create_dir_all(pair.controller.root().join("back"))?;
    let down = controller.files_service_download(
        owner.id,
        owner.generation,
        host_root,
        "",
        &names,
        controller_root,
        "back",
    )?;
    let host_job = wait_for("the host's sending job", DEFAULT_WAIT, || {
        host.job_list()
            .ok()?
            .into_iter()
            .find(|job| job.sender)
            .map(|job| job.id)
    });
    let (got, sender) = drive_to_end(controller, down, host, host_job, TRANSFER);
    assert_eq!(
        (got.state, sender.state),
        (JobState::Complete, JobState::Complete),
        "the download completes"
    );
    assert_eq!(got.files_done, 3, "every chosen file came back");
    assert_eq!(
        std::fs::read(pair.controller.root().join("back/a.txt"))?,
        payload(700, 501),
        "a chosen file comes back at its own name"
    );
    assert_eq!(
        tree(&pair.controller.root().join("back/Photos")),
        tree(&pick.join("Photos")),
        "a chosen folder comes back whole"
    );
    trace.wait_for_records("file received", 6);
    // Each job fetches its small files side by side.
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1100 size=1100
          files: file served ended=sent from=0 sent=700 size=700
          files: file served ended=sent from=0 sent=900 size=900
          files: job state done_bytes=2700 files_done=3 from=serving job=1 sender=true to=complete
          files: file received outcome=received resumed_at=0 size=1100 took=1100
          files: file received outcome=received resumed_at=0 size=700 took=700
          files: file received outcome=received resumed_at=0 size=900 took=900
          files: job state done_bytes=2700 files_done=3 from=transferring job=2 sender=false to=complete
        host:
          files: file received outcome=received resumed_at=0 size=1100 took=1100
          files: file received outcome=received resumed_at=0 size=700 took=700
          files: file received outcome=received resumed_at=0 size=900 took=900
          files: job state done_bytes=2700 files_done=3 from=transferring job=1 sender=false to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: file served ended=sent from=0 sent=1100 size=1100
          files: file served ended=sent from=0 sent=700 size=700
          files: file served ended=sent from=0 sent=900 size=900
          files: job state done_bytes=2700 files_done=3 from=serving job=2 sender=true to=complete
    "#]].assert_eq(&trace.concurrent_story(&["files"]));
    Ok(())
}

/// Diagnostics count what a finished transfer moved on each side, and a
/// finished job no longer counts as active.
#[test]
fn diagnostics_count_a_finished_transfer() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    let bytes = payload(200 * 1024, 1901);
    std::fs::write(pair.controller.root().join("counted.bin"), &bytes)?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = files::upload(&pair, "counted.bin", "arrived")?;
    let receiver = received(&pair.host.core);
    let (sender, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        (sender.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the transfer completes"
    );

    let received = pair.host.core.diagnostic_snapshot()?;
    assert_eq!(
        received.transfers.files_fetched, 1,
        "the receiver counts its fetch"
    );
    assert_eq!(
        received.transfers.bytes_received,
        bytes.len() as u64,
        "the receiver counts the bytes it took"
    );
    assert!(
        received.transfers.segments_checked >= 1,
        "the receiver counts the segments it checked"
    );
    assert_eq!(
        received.transfers.segments_mismatched, 0,
        "no segment of an exact transfer mismatched"
    );
    assert_eq!(
        received.resources.active_jobs, 0,
        "a finished receiver is not active"
    );
    let sent = pair.controller.core.diagnostic_snapshot()?;
    assert_eq!(
        sent.transfers.files_served, 1,
        "the sender counts its serve"
    );
    assert_eq!(
        sent.transfers.bytes_sent,
        bytes.len() as u64,
        "the sender counts the bytes it sent"
    );
    assert_eq!(
        sent.resources.active_jobs, 0,
        "a finished sender is not active"
    );
    Ok(())
}
