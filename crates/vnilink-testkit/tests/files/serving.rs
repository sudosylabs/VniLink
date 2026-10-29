//! What a sender serves and refuses: sources changed, replaced or linked after
//! their offer, shared content, and the partials a receiver keeps or releases.

use expect_test::expect;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::shared::ServeProbe;
use vnilink::{Core, JobSnapshot, JobState};
use vnilink_testkit::files::{
    download, drive_to_end, files_with_bytes, job_on, link_folder, only_job, payload, received,
    upload,
};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(60);

/// Where `engine` stages what it receives for `job`.
fn staging(engine: &Engine, job: u64) -> PathBuf {
    engine.transfers().join(format!("staging/job-{job}"))
}

/// Renaming a source while its job is active is refused, so its offer
/// keeps naming it; once the job is cancelled, the rename goes through.
#[test]
fn a_source_in_transfer_cannot_be_renamed_until_its_job_ends() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::write(pair.controller.folder().join("a.txt"), payload(512, 41))?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "a.txt", "dest")?;
    received(&pair.host.core);
    let here = pair.controller.location();
    assert!(
        pair.controller
            .core
            .rename_entry(here.root, &here.path(""), "a.txt", None, "b.txt")
            .is_err(),
        "a source in an active transfer cannot be renamed"
    );
    pair.controller.core.job_cancel(job)?;
    pair.controller
        .core
        .rename_entry(here.root, &here.path(""), "a.txt", None, "b.txt")?;
    assert!(
        pair.controller.folder().join("b.txt").exists(),
        "a cancelled job's source can be renamed again"
    );
    trace.wait_for_records("job state", 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A source's folder swapped after the offer for a link to another folder
/// holding a file of the same name and size is refused when it is opened:
/// the file there is not the one offered, and none of its bytes reach the
/// receiver.
#[test]
fn serving_refuses_a_parent_swapped_for_a_link() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let docs = pair.controller.folder().join("docs");
    std::fs::create_dir_all(&docs)?;
    std::fs::write(docs.join("public.txt"), b"public")?;
    let outside = pair
        .controller
        .folder()
        .parent()
        .ok_or("the engine's folder inside its scratch folder")?
        .join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("public.txt"), b"secret")?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "docs/public.txt", "got")?;
    std::fs::remove_dir_all(&docs)?;
    link_folder(&outside, &docs)?;
    let receiver = received(&pair.host.core);
    let (_, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_ne!(
        got.state,
        JobState::Complete,
        "a source behind a swapped link is not delivered: {got:?}"
    );
    assert!(
        files_with_bytes(&pair.host.folder(), b"secret").is_empty(),
        "the other folder's bytes reached the receiver"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=failed
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=failed
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A file renamed over a source after its offer, with the same size and
/// other bytes, fails the check made as it is served instead of reaching
/// the receiver.
#[test]
fn serving_refuses_a_source_replaced_after_its_offer() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.folder().join("source.bin");
    std::fs::write(&source, payload(64 * 1024, 1701))?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "source.bin", "arrived")?;
    let decoy = payload(64 * 1024, 1702);
    let staged = pair.controller.folder().join("staged.bin");
    std::fs::write(&staged, &decoy)?;
    std::fs::rename(&staged, &source)?;
    let receiver = received(&pair.host.core);
    let (_, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_ne!(
        got.state,
        JobState::Complete,
        "a replaced source is not delivered: {got:?}"
    );
    assert!(
        files_with_bytes(&pair.host.folder(), &decoy).is_empty(),
        "replacement bytes reached the receiver"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=failed
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=failed
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// Two jobs serving the same bytes from different files: cancelling one
/// leaves the other able to fetch them.
#[test]
fn jobs_sharing_content_keep_serving_it_when_one_is_cancelled() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let bytes = payload(1024, 303);
    std::fs::write(pair.controller.folder().join("a.txt"), &bytes)?;
    std::fs::write(pair.controller.folder().join("b.txt"), &bytes)?;
    let files = pair.open_files();
    let first = upload(&pair, &files, "a.txt", "got-a")?;
    let second = upload(&pair, &files, "b.txt", "got-b")?;
    // A receiver takes its sender's id.
    pair.controller.core.job_cancel(first)?;
    pair.host.core.job_poll(first)?;
    let receiver = pair
        .host
        .core
        .job_list()?
        .into_iter()
        .find(|job| job.id != first)
        .ok_or("the second job's receiver")?
        .id;
    let (sent, got) = drive_to_end(
        &pair.controller.core,
        second,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the other receiver completes: {got:?}"
    );
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the other sender completes: {sent:?}"
    );
    assert_eq!(
        std::fs::read(pair.host.folder().join("got-b/b.txt"))?,
        bytes,
        "the shared bytes land for the job still running"
    );
    trace.wait_for_records("job state", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=preparing job=2 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
          files: file served ended=sent from=0 sent=1024 size=1024
          files: job state done_bytes=1024 files_done=1 from=serving job=2 sender=true to=complete
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: job state done_bytes=1024 files_done=1 from=transferring job=2 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A finished file leaves no partial behind, and a cancelled job keeps
/// its partial for a resume until it is forgotten. Neither touches what
/// landed.
#[test]
fn receivers_release_partials_once_finished_or_forgotten() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let bytes = payload(64 * 1024, 707);
    std::fs::write(pair.controller.folder().join("payload.bin"), &bytes)?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "payload.bin", "")?;
    let receiver = received(&pair.host.core);
    let (_, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver completes: {got:?}"
    );
    assert!(
        !staging(&pair.host, receiver).exists(),
        "a finished file leaves no partial behind"
    );
    pair.host.core.job_forget(receiver)?;
    assert_eq!(
        std::fs::read(pair.host.folder().join("payload.bin"))?,
        bytes,
        "forgetting a job keeps what it landed"
    );

    let second = payload(64 * 1024, 708);
    std::fs::write(pair.controller.folder().join("partial.bin"), &second)?;
    let job = upload(&pair, &files, "partial.bin", "")?;
    let receiver = pair
        .host
        .core
        .job_list()?
        .into_iter()
        .find(|snapshot| snapshot.id == job)
        .ok_or("the second job's receiver")?
        .id;
    let partial = staging(&pair.host, receiver);
    std::fs::create_dir_all(partial.join("files"))?;
    std::fs::write(partial.join("files/partial.bin"), &second[..1024])?;
    pair.host.core.job_cancel(receiver)?;
    assert!(
        partial.join("files/partial.bin").exists(),
        "a cancelled job keeps its partial for a resume"
    );
    pair.host.core.job_forget(receiver)?;
    assert!(!partial.exists(), "forgetting a job releases its partial");
    assert!(
        !pair.host.folder().join("partial.bin").exists(),
        "a forgotten partial never lands"
    );
    assert_eq!(
        std::fs::read(pair.host.folder().join("payload.bin"))?,
        bytes,
        "releasing a partial keeps what landed before"
    );
    trace.wait_for_records("job state", 3);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=65536 size=65536
          files: job state done_bytes=65536 files_done=1 from=serving job=1 sender=true to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=2 sender=true to=cancelled
        host:
          files: file received outcome=received resumed_at=0 size=65536 took=65536
          files: job state done_bytes=65536 files_done=1 from=transferring job=1 sender=false to=complete
          files: job state done_bytes=0 files_done=0 from=transferring job=2 sender=false to=cancelled
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// What under `transfers` holds `bytes` while `sender`'s serve of `job` is
/// parked with its source open, and how `drive` ended the job.
fn copies_while_serving(
    sender: &Core,
    job: u64,
    transfers: &Path,
    bytes: &[u8],
    drive: impl FnOnce() -> (JobSnapshot, JobSnapshot) + Send,
) -> (Vec<PathBuf>, (JobSnapshot, JobSnapshot)) {
    let probe = ServeProbe::arm(sender, job);
    std::thread::scope(|scope| {
        let driving = scope.spawn(drive);
        let parked = probe.parked();
        let copies = files_with_bytes(transfers, bytes);
        probe.release();
        assert!(parked, "the sender's serve parked with its source open");
        (copies, driving.join().expect("the job's driver"))
    })
}

/// A sender reads its source where it is: while a serve holds it open, the
/// sender's transfer folder holds no copy of its bytes, uploading or
/// downloading. Each receiver lands the other computer's bytes once.
#[test]
fn senders_keep_no_copy_of_their_sources() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let up = payload(2 * 1024 * 1024, 101);
    let down = payload(2 * 1024 * 1024, 202);
    std::fs::write(pair.controller.folder().join("up.bin"), &up)?;
    std::fs::write(pair.host.folder().join("down.bin"), &down)?;
    let files = pair.open_files();

    // The controller uploads: it sends, the host receives.
    let uploading = upload(&pair, &files, "up.bin", "got-up")?;
    let upload_receiver = only_job(host)?;
    let (copies, (sent, got)) = copies_while_serving(
        controller,
        uploading,
        &pair.controller.transfers(),
        &up,
        || drive_to_end(controller, uploading, host, upload_receiver, TRANSFER),
    );
    assert!(
        copies.is_empty(),
        "the uploading sender copied its source: {copies:?}"
    );
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the upload completes on both sides"
    );

    // The controller downloads: the host sends, the controller receives.
    controller.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got-down",
    )?;
    let downloading = download(&pair, &files, "down.bin", "got-down")?;
    let download_sender = job_on(host, true)?;
    let (copies, (sent, got)) =
        copies_while_serving(host, download_sender, &pair.host.transfers(), &down, || {
            drive_to_end(host, download_sender, controller, downloading, TRANSFER)
        });
    assert!(
        copies.is_empty(),
        "the downloading sender copied its source: {copies:?}"
    );
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the download completes on both sides"
    );

    assert_eq!(
        files_with_bytes(&pair.host.folder(), &up).len(),
        1,
        "the host landed the uploaded bytes exactly once"
    );
    assert_eq!(
        files_with_bytes(&pair.controller.folder(), &down).len(),
        1,
        "the controller landed the downloaded bytes exactly once"
    );
    trace.wait_for_records("job state", 4);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=2097152 size=2097152
          files: job state done_bytes=2097152 files_done=1 from=serving job=1 sender=true to=complete
          files: file received outcome=received resumed_at=0 size=2097152 took=2097152
          files: job state done_bytes=2097152 files_done=1 from=transferring job=2 sender=false to=complete
        host:
          files: file received outcome=received resumed_at=0 size=2097152 took=2097152
          files: job state done_bytes=2097152 files_done=1 from=transferring job=1 sender=false to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: file served ended=sent from=0 sent=2097152 size=2097152
          files: job state done_bytes=2097152 files_done=1 from=serving job=2 sender=true to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// While a serve holds its verified source, Windows refuses to rename the
/// file, rename its folder or open it for writing, so the offered path
/// keeps naming the verified bytes for the whole request; once the request
/// ends, the source moves again.
#[cfg(windows)]
#[test]
fn windows_serve_holds_its_source_against_replacement() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let folder = pair.controller.folder().join("folder");
    std::fs::create_dir_all(&folder)?;
    let original = payload(64 * 1024, 1711);
    let source = folder.join("held.bin");
    std::fs::write(&source, &original)?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "folder/held.bin", "arrived")?;
    let receiver = only_job(host)?;

    let probe = ServeProbe::arm(controller, job);
    let (sent, got) = std::thread::scope(|scope| {
        let driving = scope.spawn(|| drive_to_end(controller, job, host, receiver, TRANSFER));
        probe.wait_until_parked();
        let renamed = std::fs::rename(&source, folder.join("moved.bin")).is_ok();
        let folder_renamed =
            std::fs::rename(&folder, pair.controller.folder().join("moved")).is_ok();
        let written = std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .is_ok();
        probe.release();
        assert!(!renamed, "the held source was renamed");
        assert!(!folder_renamed, "the held source's folder was renamed");
        assert!(!written, "the held source was opened for writing");
        driving.join().expect("the job's driver")
    });
    drop(probe);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the held source's job completes on both sides"
    );
    assert_eq!(
        files_with_bytes(&pair.host.folder(), &original).len(),
        1,
        "the verified bytes land exactly once"
    );
    // The handle closes as the serve ends, and a scanner may hold a
    // just-closed file for a moment.
    wait_for(
        "the released source to move again",
        Duration::from_secs(5),
        || std::fs::rename(&source, folder.join("moved.bin")).ok(),
    );
    Ok(())
}

/// A sender that does not answer in time costs the receiver one retry, not
/// the job: the next try, after its pause, finishes the file without anyone
/// resuming it.
#[test]
fn a_late_sender_is_retried_without_a_resume() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let bytes = payload(64 * 1024, 1801);
    std::fs::write(pair.controller.folder().join("late.bin"), &bytes)?;
    let files = pair.open_files();
    let job = upload(&pair, &files, "late.bin", "arrived")?;
    let receiver = only_job(host)?;

    // The first serve is held past the receiver's answer deadline.
    let probe = ServeProbe::arm(controller, job);
    let first = std::thread::scope(|scope| {
        let polling = scope.spawn(|| host.job_poll(receiver));
        probe.wait_until_parked();
        let first = polling.join().expect("the receiver's poll");
        probe.release();
        first
    })?;
    drop(probe);
    assert_eq!(
        first.state,
        JobState::Transferring,
        "a late sender leaves the job transferring, to retry"
    );
    assert_eq!(
        first.error.as_deref(),
        Some("the other computer could not be reached"),
        "the job says why the file did not arrive"
    );

    let got = wait_for(
        "the retry to finish the file",
        Duration::from_secs(20),
        || {
            let snapshot = host.job_poll(receiver).expect("a poll");
            assert_ne!(
                snapshot.state,
                JobState::Interrupted,
                "a late sender is retried, not left to a resume"
            );
            snapshot.state.is_terminal().then_some(snapshot)
        },
    );
    assert_eq!(got.state, JobState::Complete, "the retry completes the job");
    assert_eq!(got.error, None, "a completed retry clears the error");
    assert_eq!(
        files_with_bytes(&pair.host.folder(), &bytes).len(),
        1,
        "the file lands exactly once"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=65536 size=65536
        host:
          files: file received outcome=received resumed_at=0 size=65536 took=65536
          files: job state done_bytes=65536 files_done=1 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
