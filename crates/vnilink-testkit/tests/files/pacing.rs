//! How transfers pace themselves: a small file promptly, many skipped files
//! without a pass each, a waiting transfer that sleeps, small files fetched
//! ahead and landed in order, and what diagnostics count.

use expect_test::expect;
use std::time::{Duration, Instant};

use vnilink::{ConflictChoice, JobDestination, JobItem, JobState};
use vnilink_testkit::files::{
    drive, drive_to_end, finished, only_job, payload, received, settled, tree, upload,
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
/// the second rides the same Files service as the first.
#[test]
fn a_small_file_completes_promptly_on_its_own() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    drive(&pair);
    let files = pair.open_files();
    for name in ["first.bin", "second.bin"] {
        std::fs::write(pair.controller.folder().join(name), vec![7u8; 2500])?;
        // Submitting scans and offers the file: it counts.
        let started = std::time::Instant::now();
        let job = upload(&pair, &files, name, "")?;
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
            std::fs::read(pair.host.folder().join(name))?.len(),
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
    std::fs::write(pair.controller.folder().join("carry.bin"), &bytes)?;
    let session = pair.connect_services();
    let (lost, pipe) = pair.controller.open_files(&session)?;
    let job = controller.files_service_upload(
        lost.id,
        lost.generation,
        pair.controller.location().root,
        &pair.controller.location().path(""),
        &["carry.bin".to_owned()],
        pair.host.location().root,
        &pair.host.location().path("inbox"),
    )?;
    assert_eq!(
        controller.job_snapshot(job)?.destination,
        Some(JobDestination {
            root: pair.host.location().root,
            folder: pair.host.location().path("inbox"),
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
        std::fs::read(pair.host.folder().join("inbox/carry.bin"))?,
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
    let (_owner, pipe) = pair.controller.open_files(&pair.connect_services())?;
    let here = pair.host.location();
    pipe.watch(Some((here.root, here.path(""))));
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
                std::fs::write(pair.host.folder().join(name), b"new").ok()?;
                made = Some(Instant::now());
            }
            (pipe.folder_revision() > before).then_some(())
        },
    );
    assert!(
        pipe.list(here.root, &here.path(""), 0, 100)?
            .entries
            .iter()
            .any(|entry| entry.name == "arrived-1.txt"),
        "the pipe is not held by the wait"
    );
    pipe.watch(None);
    let stopped = pipe.folder_revision();
    std::fs::write(pair.host.folder().join("later.txt"), b"later")?;
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
        let folder = side.folder().join("many");
        std::fs::create_dir_all(&folder)?;
        for index in 0..FILES {
            std::fs::write(folder.join(format!("f-{index:04}.txt")), b"x")?;
        }
    }
    let files = pair.open_files();
    drive(&pair);
    let job = upload(&pair, &files, "many", "")?;
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
    trace.wait_for_records_with("job state", &[("engine", "host"), ("to", "complete")], 1);
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
    std::fs::write(pair.controller.folder().join("same.txt"), b"theirs")?;
    std::fs::write(pair.host.folder().join("same.txt"), b"mine")?;
    let files = pair.open_files();
    drive(&pair);
    let job = upload(&pair, &files, "same.txt", "")?;
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
        std::fs::read(pair.host.folder().join("same.txt"))?,
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
        std::fs::read(pair.host.folder().join("same.txt"))?,
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
    let (mine, theirs) = (pair.controller.location(), pair.host.location());
    let pick = pair.controller.folder().join("pick");
    std::fs::create_dir_all(pick.join("Photos"))?;
    std::fs::write(pick.join("a.txt"), payload(700, 501))?;
    std::fs::write(pick.join("Photos/one.jpg"), payload(900, 502))?;
    std::fs::write(pick.join("Photos/two.jpg"), payload(1100, 503))?;
    std::fs::write(pick.join("left.txt"), b"not chosen")?;
    let (owner, _pipe) = pair.controller.open_files(&pair.connect_services())?;
    let names = ["a.txt".to_owned(), "Photos".to_owned()];
    assert!(
        controller
            .files_service_upload(
                owner.id,
                owner.generation,
                mine.root,
                &mine.path("pick"),
                &["a.txt".to_owned(), "a.txt".to_owned()],
                theirs.root,
                &theirs.path(""),
            )
            .is_err(),
        "an item chosen twice is refused"
    );
    let up = controller.files_service_upload(
        owner.id,
        owner.generation,
        mine.root,
        &mine.path("pick"),
        &names,
        theirs.root,
        &theirs.path(""),
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
    let rename = |name: &str, to: &str| {
        controller.rename_entry(mine.root, &mine.path("pick"), name, None, to)
    };
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
        std::fs::read(pair.host.folder().join("a.txt"))?,
        payload(700, 501),
        "a chosen file lands at its own name"
    );
    assert_eq!(
        tree(&pair.host.folder().join("Photos")),
        tree(&pick.join("Photos")),
        "a chosen folder lands whole"
    );
    assert!(
        !pair.host.folder().join("still-left.txt").exists(),
        "a sibling not chosen stays behind"
    );

    std::fs::create_dir_all(pair.controller.folder().join("back"))?;
    let down = controller.files_service_download(
        owner.id,
        owner.generation,
        theirs.root,
        &theirs.path(""),
        &names,
        mine.root,
        &mine.path("back"),
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
        std::fs::read(pair.controller.folder().join("back/a.txt"))?,
        payload(700, 501),
        "a chosen file comes back at its own name"
    );
    assert_eq!(
        tree(&pair.controller.folder().join("back/Photos")),
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
    std::fs::write(pair.controller.folder().join("counted.bin"), &bytes)?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "counted.bin", "arrived")?;
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

/// While a job takes one file, it fetches the next small files ahead, so a
/// folder of small files lands without a round trip per file; they still
/// land in order and exactly. Cancelling stops what is ahead.
#[test]
fn small_files_arrive_ahead_and_land_in_order() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let many: Vec<(String, usize)> = (0..20)
        .map(|n| (format!("many/f{n:02}.bin"), 3000))
        .collect();
    write_named(&pair.controller.folder(), &many, 2000);
    let files = pair.open_files();
    let job = upload(&pair, &files, "many", "")?;
    let receiver = only_job(host)?;

    let first = host.job_poll(receiver)?;
    assert_eq!(first.files_done, 1, "the first poll lands the first file");
    assert_eq!(
        host.fetching_ahead(receiver),
        7,
        "seven files are fetched ahead of the second"
    );
    let second = host.job_poll(receiver)?;
    assert_eq!(
        second.files_done, 2,
        "each later poll lands exactly the next file"
    );
    let (sent, got) = drive_to_end(controller, job, host, receiver, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the folder completes on both sides"
    );
    assert_eq!(
        tree(&pair.host.folder().join("many")),
        tree(&pair.controller.folder().join("many")),
        "the small files land exactly"
    );
    assert_eq!(
        host.fetching_ahead(receiver),
        0,
        "a finished job fetches nothing ahead"
    );

    // A second job, cancelled with files ahead, stops them.
    let again: Vec<(String, usize)> = (0..10)
        .map(|n| (format!("again/g{n:02}.bin"), 3000))
        .collect();
    write_named(&pair.controller.folder(), &again, 2100);
    upload(&pair, &files, "again", "")?;
    let receiver = host
        .job_list()?
        .into_iter()
        .find(|listed| !listed.sender && listed.state != JobState::Complete)
        .ok_or("the second job's receiver")?
        .id;
    host.job_poll(receiver)?;
    assert!(
        host.fetching_ahead(receiver) > 0,
        "the second job fetches ahead"
    );
    host.job_cancel(receiver)?;
    assert_eq!(
        host.fetching_ahead(receiver),
        0,
        "cancelling stops the files fetched ahead"
    );
    trace.wait_for_records("job state", 4);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=60000 files_done=20 from=serving job=1 sender=true to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=2 sender=true to=cancelled
        host:
          files: job state done_bytes=60000 files_done=20 from=transferring job=1 sender=false to=complete
          files: job state done_bytes=3000 files_done=1 from=transferring job=2 sender=false to=cancelled
    "#]].assert_eq(&trace.tell(&["files"]).only(&["job state"]).to_string());
    Ok(())
}

/// Writes each of `files` (relative path, size) under `folder`, the n-th
/// with the contents of seed `first_seed + n`.
fn write_named(folder: &std::path::Path, files: &[(String, usize)], first_seed: u64) {
    for (n, (path, size)) in files.iter().enumerate() {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().expect("a file in a folder")).expect("a folder");
        std::fs::write(path, payload(*size, first_seed + n as u64)).expect("a file");
    }
}
