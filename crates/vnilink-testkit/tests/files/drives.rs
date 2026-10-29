//! A drive taken out or replaced under a transfer, as a stick is on
//! Windows: what is interrupted, refused or kept, and what resumes. A
//! transfer is pinned to the drive it began on, so another volume at the
//! drive's letter never receives or serves its bytes.
//!
//! Jobs here are driven by hand (`job_poll`, `drive_to_end`), so a drive
//! changes between two exact files of a transfer.

use expect_test::expect;
use std::time::Duration;

use crate::shared::ServeProbe;
use vnilink::{ConflictChoice, Core, Error, JobState};
use vnilink_testkit::files::{
    drive_to_end, only_job, payload, poll_within, received, source, Location, StandInDrive,
};
use vnilink_testkit::{traced, wait_for, Files, Pair, TestResult, DEFAULT_WAIT};

const TRANSFER: Duration = Duration::from_secs(30);

/// What a job on a drive that changed says when it fails.
const DRIVE_CHANGED: &str = "the drive changed during the transfer";

/// The controller fetches `path` below `from` on the host into `to` below
/// `into` on its own drive, as a job of `files`.
fn download(
    core: &Core,
    files: &Files,
    from: &Location,
    path: &str,
    into: &Location,
    to: &str,
) -> Result<u64, Error> {
    let (folder, names) = source(&from.path(path));
    core.files_service_download(
        files.service.id,
        files.service.generation,
        from.root,
        &folder,
        &names,
        into.root,
        &into.path(to),
    )
}

/// The controller sends `path` below `from` on its own drive to `to` below
/// `into` on the host, as a job of `files`.
fn upload(
    core: &Core,
    files: &Files,
    from: &Location,
    path: &str,
    into: &Location,
    to: &str,
) -> Result<u64, Error> {
    let (folder, names) = source(&from.path(path));
    core.files_service_upload(
        files.service.id,
        files.service.generation,
        from.root,
        &folder,
        &names,
        into.root,
        &into.path(to),
    )
}

/// A receiver's drive replaced mid-download by an empty volume interrupts
/// the job before it fetches again, writing nothing to either volume; with
/// the drive put back, the job resumes to the end.
#[test]
fn a_receiving_drive_replaced_mid_download_interrupts_without_writing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let drive = StandInDrive::share(controller, "drive");
    let one = payload(512, 1605);
    let two = payload(512, 1606);
    std::fs::create_dir_all(host.folder().join("set"))?;
    std::fs::write(host.folder().join("set/one.txt"), &one)?;
    std::fs::write(host.folder().join("set/two.txt"), &two)?;
    let files = pair.open_files();
    let (there, here) = (host.location(), drive.location(controller));
    controller
        .core
        .create_dir(here.root, &here.path(""), "got")?;
    let job = download(&controller.core, &files, &there, "set", &here, "got")?;
    poll_within(&controller.core, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let host_job = host.core.job_list()?[0].id;
    assert_eq!(
        std::fs::read(drive.files().join("got/set/one.txt"))?,
        one,
        "the first file lands exactly"
    );
    // The check of the drive precedes the fetch, so on the other volume no
    // fetch runs and no folder or file is made.
    let original = drive.replace(controller);
    poll_within(
        &controller.core,
        job,
        10,
        "the job to be interrupted",
        |s| s.state == JobState::Interrupted,
    );
    assert_eq!(
        std::fs::read_dir(drive.files())?.count(),
        0,
        "nothing is written to the other volume"
    );
    assert!(
        !original.join("files/got/set/two.txt").exists(),
        "the second file does not land on the drive taken out"
    );
    drive.put_back(controller, &original);
    controller.core.job_resume(job)?;
    let (got, sent) = drive_to_end(&controller.core, job, &host.core, host_job, TRANSFER);
    assert_eq!(
        (got.state, sent.state),
        (JobState::Complete, JobState::Complete),
        "with its drive back, the job completes on both sides"
    );
    assert_eq!(
        std::fs::read(drive.files().join("got/set/two.txt"))?,
        two,
        "the second file lands exactly"
    );
    // The second file is fetched beside the first, so it has either
    // arrived or is cancelled when the drive is found replaced: what its
    // fetches record differs, the jobs' story does not.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
    "#]],
        || trace.tell(&["files"]).only(&["job state"]).to_string(),
    );
    Ok(())
}

/// A Replace chosen for a name applies to the file on the drive it was
/// chosen for: with another volume at the drive's letter holding a file of
/// that name, the job is interrupted and touches neither; with the drive
/// back, it replaces the original.
#[test]
fn a_replace_choice_applies_only_on_the_drive_it_was_made_for() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let drive = StandInDrive::share(controller, "drive");
    let remote = payload(512, 1610);
    std::fs::create_dir_all(host.folder().join("set"))?;
    std::fs::write(host.folder().join("set/one.txt"), &remote)?;
    std::fs::create_dir_all(drive.files().join("got/set"))?;
    std::fs::write(
        drive.files().join("got/set/one.txt"),
        b"original-local-content",
    )?;
    let files = pair.open_files();
    let (there, here) = (host.location(), drive.location(controller));
    let job = download(&controller.core, &files, &there, "set", &here, "got")?;
    assert_eq!(
        controller.core.job_poll(job)?.state,
        JobState::WaitingDecision,
        "a name already there waits for a choice"
    );
    controller
        .core
        .job_resolve(job, ConflictChoice::Replace, false)?;
    let original = drive.swap();
    let valuable = payload(512, 1611);
    std::fs::create_dir_all(drive.files().join("got/set"))?;
    std::fs::write(drive.files().join("got/set/one.txt"), &valuable)?;
    drive.read_again(controller);
    poll_within(
        &controller.core,
        job,
        10,
        "the job to be interrupted",
        |s| s.state == JobState::Interrupted,
    );
    assert_eq!(
        std::fs::read(drive.files().join("got/set/one.txt"))?,
        valuable,
        "the choice does not replace a file on another volume"
    );
    assert_eq!(
        std::fs::read(original.join("files/got/set/one.txt"))?,
        b"original-local-content",
        "nor the file it was made for while its drive is out"
    );
    let other = drive.put_back(controller, &original);
    controller.core.job_resume(job)?;
    let host_job = host.core.job_list()?[0].id;
    let (got, sent) = drive_to_end(&controller.core, job, &host.core, host_job, TRANSFER);
    assert_eq!(
        (got.state, sent.state),
        (JobState::Complete, JobState::Complete),
        "with its drive back, the job completes on both sides"
    );
    assert_eq!(
        std::fs::read(drive.files().join("got/set/one.txt"))?,
        remote,
        "the choice replaces the file it was made for"
    );
    assert_eq!(
        std::fs::read(other.join("files/got/set/one.txt"))?,
        valuable,
        "the other volume's file is untouched"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=512 files_done=1 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=512 files_done=1 from=serving job=1 sender=true to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A Replace saved before a restart does not apply to another volume at
/// the drive's letter: resuming interrupts the job, and neither the new
/// file nor the original is touched.
#[test]
fn a_saved_replace_choice_refuses_another_volume_at_its_drive() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let drive = StandInDrive::share(&pair.controller, "drive");
    std::fs::create_dir_all(pair.host.folder().join("set"))?;
    std::fs::write(pair.host.folder().join("set/one.txt"), payload(512, 1701))?;
    std::fs::create_dir_all(drive.files().join("got/set"))?;
    std::fs::write(drive.files().join("got/set/one.txt"), b"original")?;
    let controller = &pair.controller.core;
    let files = pair.open_files();
    let (there, here) = (pair.host.location(), drive.location(&pair.controller));
    let job = download(controller, &files, &there, "set", &here, "got")?;
    assert_eq!(
        controller.job_poll(job)?.state,
        JobState::WaitingDecision,
        "a name already there waits for a choice"
    );
    controller.job_resolve(job, ConflictChoice::Replace, false)?;
    drop(files);
    controller.shutdown();
    let original = drive.swap();
    std::fs::create_dir_all(drive.files().join("got/set"))?;
    std::fs::write(
        drive.files().join("got/set/one.txt"),
        b"valuable-replacement",
    )?;
    pair.controller.restart();
    drive.read_again(&pair.controller);
    let controller = &pair.controller.core;
    // The restarted controller's Files are new ones: they adopt the job
    // before resuming it.
    let files = pair.open_files();
    controller.files_service_adopt_job(files.service.id, files.service.generation, job)?;
    controller.job_resume(job)?;
    assert_eq!(
        controller.job_poll(job)?.state,
        JobState::Interrupted,
        "the saved choice does not apply to another volume"
    );
    assert_eq!(
        std::fs::read(drive.files().join("got/set/one.txt"))?,
        b"valuable-replacement",
        "the other volume's file is untouched"
    );
    assert_eq!(
        std::fs::read(original.join("files/got/set/one.txt"))?,
        b"original",
        "the original drive's file is untouched"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A sender that restarts to find another volume at its drive's letter,
/// holding identical copies, fails the resume and says the drive changed:
/// only the drive the job was offered from may serve it, and nothing more
/// lands.
#[test]
fn a_sender_restarted_onto_another_volume_fails_the_resume_and_says_why() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let drive = StandInDrive::share(&pair.host, "drive");
    let one = payload(512, 1607);
    let two = payload(512, 1608);
    std::fs::create_dir_all(drive.files().join("set"))?;
    std::fs::write(drive.files().join("set/one.txt"), &one)?;
    std::fs::write(drive.files().join("set/two.txt"), &two)?;
    let controller = &pair.controller.core;
    let files = pair.open_files();
    let (there, here) = (drive.location(&pair.host), pair.controller.location());
    controller.create_dir(here.root, &here.path(""), "got")?;
    let job = download(controller, &files, &there, "set", &here, "got")?;
    poll_within(controller, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let sender_job = pair.host.core.job_list()?[0].id;
    // The job's pin on its drive is saved, so after the restart only it can
    // fail the resume: the copies are identical.
    pair.host.core.shutdown();
    wait_for(
        "the receiver to learn that its sender shut down",
        Duration::from_secs(1),
        || (controller.job_poll(job).ok()?.state == JobState::Interrupted).then_some(()),
    );
    drop(files);
    drive.swap();
    std::fs::create_dir_all(drive.files().join("set"))?;
    std::fs::write(drive.files().join("set/one.txt"), &one)?;
    std::fs::write(drive.files().join("set/two.txt"), &two)?;
    pair.host.restart();
    drive.read_again(&pair.host);
    let host = &pair.host.core;
    let jobs = host.job_list()?;
    assert_eq!(jobs.len(), 1, "the sender's job survives the restart");
    assert_eq!(
        jobs[0].state,
        JobState::Interrupted,
        "the sender's job is interrupted after the restart"
    );
    // A job the host serves is resumed by the controller's Files, new ones
    // once the host is back: they adopt the job, and resuming it asks the
    // host to serve again from the drive it offered.
    pair.reintroduce();
    let files = pair.open_files();
    controller.files_service_adopt_job(files.service.id, files.service.generation, job)?;
    controller.job_resume(job)?;
    let resumed = wait_for("the sender to answer the resume", DEFAULT_WAIT, || {
        let snapshot = host.job_snapshot(sender_job).ok()?;
        snapshot.state.is_terminal().then_some(snapshot)
    });
    assert_eq!(
        resumed.state,
        JobState::Failed,
        "a job whose drive changed fails its resume"
    );
    assert_eq!(
        resumed.error.as_deref(),
        Some(DRIVE_CHANGED),
        "the failure says the drive changed"
    );
    let got = pair.controller.folder().join("got/set");
    assert_eq!(
        std::fs::read(got.join("one.txt"))?,
        one,
        "the first file stays"
    );
    assert!(
        !got.join("two.txt").exists(),
        "nothing from the other volume lands"
    );
    // The second file is fetched beside the first, so whether it arrived
    // before the host shut down differs between runs; the jobs' story does
    // not.
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state done_bytes=0 files_done=0 from=interrupted job=1 sender=true to=failed
    "#]],
        || trace.tell(&["files"]).only(&["job state"]).to_string(),
    );
    Ok(())
}

/// A sender's drive replaced mid-upload, even by a volume with identical
/// copies of its files, interrupts the job before the next file lands:
/// only the drive it offered from may serve it. With the drive back, the
/// upload resumes with the original bytes.
#[test]
fn a_sending_drive_replaced_mid_upload_interrupts_and_resumes_once_back() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let drive = StandInDrive::share(&pair.controller, "drive");
    // The host receives one file at a time, to stop between the two.
    host.fetch_one_file_at_a_time();
    let one = payload(512, 1603);
    let two = payload(512, 1604);
    std::fs::create_dir_all(drive.files().join("set"))?;
    std::fs::write(drive.files().join("set/one.txt"), &one)?;
    std::fs::write(drive.files().join("set/two.txt"), &two)?;
    let files = pair.open_files();
    let (here, there) = (drive.location(&pair.controller), pair.host.location());
    let job = upload(controller, &files, &here, "set", &there, "arrived")?;
    let receiver = received(host);
    poll_within(host, receiver, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let arrived = pair.host.folder().join("arrived/set");
    assert_eq!(
        std::fs::read(arrived.join("one.txt"))?,
        one,
        "the first file lands"
    );

    // Identical copies would pass a content check: only the job's pin to
    // the drive it offered from can stop the second file.
    let original = drive.swap();
    std::fs::create_dir_all(drive.files().join("set"))?;
    std::fs::write(drive.files().join("set/one.txt"), &one)?;
    std::fs::write(drive.files().join("set/two.txt"), &two)?;
    drive.read_again(&pair.controller);
    poll_within(host, receiver, 10, "the job to be interrupted", |s| {
        s.state == JobState::Interrupted
    });
    assert!(
        !arrived.join("two.txt").exists(),
        "bytes from another volume never land"
    );
    assert_eq!(
        controller.job_snapshot(job)?.state,
        JobState::Serving,
        "the sender mirrors its receiver only when polled"
    );

    drive.put_back(&pair.controller, &original);
    host.job_resume(receiver)?;
    let (sent, got) = drive_to_end(controller, job, host, receiver, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "with its drive back, the upload completes on both sides"
    );
    assert_eq!(
        std::fs::read(arrived.join("two.txt"))?,
        two,
        "the second file lands with the original bytes"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A serve that opened its source before the sender's drive is replaced
/// finishes with the bytes it opened, not the other volume's decoy of the
/// same size: only opens after the change are refused.
#[test]
fn a_source_opened_before_its_drive_changed_serves_its_own_bytes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let drive = StandInDrive::share(&pair.controller, "drive");
    // The host receives one file at a time, to park the second one's serve.
    host.fetch_one_file_at_a_time();
    let one = payload(512, 1601);
    let two = payload(512, 1602);
    std::fs::create_dir_all(drive.files().join("set"))?;
    std::fs::write(drive.files().join("set/one.txt"), &one)?;
    std::fs::write(drive.files().join("set/two.txt"), &two)?;
    let files = pair.open_files();
    let (here, there) = (drive.location(&pair.controller), pair.host.location());
    let job = upload(controller, &files, &here, "set", &there, "arrived")?;
    let receiver = only_job(host)?;
    poll_within(host, receiver, 20, "the first file to land", |s| {
        s.files_done == 1
    });

    let probe = ServeProbe::arm(controller, job);
    let polled = std::thread::scope(|scope| {
        let polling = scope.spawn(|| host.job_poll(receiver));
        probe.wait_until_parked();
        // Another volume at the letter, its second file a decoy of the same
        // size and other bytes. The drive's folder stays put: Windows will
        // not move a folder holding the file the serve has open.
        let volume = pair.controller.folder().with_file_name("other-volume");
        let other = volume.join("files/set");
        std::fs::create_dir_all(&other).expect("the decoy's folder");
        std::fs::write(other.join("one.txt"), &one).expect("a copy");
        std::fs::write(other.join("two.txt"), payload(512, 1609)).expect("the decoy");
        drive.share_other(&pair.controller, &volume);
        probe.release();
        polling.join().expect("the receiver's poll")
    })?;
    drop(probe);
    assert_eq!(
        polled.files_done, 2,
        "the open serve finishes the second file across the change"
    );
    assert_eq!(
        std::fs::read(pair.host.folder().join("arrived/set/two.txt"))?,
        two,
        "the open serve sent the bytes it opened, not the decoy"
    );
    let (sent, got) = drive_to_end(controller, job, host, receiver, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the job completes on both sides"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
