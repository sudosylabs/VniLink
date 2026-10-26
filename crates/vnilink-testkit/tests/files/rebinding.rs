//! A root rebound or replaced under a transfer: what is interrupted, refused
//! or kept, and what resumes.
//!
//! Jobs here are driven by hand (`job_poll`, `drive_to_end`), so a root
//! changes between two exact files of a transfer.

use expect_test::expect;
use std::path::PathBuf;
use std::time::Duration;

use crate::shared::ServeProbe;
use vnilink::{ConflictChoice, JobState};
use vnilink_testkit::files::{self, drive_to_end, payload};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(30);

/// What a job rebound under says when it fails.
const ROOTS_REBOUND: &str = "the file roots were rebound during the transfer";

/// A folder beside `engine`'s root, outside it, to rebind to.
fn beside(engine: &Engine, name: &str) -> PathBuf {
    engine
        .root()
        .parent()
        .expect("a root inside the engine's folder")
        .join(name)
}

/// A receiver's root rebound to an empty folder mid-download interrupts the
/// job before it fetches again, writing nothing under either root; with
/// the root restored, the job resumes to the end.
#[test]
fn a_receiving_root_rebound_mid_download_interrupts_without_writing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let one = payload(512, 1605);
    let two = payload(512, 1606);
    std::fs::write(host.root().join("one.txt"), &one)?;
    std::fs::write(host.root().join("two.txt"), &two)?;
    controller.core.connect_files(&host.endpoint())?;
    controller
        .core
        .create_dir(controller.root_id(), "", "got")?;
    let job = files::download(&pair, "", "got")?;
    files::poll_within(&controller.core, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let host_job = host.core.job_list()?[0].id;
    let got = controller.root().join("got");
    assert_eq!(
        std::fs::read(got.join("one.txt"))?,
        one,
        "the first file lands exactly"
    );
    // The conflict check precedes the fetch, so under the new root no fetch
    // runs and no folder or file is made.
    let rebound = beside(controller, "rebound");
    std::fs::create_dir_all(&rebound)?;
    controller.core.set_file_roots(vec![rebound.clone()])?;
    files::poll_within(
        &controller.core,
        job,
        10,
        "the job to be interrupted",
        |s| s.state == JobState::Interrupted,
    );
    assert_eq!(
        std::fs::read_dir(&rebound)?.count(),
        0,
        "nothing is written under the rebound root"
    );
    assert!(
        !got.join("two.txt").exists(),
        "the second file does not land under the old root"
    );
    controller.core.set_file_roots(vec![controller.root()])?;
    controller.core.job_resume(job)?;
    let (got_job, sent) = drive_to_end(&controller.core, job, &host.core, host_job, TRANSFER);
    assert_eq!(
        (got_job.state, sent.state),
        (JobState::Complete, JobState::Complete),
        "with its root restored, the job completes on both sides"
    );
    assert_eq!(
        std::fs::read(got.join("two.txt"))?,
        two,
        "the second file lands exactly"
    );
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=512 size=512 took=0
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=512 sent=0 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A Replace chosen for a name applies to the file under the root it was
/// chosen for: rebound to a folder with a file of that name, the job is
/// interrupted and touches neither; restored, it replaces the original.
#[test]
fn a_replace_choice_applies_only_under_the_root_it_was_made_for() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let remote = payload(512, 1610);
    std::fs::write(host.root().join("one.txt"), &remote)?;
    let got = controller.root().join("got");
    std::fs::create_dir_all(&got)?;
    std::fs::write(got.join("one.txt"), b"original-local-content")?;
    controller.core.connect_files(&host.endpoint())?;
    let job = files::download(&pair, "", "got")?;
    assert_eq!(
        controller.core.job_poll(job)?.state,
        JobState::WaitingDecision,
        "a name already there waits for a choice"
    );
    controller
        .core
        .job_resolve(job, ConflictChoice::Replace, false)?;
    let rebound = beside(controller, "rebound");
    std::fs::create_dir_all(rebound.join("got"))?;
    let valuable = payload(512, 1611);
    std::fs::write(rebound.join("got/one.txt"), &valuable)?;
    controller.core.set_file_roots(vec![rebound.clone()])?;
    files::poll_within(
        &controller.core,
        job,
        10,
        "the job to be interrupted",
        |s| s.state == JobState::Interrupted,
    );
    assert_eq!(
        std::fs::read(rebound.join("got/one.txt"))?,
        valuable,
        "the choice does not replace a file under another root"
    );
    assert_eq!(
        std::fs::read(got.join("one.txt"))?,
        b"original-local-content",
        "nor the file it was made for while rebound"
    );
    controller.core.set_file_roots(vec![controller.root()])?;
    controller.core.job_resume(job)?;
    let host_job = host.core.job_list()?[0].id;
    let (got_job, sent) = drive_to_end(&controller.core, job, &host.core, host_job, TRANSFER);
    assert_eq!(
        (got_job.state, sent.state),
        (JobState::Complete, JobState::Complete),
        "with its root restored, the job completes on both sides"
    );
    assert_eq!(
        std::fs::read(got.join("one.txt"))?,
        remote,
        "the choice replaces the file it was made for"
    );
    assert_eq!(
        std::fs::read(rebound.join("got/one.txt"))?,
        valuable,
        "the other root's file is untouched"
    );
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
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A Replace saved before a restart does not apply to a root replaced at the
/// same path: resuming interrupts the job, and neither the new file nor the
/// original is touched.
#[test]
fn a_saved_replace_choice_refuses_a_root_replaced_at_the_same_path() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    std::fs::write(pair.host.root().join("one.txt"), payload(512, 1701))?;
    let root = pair.controller.root();
    std::fs::create_dir_all(root.join("got"))?;
    std::fs::write(root.join("got/one.txt"), b"original")?;
    let controller = &pair.controller.core;
    controller.connect_files(&pair.host.endpoint())?;
    let job = files::download(&pair, "", "got")?;
    assert_eq!(
        controller.job_poll(job)?.state,
        JobState::WaitingDecision,
        "a name already there waits for a choice"
    );
    controller.job_resolve(job, ConflictChoice::Replace, false)?;
    controller.shutdown();
    let original = beside(&pair.controller, "original-root");
    std::fs::rename(&root, &original)?;
    std::fs::create_dir_all(root.join("got"))?;
    std::fs::write(root.join("got/one.txt"), b"valuable-replacement")?;
    pair.controller.restart();
    let controller = &pair.controller.core;
    controller.connect_files(&pair.host.endpoint())?;
    controller.job_resume(job)?;
    assert_eq!(
        controller.job_poll(job)?.state,
        JobState::Interrupted,
        "the saved choice does not apply to the replaced root"
    );
    assert_eq!(
        std::fs::read(root.join("got/one.txt"))?,
        b"valuable-replacement",
        "the replaced root's file is untouched"
    );
    assert_eq!(
        std::fs::read(original.join("got/one.txt"))?,
        b"original",
        "the original root's file is untouched"
    );
    expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A sender that restarts with its root rebound to identical copies fails
/// the resume and says the roots were rebound: only the root the job was
/// offered from may serve it, and nothing more lands.
#[test]
fn a_sender_restarted_with_its_root_rebound_fails_the_resume_and_says_why() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let one = payload(512, 1607);
    let two = payload(512, 1608);
    std::fs::write(pair.host.root().join("one.txt"), &one)?;
    std::fs::write(pair.host.root().join("two.txt"), &two)?;
    let controller = &pair.controller.core;
    controller.connect_files(&pair.host.endpoint())?;
    controller.create_dir(pair.controller.root_id(), "", "got")?;
    let job = files::download(&pair, "", "got")?;
    files::poll_within(controller, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let sender_job = pair.host.core.job_list()?[0].id;
    // The job's pin on its root is saved, so after the restart only it can
    // fail the resume: the copies are identical.
    pair.host.core.shutdown();
    wait_for(
        "the receiver to learn that its sender shut down",
        Duration::from_secs(1),
        || (controller.job_poll(job).ok()?.state == JobState::Interrupted).then_some(()),
    );
    let rebound = beside(&pair.host, "rebound");
    std::fs::create_dir_all(&rebound)?;
    std::fs::write(rebound.join("one.txt"), &one)?;
    std::fs::write(rebound.join("two.txt"), &two)?;
    pair.host.restart();
    let host = &pair.host.core;
    host.set_file_roots(vec![rebound])?;
    let jobs = host.job_list()?;
    assert_eq!(jobs.len(), 1, "the sender's job survives the restart");
    assert_eq!(
        jobs[0].state,
        JobState::Interrupted,
        "the sender's job is interrupted after the restart"
    );
    let resumed = host.job_resume(sender_job)?;
    assert_eq!(
        resumed.state,
        JobState::Failed,
        "a job rebound under fails its resume"
    );
    assert_eq!(
        resumed.error.as_deref(),
        Some(ROOTS_REBOUND),
        "the failure says the roots were rebound"
    );
    let got = pair.controller.root().join("got");
    assert_eq!(
        std::fs::read(got.join("one.txt"))?,
        one,
        "the first file stays"
    );
    assert!(
        !got.join("two.txt").exists(),
        "nothing from the rebound root lands"
    );
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state done_bytes=0 files_done=0 from=interrupted job=1 sender=true to=failed
          files: the peer was not told: its side keeps the job as it was until it next asks change=resume failure=no session job=1
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A sender's root rebound mid-upload, even to identical copies of its
/// files, interrupts the job before the next file lands: only the root it
/// offered from may serve it. Bound to that root again, the upload resumes
/// with the original bytes.
#[test]
fn a_sending_root_rebound_mid_upload_interrupts_and_resumes_once_restored() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The dialed side cannot rebind mid-session, so the controller sends;
    // the host receives one file at a time, to stop between the two.
    host.fetch_one_file_at_a_time();
    let one = payload(512, 1603);
    let two = payload(512, 1604);
    let source = pair.controller.root().join("set");
    std::fs::create_dir_all(&source)?;
    std::fs::write(source.join("one.txt"), &one)?;
    std::fs::write(source.join("two.txt"), &two)?;
    controller.connect_files(&pair.host.endpoint())?;
    let job = files::upload(&pair, "set", "arrived")?;
    let receiver = files::received(host);
    files::poll_within(host, receiver, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    let arrived = pair.host.root().join("arrived/set");
    assert_eq!(
        std::fs::read(arrived.join("one.txt"))?,
        one,
        "the first file lands"
    );

    // Identical copies would pass a content check: only the job's pin to
    // the root it offered from can stop the second file.
    let rebound = beside(&pair.controller, "rebound");
    std::fs::create_dir_all(rebound.join("set"))?;
    std::fs::write(rebound.join("set/one.txt"), &one)?;
    std::fs::write(rebound.join("set/two.txt"), &two)?;
    controller.set_file_roots(vec![rebound])?;
    files::poll_within(host, receiver, 10, "the job to be interrupted", |s| {
        s.state == JobState::Interrupted
    });
    assert!(
        !arrived.join("two.txt").exists(),
        "bytes from a rebound root never land"
    );
    assert_eq!(
        controller.job_snapshot(job)?.state,
        JobState::Serving,
        "the sender mirrors its receiver only when polled"
    );

    controller.set_file_roots(vec![pair.controller.root()])?;
    host.job_resume(receiver)?;
    let (sent, got) = drive_to_end(controller, job, host, receiver, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "bound to its root again, the upload completes on both sides"
    );
    assert_eq!(
        std::fs::read(arrived.join("two.txt"))?,
        two,
        "the second file lands with the original bytes"
    );
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
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A serve that opened its source before the sender's root is rebound
/// finishes with the bytes it opened, not the rebound root's decoy of the
/// same size: only opens after the rebind are refused.
#[test]
fn a_source_opened_before_a_rebind_serves_its_own_bytes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The dialed side cannot rebind mid-session, so the controller sends;
    // the host receives one file at a time, to park the second one's serve.
    host.fetch_one_file_at_a_time();
    let one = payload(512, 1601);
    let two = payload(512, 1602);
    std::fs::write(pair.controller.root().join("one.txt"), &one)?;
    std::fs::write(pair.controller.root().join("two.txt"), &two)?;
    controller.connect_files(&pair.host.endpoint())?;
    let job = controller.job_submit_upload(
        &pair.host.endpoint(),
        pair.controller.root_id(),
        "",
        pair.host.root_id(),
        "arrived",
    )?;
    let receiver = files::only_job(host)?;
    files::poll_within(host, receiver, 20, "the first file to land", |s| {
        s.files_done == 1
    });

    // The decoy second file has the same size and other bytes.
    let rebound = beside(&pair.controller, "rebound");
    std::fs::create_dir_all(&rebound)?;
    std::fs::write(rebound.join("one.txt"), &one)?;
    std::fs::write(rebound.join("two.txt"), payload(512, 1609))?;
    let probe = ServeProbe::arm(controller, job);
    let polled = std::thread::scope(|scope| {
        let polling = scope.spawn(|| host.job_poll(receiver));
        probe.wait_until_parked();
        let rebinding = controller.set_file_roots(vec![rebound]);
        probe.release();
        rebinding?;
        polling.join().expect("the receiver's poll")
    })?;
    drop(probe);
    assert_eq!(
        polled.files_done, 2,
        "the open serve finishes the second file across the rebind"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("arrived/two.txt"))?,
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
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}
