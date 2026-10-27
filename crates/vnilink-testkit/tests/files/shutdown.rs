//! A peer that shuts down mid-transfer: the other side learns it at once,
//! not when a dial to the stopped engine times out.
//!
//! Jobs here are driven by hand (`job_poll`), so the shutdown falls
//! between two exact files of a transfer.

use expect_test::expect;
use std::time::{Duration, Instant};

use vnilink::{Core, JobSnapshot, JobState, SessionPhase};
use vnilink_testkit::files::{self, payload, received};
use vnilink_testkit::{traced, wait_for, Pair, TestResult};

/// Well inside the 30 s a dial to a stopped engine waits.
const NOTICED_WITHIN: Duration = Duration::from_secs(1);

/// Polls `receiver`'s job until it stops, and how long that took.
fn poll_until_stopped(receiver: &Core, job: u64) -> (JobSnapshot, Duration) {
    let started = Instant::now();
    loop {
        let snapshot = receiver.job_poll(job).expect("a poll");
        if snapshot.state.is_terminal() || snapshot.state == JobState::Interrupted {
            return (snapshot, started.elapsed());
        }
        assert!(
            started.elapsed() < NOTICED_WITHIN,
            "the job still runs after its sender shut down: {snapshot:?}"
        );
    }
}

/// A host that shuts down mid-download ends the controller's session, and
/// the controller's job is interrupted at once: it does not dial the
/// stopped host again.
#[test]
fn a_host_that_shuts_down_mid_download_interrupts_the_job_at_once() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::write(pair.host.folder().join("one.txt"), payload(512, 1801))?;
    std::fs::write(pair.host.folder().join("two.txt"), payload(512, 1802))?;
    let controller = &pair.controller.core;
    let session = controller.connect_files(&pair.host.endpoint())?;
    controller.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    let job = files::download(&pair, "", "got")?;
    files::poll_within(controller, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    pair.host.core.shutdown();
    let (snapshot, took) = poll_until_stopped(controller, job);
    assert_eq!(
        snapshot.state,
        JobState::Interrupted,
        "a job whose sender shut down is interrupted, to resume later"
    );
    assert!(
        took < NOTICED_WITHIN,
        "the controller learns at once that its host shut down, not after {took:?}"
    );
    let ended = wait_for("the session to end", NOTICED_WITHIN, || {
        let phase = controller.controller_session(session.id).ok()?.phase;
        matches!(phase, SessionPhase::Closed | SessionPhase::Failed(_)).then_some(phase)
    });
    assert_eq!(
        ended,
        SessionPhase::Closed,
        "a host that shut down ends the session; it is not dialed again"
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
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A controller that shuts down mid-upload leaves the host's receiving job
/// to retry, and the poll that learns it returns at once and says why: no
/// session ends on the host's side, so only the files connection's close
/// tells it.
#[test]
fn a_controller_that_shuts_down_mid_upload_leaves_the_receiver_retrying_and_says_why() -> TestResult
{
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The host receives one file at a time, to stop between the two.
    host.fetch_one_file_at_a_time();
    let source = pair.controller.folder().join("set");
    std::fs::create_dir_all(&source)?;
    std::fs::write(source.join("one.txt"), payload(512, 1803))?;
    std::fs::write(source.join("two.txt"), payload(512, 1804))?;
    controller.connect_files(&pair.host.endpoint())?;
    files::upload(&pair, "set", "arrived")?;
    let job = received(host);
    files::poll_within(host, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    controller.shutdown();
    let started = Instant::now();
    let snapshot = host.job_poll(job)?;
    let took = started.elapsed();
    assert_eq!(
        (snapshot.state, snapshot.error.as_deref()),
        (JobState::Transferring, Some("the other computer shut down")),
        "a receiver whose sender shut down keeps its job to retry, and says why"
    );
    assert!(
        took < NOTICED_WITHIN,
        "the host learns at once that its controller shut down, not after {took:?}"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}
