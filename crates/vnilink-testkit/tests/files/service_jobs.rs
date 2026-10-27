//! Transfers that ride the Files service rather than a files-only session: both
//! directions, closing, admission and adoption, and restarts of either computer.
//!
//! Jobs here are driven by hand (`drive_to_end`, `job_poll`): each scenario
//! closes, restarts or adopts at an exact point of a transfer, which the
//! application's background drivers would race past.

use expect_test::expect;
use std::time::Duration;

use vnilink::{Error, JobSnapshot, JobState, ServiceInfo, ServiceKind, SessionPhase};
use vnilink_testkit::files::{self, drive_to_end, job_on, payload};
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, Traced, DEFAULT_WAIT, FILES};

const TRANSFER: Duration = Duration::from_secs(30);

fn both(sent: &JobSnapshot, got: &JobSnapshot) -> (JobState, JobState) {
    (sent.state, got.state)
}

/// The last file `engine` recorded receiving whole.
fn last_received(trace: &Traced, engine: &str) -> serde_json::Value {
    trace
        .named("file received")
        .into_iter()
        .filter(|event| event["engine"] == engine && event["outcome"] == "received")
        .next_back()
        .expect("a file received whole")
}

/// An upload and a download on a services connection's Files service both
/// land exactly and leave their sources, and closing a Desktop service on
/// the same connection does not disturb them.
#[test]
fn files_service_jobs_complete_both_ways_and_keep_their_sources() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    // The host offers a Desktop only with a screen to show.
    vnilink_testkit::desktop::install_screen(&host.core)?;
    let upload = payload(8192, 113);
    let download = payload(16384, 117);
    std::fs::write(controller.folder().join("up.bin"), &upload)?;
    std::fs::write(host.folder().join("down.bin"), &download)?;
    let connection = pair.connect_services();
    let (files, _pipe) = pair.controller.open_files(&connection)?;
    let up = controller.core.files_service_upload(
        files.id,
        files.generation,
        controller.location().root,
        &controller.location().path(""),
        &["up.bin".to_owned()],
        host.location().root,
        &host.location().path("uploads"),
    )?;
    let desktop = pair.attach(&connection, ServiceKind::Desktop);
    controller
        .core
        .open_desktop_service(desktop.id, desktop.generation)?
        .close();
    let receiver = job_on(&host.core, false)?;
    let (sent, got) = drive_to_end(&controller.core, up, &host.core, receiver, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "an upload on the Files service completes on both sides"
    );
    let down = controller.core.files_service_download(
        files.id,
        files.generation,
        host.location().root,
        &host.location().path(""),
        &["down.bin".to_owned()],
        controller.location().root,
        &controller.location().path("downloads"),
    )?;
    let sender = job_on(&host.core, true)?;
    let (got, sent) = drive_to_end(&controller.core, down, &host.core, sender, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "a download on the Files service completes on both sides"
    );
    assert_eq!(
        std::fs::read(host.folder().join("uploads/up.bin"))?,
        upload,
        "the upload lands exactly"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("downloads/down.bin"))?,
        download,
        "the download lands exactly"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("up.bin"))?,
        upload,
        "an upload keeps its source"
    );
    assert_eq!(
        std::fs::read(host.folder().join("down.bin"))?,
        download,
        "a download keeps its source"
    );
    assert_eq!(
        controller
            .core
            .files_service_jobs(files.id, files.generation)?
            .len(),
        2,
        "the Files service lists both of its jobs"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=8192 size=8192
          files: job state done_bytes=8192 files_done=1 from=serving job=1 sender=true to=complete
          files: file received outcome=received resumed_at=0 size=16384 took=16384
          files: job state done_bytes=16384 files_done=1 from=transferring job=2 sender=false to=complete
        host:
          files: file received outcome=received resumed_at=0 size=8192 took=8192
          files: job state done_bytes=8192 files_done=1 from=transferring job=1 sender=false to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: file served ended=sent from=0 sent=16384 size=16384
          files: job state done_bytes=16384 files_done=1 from=serving job=2 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A services connection with no desktop, which carries no picture
/// quality, moves files both ways through its Files service.
#[test]
fn a_services_connection_without_a_desktop_moves_files_both_ways() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    host.core.set_service_sharing(ServiceKind::Desktop, false)?;
    host.core
        .set_video_codecs(vnilink::VideoCodecCaps::default())?;
    controller
        .core
        .set_video_codecs(vnilink::VideoCodecCaps::default())?;
    let upload = payload(8192, 113);
    let download = payload(16384, 117);
    std::fs::write(controller.folder().join("up.bin"), &upload)?;
    std::fs::write(host.folder().join("down.bin"), &download)?;
    let connection = pair.connect_services();
    let (files, _pipe) = pair.controller.open_files(&connection)?;
    let up = controller.core.files_service_upload(
        files.id,
        files.generation,
        controller.location().root,
        &controller.location().path(""),
        &["up.bin".to_owned()],
        host.location().root,
        &host.location().path("uploads"),
    )?;
    let receiver = job_on(&host.core, false)?;
    let (sent, got) = drive_to_end(&controller.core, up, &host.core, receiver, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "an upload without a desktop completes on both sides"
    );
    let down = controller.core.files_service_download(
        files.id,
        files.generation,
        host.location().root,
        &host.location().path(""),
        &["down.bin".to_owned()],
        controller.location().root,
        &controller.location().path("downloads"),
    )?;
    let sender = job_on(&host.core, true)?;
    let (got, sent) = drive_to_end(&controller.core, down, &host.core, sender, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "a download without a desktop completes on both sides"
    );
    assert_eq!(
        std::fs::read(host.folder().join("uploads/up.bin"))?,
        upload,
        "the upload lands exactly"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("downloads/down.bin"))?,
        download,
        "the download lands exactly"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("up.bin"))?,
        upload,
        "an upload keeps its source"
    );
    assert_eq!(
        std::fs::read(host.folder().join("down.bin"))?,
        download,
        "a download keeps its source"
    );
    assert_eq!(
        controller
            .core
            .files_service_jobs(files.id, files.generation)?
            .len(),
        2,
        "the Files service lists both of its jobs"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=8192 size=8192
          files: job state done_bytes=8192 files_done=1 from=serving job=1 sender=true to=complete
          files: file received outcome=received resumed_at=0 size=16384 took=16384
          files: job state done_bytes=16384 files_done=1 from=transferring job=2 sender=false to=complete
        host:
          files: file received outcome=received resumed_at=0 size=8192 took=8192
          files: job state done_bytes=8192 files_done=1 from=transferring job=1 sender=false to=complete
          files: job state from=preparing job=2 sender=true to=serving
          files: file served ended=sent from=0 sent=16384 size=16384
          files: job state done_bytes=16384 files_done=1 from=serving job=2 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Closing a Files service interrupts its own job on both sides, which it
/// can no longer resume, and leaves a session job of the same file (on a
/// files-only session beside the link), the Desktop owner and the
/// connection running.
#[test]
fn closing_a_files_service_interrupts_its_job_and_keeps_a_session_job_of_the_same_file(
) -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let bytes = payload(32768, 123);
    std::fs::write(controller.folder().join("same.bin"), &bytes)?;
    let connection = pair.connect_services();
    let (files, pipe) = pair.controller.open_files(&connection)?;
    let desktop = pair.attach(&connection, ServiceKind::Desktop);
    // A job no service owns rides a files-only session.
    controller.core.connect_files(&host.endpoint())?;
    let scoped = controller.core.files_service_upload(
        files.id,
        files.generation,
        controller.location().root,
        &controller.location().path(""),
        &["same.bin".to_owned()],
        host.location().root,
        &host.location().path("cancelled"),
    )?;
    let scoped_receiver = job_on(&host.core, false)?;
    let session_job = controller.core.job_submit_upload(
        &host.endpoint(),
        controller.location().root,
        &controller.location().path("same.bin"),
        host.location().root,
        &host.location().path("kept"),
    )?;
    let session_receiver = host
        .core
        .job_list()?
        .into_iter()
        .find(|job| job.id != scoped_receiver)
        .expect("the session job's receiver")
        .id;
    controller
        .core
        .close_controller_service(files.id, files.generation)?;
    assert_eq!(
        controller.core.job_snapshot(scoped)?.state,
        JobState::Interrupted,
        "closing the service interrupts its job at once"
    );
    wait_for("the host to interrupt the job", DEFAULT_WAIT, || {
        (host.core.job_snapshot(scoped_receiver).ok()?.state == JobState::Interrupted).then_some(())
    });
    assert!(
        pipe.is_cancelled(),
        "the closed service's pipe is cancelled"
    );
    assert!(
        matches!(controller.core.job_resume(scoped), Err(Error::NoSession)),
        "a closed service's job cannot resume without one"
    );
    assert!(
        matches!(host.core.job_resume(scoped_receiver), Err(Error::NoSession)),
        "nor can its receiver"
    );
    let (sent, got) = drive_to_end(
        &controller.core,
        session_job,
        &host.core,
        session_receiver,
        TRANSFER,
    );
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the session job of the same file completes"
    );
    assert_eq!(
        std::fs::read(host.folder().join("kept/same.bin"))?,
        bytes,
        "the session job lands exactly"
    );
    assert!(
        !host.folder().join("cancelled").exists(),
        "the closed service's job left nothing"
    );
    assert!(
        !controller
            .core
            .service_lease(desktop.id, desktop.generation)?
            .is_cancelled(),
        "the desktop service on the connection stays"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "the connection stays"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=preparing job=2 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: file served ended=sent from=0 sent=32768 size=32768
          files: job state done_bytes=32768 files_done=1 from=serving job=2 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: file received outcome=received resumed_at=0 size=32768 took=32768
          files: job state done_bytes=32768 files_done=1 from=transferring job=2 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// The host closing its Files service interrupts a download on both sides
/// and refuses new work on that service, while the connection stays.
#[test]
fn a_host_closing_the_files_service_interrupts_a_download_and_refuses_new_work() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    std::fs::write(host.folder().join("a.bin"), payload(8192, 137))?;
    let connection = pair.connect_services();
    let (files, pipe) = pair.controller.open_files(&connection)?;
    let job = controller.core.files_service_download(
        files.id,
        files.generation,
        host.location().root,
        &host.location().path(""),
        &["a.bin".to_owned()],
        controller.location().root,
        &controller.location().path("cancelled"),
    )?;
    let sender = job_on(&host.core, true)?;
    let hosted = host.core.hosted_services()?[0].clone();
    host.core
        .close_hosted_service(hosted.id, hosted.generation)?;
    assert_eq!(
        host.core.job_snapshot(sender)?.state,
        JobState::Interrupted,
        "closing the hosted service interrupts its job at once"
    );
    wait_for(
        "the controller's pipe to be cancelled",
        DEFAULT_WAIT,
        || pipe.is_cancelled().then_some(()),
    );
    wait_for("the controller to interrupt the job", DEFAULT_WAIT, || {
        (controller.core.job_snapshot(job).ok()?.state == JobState::Interrupted).then_some(())
    });
    assert!(
        matches!(
            controller.core.files_service_download(
                files.id,
                files.generation,
                host.location().root,
                &host.location().path(""),
                &["a.bin".to_owned()],
                controller.location().root,
                &controller.location().path("later"),
            ),
            Err(Error::NoSession)
        ),
        "a closed service takes no new work"
    );
    assert!(
        !controller.folder().join("cancelled").exists(),
        "the interrupted download left nothing"
    );
    assert_eq!(
        controller.core.controller_session(connection.id)?.phase,
        SessionPhase::Active,
        "the connection stays"
    );
    expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A Files service the host has not admitted takes no job on either side;
/// an admitted one does, and its job, interrupted when its pipe closes,
/// stays interrupted after the controller restarts and needs a service to
/// resume.
#[test]
fn files_service_jobs_need_an_admitted_service_and_one_to_resume_after_a_restart() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    std::fs::write(pair.controller.folder().join("a.bin"), payload(4096, 139))?;
    let (theirs, mine) = (pair.host.location(), pair.controller.location());
    let controller = &pair.controller.core;
    let session = controller.connect_files(&pair.host.endpoint())?;
    let unopened = pair.attach(&session, ServiceKind::Files);
    assert!(
        controller
            .files_service_upload(
                unopened.id,
                unopened.generation,
                mine.root,
                &mine.path(""),
                &["a.bin".to_owned()],
                theirs.root,
                &theirs.path("refused"),
            )
            .is_err(),
        "a service the host has not admitted takes no job"
    );
    assert!(
        pair.host.core.job_list()?.is_empty(),
        "the host keeps no job it refused"
    );
    assert!(
        controller.job_list()?.is_empty(),
        "the controller keeps no refused job"
    );
    // An ambiguous refusal ends only that service, so that a lost reply
    // cannot leave an admitted job without one; the retry opens another.
    if !service_ended(&controller.controller_service(unopened.id)?.phase) {
        controller.close_controller_service(unopened.id, unopened.generation)?;
    }
    let (files, pipe) = pair.controller.open_files(&session)?;
    let job = controller.files_service_upload(
        files.id,
        files.generation,
        mine.root,
        &mine.path(""),
        &["a.bin".to_owned()],
        theirs.root,
        &theirs.path("cancelled"),
    )?;
    pipe.close();
    wait_for("the job to be interrupted", DEFAULT_WAIT, || {
        (controller.job_snapshot(job).ok()?.state == JobState::Interrupted).then_some(())
    });
    drop(pipe);
    pair.controller.restart();
    let restarted = &pair.controller.core;
    assert_eq!(
        restarted.job_snapshot(job)?.state,
        JobState::Interrupted,
        "a service's job stays interrupted across a restart"
    );
    assert!(
        matches!(restarted.job_resume(job), Err(Error::NoSession)),
        "a service's job resumes only through a service"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=preparing job=2 sender=true to=serving
          files: job state from=serving job=2 sender=true to=interrupted
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// One controller with Files services on two computers: each service sees
/// and acts on its own jobs only, and closing one interrupts its job while
/// the other's transfer of the same file completes.
#[test]
fn closing_one_computers_files_service_keeps_another_computers_transfer() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (first, controller) = (&pair.host, &pair.controller);
    let second = Engine::start("second");
    let (sharing, _) = FILES;
    second.pair_with(controller)?;
    second.core.set_desktop_sharing(sharing)?;
    let bytes = payload(65536, 149);
    std::fs::write(controller.folder().join("shared.bin"), &bytes)?;
    let one = pair.connect_services();
    let (one, _one_pipe) = pair.controller.open_files(&one)?;
    let two = controller.connect_services(&second)?;
    let (two, _two_pipe) = pair.controller.open_files(&two)?;
    let cancelled = controller.core.files_service_upload(
        one.id,
        one.generation,
        controller.location().root,
        &controller.location().path(""),
        &["shared.bin".to_owned()],
        first.location().root,
        &first.location().path("cancelled"),
    )?;
    let kept = controller.core.files_service_upload(
        two.id,
        two.generation,
        controller.location().root,
        &controller.location().path(""),
        &["shared.bin".to_owned()],
        second.location().root,
        &second.location().path("kept"),
    )?;
    // A service cannot use another service's job, though both live in the
    // same engine and send the same file.
    let before = controller.core.job_snapshot(kept)?;
    let ids = |service: &ServiceInfo| -> Result<Vec<u64>, Error> {
        Ok(controller
            .core
            .files_service_jobs(service.id, service.generation)?
            .iter()
            .map(|job| job.id)
            .collect())
    };
    assert_eq!(
        ids(&one)?,
        vec![cancelled],
        "the first service lists its job"
    );
    assert_eq!(ids(&two)?, vec![kept], "the second service lists its job");
    let core = &controller.core;
    assert!(
        core.files_service_job_snapshot(one.id, one.generation, kept)
            .is_err(),
        "a service cannot see another's job"
    );
    assert!(
        core.files_service_job_poll(one.id, one.generation, kept)
            .is_err(),
        "a service cannot poll another's job"
    );
    assert!(
        core.files_service_job_cancel(one.id, one.generation, kept)
            .is_err(),
        "a service cannot cancel another's job"
    );
    assert!(
        core.files_service_job_resume(one.id, one.generation, kept)
            .is_err(),
        "a service cannot resume another's job"
    );
    assert!(
        core.files_service_job_resolve(
            one.id,
            one.generation,
            kept,
            vnilink::ConflictChoice::Skip,
            true
        )
        .is_err(),
        "a service cannot answer another's job"
    );
    assert!(
        core.files_service_job_forget(one.id, one.generation, kept)
            .is_err(),
        "a service cannot forget another's job"
    );
    assert!(
        matches!(
            core.files_service_job_snapshot(two.id, two.generation + 1, kept),
            Err(Error::StaleSession)
        ),
        "a service of another generation is stale"
    );
    assert_eq!(
        core.job_snapshot(kept)?,
        before,
        "refused calls leave the job as it was"
    );
    let second_job = second.core.job_list()?[0].id;
    core.close_controller_service(one.id, one.generation)?;
    assert_eq!(
        core.job_snapshot(cancelled)?.state,
        JobState::Interrupted,
        "closing a service interrupts its job"
    );
    assert_eq!(
        core.files_service_job_snapshot(one.id, one.generation, cancelled)?
            .state,
        JobState::Interrupted,
        "the closed service still shows its job"
    );
    assert!(
        core.files_service_job_resume(one.id, one.generation, cancelled)
            .is_err(),
        "a closed service cannot resume its job"
    );
    // An interrupted transfer is unfinished: it is cancelled before it is
    // forgotten.
    core.files_service_job_cancel(one.id, one.generation, cancelled)?;
    core.files_service_job_forget(one.id, one.generation, cancelled)?;
    assert!(
        core.files_service_jobs(one.id, one.generation)?.is_empty(),
        "a forgotten job leaves its service"
    );
    let (sent, got) = drive_to_end(core, kept, &second.core, second_job, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the other computer's transfer completes"
    );
    assert_eq!(
        std::fs::read(second.folder().join("kept/shared.bin"))?,
        bytes,
        "the other computer's transfer lands exactly"
    );
    assert!(
        !first.folder().join("cancelled").exists(),
        "the closed service's job left nothing"
    );
    assert!(
        !core.service_lease(two.id, two.generation)?.is_cancelled(),
        "the other service stays open"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=preparing job=2 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state done_bytes=0 files_done=0 from=interrupted job=1 sender=true to=cancelled
          files: the peer was not told: its side keeps the job as it was until it next asks change=cancel failure=network job=1
          files: file served ended=sent from=0 sent=65536 size=65536
          files: job state done_bytes=65536 files_done=1 from=serving job=2 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
        second:
          files: file received outcome=received resumed_at=0 size=65536 took=65536
          files: job state done_bytes=65536 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A job whose Files service closed is adopted by a new service on the same
/// connection: the old service can no longer act on it, and the new one
/// resumes it to the end.
#[test]
fn a_new_files_service_adopts_a_closed_ones_job_and_fences_the_old_one() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let bytes = payload(1024 * 1024, 903);
    std::fs::write(controller.folder().join("adopt.bin"), &bytes)?;
    let session = controller.core.connect_files(&host.endpoint())?;
    let (old, pipe) = pair.controller.open_files(&session)?;
    let job = controller.core.files_service_upload(
        old.id,
        old.generation,
        controller.location().root,
        &controller.location().path(""),
        &["adopt.bin".to_owned()],
        host.location().root,
        &host.location().path("adopted"),
    )?;
    let receiver = job_on(&host.core, false)?;
    controller
        .core
        .close_controller_service(old.id, old.generation)?;
    drop(pipe);
    pair.host.wait_released(None);
    let (fresh, _pipe) = pair.controller.open_files(&session)?;
    let core = &controller.core;
    assert_eq!(
        core.files_service_adopt_job(fresh.id, fresh.generation, job)?
            .state,
        JobState::Interrupted,
        "an adopted job is interrupted until resumed"
    );
    assert!(
        core.files_service_job_resume(old.id, old.generation, job)
            .is_err(),
        "the old service cannot resume an adopted job"
    );
    assert!(
        core.files_service_job_forget(old.id, old.generation, job)
            .is_err(),
        "the old service cannot forget an adopted job"
    );
    assert!(
        core.files_service_jobs(old.id, old.generation)?.is_empty(),
        "the old service no longer lists the job"
    );
    assert_eq!(
        core.files_service_jobs(fresh.id, fresh.generation)?.len(),
        1,
        "the new service lists the job"
    );
    core.files_service_job_resume(fresh.id, fresh.generation, job)?;
    let (sent, got) = drive_to_end(core, job, &host.core, receiver, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the adopted job completes on both sides"
    );
    assert_eq!(
        std::fs::read(host.folder().join("adopted/adopt.bin"))?,
        bytes,
        "the adopted job lands exactly"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1048576 size=1048576
          files: job state done_bytes=1048576 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=1048576 took=1048576
          files: job state done_bytes=1048576 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A download whose connection ended is adopted on a new connection with
/// the progress it had verified, and resumes to the same files.
#[test]
fn a_download_adopted_on_a_new_connection_keeps_its_verified_progress() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let first = payload(65536, 912);
    let second = payload(131072, 913);
    std::fs::create_dir_all(host.folder().join("source"))?;
    std::fs::write(host.folder().join("source/a.bin"), &first)?;
    std::fs::write(host.folder().join("source/b.bin"), &second)?;
    let session = pair.connect_services();
    let (old, pipe) = pair.controller.open_files(&session)?;
    let core = &controller.core;
    let job = core.files_service_download(
        old.id,
        old.generation,
        host.location().root,
        &host.location().path(""),
        &["source".to_owned()],
        controller.location().root,
        &controller.location().path("received"),
    )?;
    let sender = job_on(&host.core, true)?;
    let progress = core.files_service_job_poll(old.id, old.generation, job)?;
    assert_eq!(
        progress.done_bytes,
        first.len() as u64,
        "one poll lands the first file"
    );
    assert_ne!(progress.state, JobState::Complete, "and only the first");
    assert_eq!(
        std::fs::read(controller.folder().join("received/source/a.bin"))?,
        first,
        "the first file lands exactly"
    );
    core.disconnect_session(session.id)?;
    drop(pipe);
    pair.host.wait_released(None);
    // The host releasing the service and the controller retiring the
    // connection are separate: a stopping connection still holds its peer.
    wait_for("the old connection to retire", DEFAULT_WAIT, || match core
        .controller_session(session.id)
    {
        Ok(info) => {
            matches!(info.phase, SessionPhase::Closed | SessionPhase::Failed(_)).then_some(())
        }
        Err(Error::NoSession) => Some(()),
        Err(error) => panic!("the old connection failed to retire: {error}"),
    });
    let fresh_session = pair.connect_services();
    let (fresh, _pipe) = pair.controller.open_files(&fresh_session)?;
    let adopted = core.files_service_adopt_job(fresh.id, fresh.generation, job)?;
    assert_eq!(adopted.id, job, "adoption keeps the job's id");
    assert_eq!(
        adopted.done_bytes, progress.done_bytes,
        "adoption keeps the verified progress"
    );
    assert!(
        core.files_service_job_resume(old.id, old.generation, job)
            .is_err(),
        "the old service cannot resume an adopted job"
    );
    core.files_service_job_resume(fresh.id, fresh.generation, job)?;
    let (got, sent) = drive_to_end(core, job, &host.core, sender, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the adopted download completes on both sides"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("received/source/a.bin"))?,
        first,
        "the first file is kept"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("received/source/b.bin"))?,
        second,
        "the second file lands exactly"
    );
    // How far the second file was fetched ahead when the job stopped varies;
    // its resumed fetch takes exactly the rest.
    expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: job state done_bytes=196608 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: job state done_bytes=196608 files_done=2 from=serving job=1 sender=true to=complete
    "#]]
    .assert_eq(&trace.tell(&["files"]).only(&["job state"]).to_string());
    let resumed = last_received(&trace, "controller");
    assert_eq!(
        resumed["resumed_at"].as_u64().unwrap_or(u64::MAX) + resumed["took"].as_u64().unwrap_or(0),
        131072,
        "the resumed fetch takes the rest of the file after what was verified: {resumed}"
    );
    Ok(())
}

/// After both computers restart, a new Files service lists none of the old
/// jobs until it adopts one, which keeps its verified files and resumes
/// to the end.
#[test]
fn a_download_adopted_after_both_computers_restart_keeps_its_verified_files() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let first = payload(32768, 940);
    let second = payload(65536, 941);
    std::fs::create_dir(pair.host.folder().join("pack"))?;
    std::fs::write(pair.host.folder().join("pack/a.bin"), &first)?;
    std::fs::write(pair.host.folder().join("pack/b.bin"), &second)?;
    let session = pair.connect_services();
    let (old, pipe) = pair.controller.open_files(&session)?;
    let job = pair.controller.core.files_service_download(
        old.id,
        old.generation,
        pair.host.location().root,
        &pair.host.location().path(""),
        &["pack".to_owned()],
        pair.controller.location().root,
        &pair.controller.location().path("received"),
    )?;
    let peer_job = job_on(&pair.host.core, true)?;
    assert_eq!(
        pair.controller
            .core
            .files_service_job_poll(old.id, old.generation, job)?
            .files_done,
        1,
        "one poll lands the first file"
    );
    drop(pipe);
    pair.controller.restart();
    pair.host.restart();
    pair.reintroduce();
    let session = pair.connect_services();
    let (fresh, _pipe) = pair.controller.open_files(&session)?;
    let core = &pair.controller.core;
    assert!(
        core.files_service_jobs(fresh.id, fresh.generation)?
            .is_empty(),
        "a new service lists no job it has not adopted"
    );
    let adopted = core.files_service_adopt_job(fresh.id, fresh.generation, job)?;
    assert_eq!(adopted.files_done, 1, "adoption keeps the verified file");
    assert_eq!(
        adopted.done_bytes,
        first.len() as u64,
        "adoption keeps the verified bytes"
    );
    core.files_service_job_resume(fresh.id, fresh.generation, job)?;
    let (got, sent) = drive_to_end(core, job, &pair.host.core, peer_job, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the adopted download completes on both sides"
    );
    assert_eq!(
        std::fs::read(pair.controller.folder().join("received/pack/a.bin"))?,
        first,
        "the first file is kept"
    );
    assert_eq!(
        std::fs::read(pair.controller.folder().join("received/pack/b.bin"))?,
        second,
        "the second file lands exactly"
    );
    // How far the second file was fetched ahead when the job stopped varies;
    // its resumed fetch takes exactly the rest.
    expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: job state done_bytes=98304 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: job state done_bytes=98304 files_done=2 from=serving job=1 sender=true to=complete
    "#]]
    .assert_eq(&trace.tell(&["files"]).only(&["job state"]).to_string());
    let resumed = last_received(&trace, "controller");
    assert_eq!(
        resumed["resumed_at"].as_u64().unwrap_or(u64::MAX) + resumed["took"].as_u64().unwrap_or(0),
        65536,
        "the resumed fetch takes the rest of the file after what was verified: {resumed}"
    );
    Ok(())
}

/// An adoption the controller fails to save leaves the job unadopted and
/// fenced from the old service, and adopting again once saving works
/// resumes it to the end.
#[test]
fn an_adoption_that_fails_to_save_can_be_retried() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let expected = payload(65536, 942);
    std::fs::write(controller.folder().join("source.bin"), &expected)?;
    let session = pair.connect_services();
    let (old, pipe) = pair.controller.open_files(&session)?;
    let core = &controller.core;
    let job = core.files_service_upload(
        old.id,
        old.generation,
        controller.location().root,
        &controller.location().path(""),
        &["source.bin".to_owned()],
        host.location().root,
        &host.location().path("received"),
    )?;
    let peer_job = job_on(&host.core, false)?;
    core.close_controller_service(old.id, old.generation)?;
    drop(pipe);
    pair.host.wait_released(None);
    let (fresh, _pipe) = pair.controller.open_files(&session)?;
    // A folder where the job's record goes refuses its replacement on every
    // platform, whatever the account may write.
    let record = controller.transfers().join(format!("jobs/{job}.jsonl"));
    let saved = controller.transfers().join("journal.saved");
    std::fs::rename(&record, &saved)?;
    std::fs::create_dir(&record)?;
    assert!(
        core.files_service_adopt_job(fresh.id, fresh.generation, job)
            .is_err(),
        "an adoption that cannot be saved fails"
    );
    assert!(
        core.files_service_jobs(fresh.id, fresh.generation)?
            .is_empty(),
        "a failed adoption leaves the job unadopted"
    );
    assert!(
        core.files_service_job_resume(old.id, old.generation, job)
            .is_err(),
        "the old service still cannot resume it"
    );
    std::fs::remove_dir(&record)?;
    std::fs::rename(&saved, &record)?;
    let adopted = core.files_service_adopt_job(fresh.id, fresh.generation, job)?;
    assert_eq!(
        adopted.state,
        JobState::Interrupted,
        "the retried adoption takes the job, interrupted"
    );
    core.files_service_job_resume(fresh.id, fresh.generation, job)?;
    let (sent, got) = drive_to_end(core, job, &host.core, peer_job, TRANSFER);
    assert_eq!(
        both(&sent, &got),
        (JobState::Complete, JobState::Complete),
        "the adopted upload completes on both sides"
    );
    assert_eq!(
        std::fs::read(host.folder().join("received/source.bin"))?,
        expected,
        "the adopted upload lands exactly"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          warn files: transfer records were not saved: a restart may lose their latest state failure=invalid setting reason=the job record could not be saved
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=65536 size=65536
          files: job state done_bytes=65536 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=65536 took=65536
          files: job state done_bytes=65536 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A receiver whose resource budget a file exceeds fails the job before
/// fetching anything: no staging, nothing landed, the source kept, and the
/// sender fails too.
#[test]
fn a_file_past_the_receivers_budget_fails_before_any_fetch() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host, &pair.controller);
    let bytes = payload(64 * 1024, 314);
    std::fs::create_dir_all(controller.folder().join("set"))?;
    std::fs::write(controller.folder().join("set/send.bin"), &bytes)?;
    host.core
        .set_file_resource_policy(vnilink::job::FileResourcePolicy {
            max_concurrent_receivers: 1,
            max_reserved_bytes: 3 * bytes.len() as u64 - 1,
            minimum_free_bytes: 0,
        })?;
    controller.core.connect_files(&host.endpoint())?;
    let sent = files::upload(&pair, "set", "arrived")?;
    let receiver = job_on(&host.core, false)?;
    let failed = host.core.job_poll(receiver)?;
    assert_eq!(failed.state, JobState::Failed, "the receiver fails the job");
    assert_eq!(
        failed.error.as_deref(),
        Some("file exceeds the receiver resource budget"),
        "the failure says why"
    );
    assert_eq!(failed.done_bytes, 0, "nothing was fetched");
    assert!(
        !host
            .transfers()
            .join(format!("staging/job-{receiver}"))
            .exists(),
        "nothing was staged"
    );
    assert!(
        !host.folder().join("arrived/set/send.bin").exists(),
        "nothing landed"
    );
    assert_eq!(
        std::fs::read(controller.folder().join("set/send.bin"))?,
        bytes,
        "the source is kept"
    );
    assert_eq!(
        controller.core.job_poll(sent)?.state,
        JobState::Failed,
        "the sender fails with its receiver"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=failed
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=failed
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A receiver that restarts keeps an interrupted job's staged bytes, and
/// removes staging no job owns and a record rewrite torn by the stop; the
/// sender's source is untouched.
#[test]
fn a_restart_removes_orphaned_staging_and_keeps_an_interrupted_jobs() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    std::fs::write(
        pair.controller.folder().join("source.bin"),
        b"source survives crash",
    )?;
    pair.controller.core.connect_files(&pair.host.endpoint())?;
    files::upload(&pair, "", "received")?;
    let receiver = job_on(&pair.host.core, false)?;
    pair.host.core.shutdown();
    pair.controller.core.shutdown();
    let transfers = pair.host.transfers();
    let staged = transfers.join(format!("staging/job-{receiver}"));
    std::fs::create_dir_all(&staged)?;
    std::fs::write(staged.join("verified"), b"resume data")?;
    let orphan = transfers.join("staging/job-18446744073709551615");
    std::fs::create_dir_all(&orphan)?;
    std::fs::write(orphan.join("unused"), b"orphan")?;
    let torn = transfers.join(format!("jobs/{receiver}.jsonl.tmp"));
    std::fs::write(&torn, b"incomplete rewrite")?;
    pair.host.restart();
    assert_eq!(
        pair.host.core.job_snapshot(receiver)?.state,
        JobState::Interrupted,
        "the receiver's job is interrupted after the restart"
    );
    assert_eq!(
        std::fs::read(staged.join("verified"))?,
        b"resume data",
        "an interrupted job's staging is kept"
    );
    assert!(!orphan.exists(), "staging no job owns is removed");
    assert!(!torn.exists(), "a torn record rewrite is removed");
    assert_eq!(
        std::fs::read(pair.controller.folder().join("source.bin"))?,
        b"source survives crash",
        "the source is untouched"
    );
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}
