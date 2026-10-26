//! What a sender serves and refuses: sources changed, replaced or linked after
//! their offer, shared content, and the partials a receiver keeps or releases.

use expect_test::expect;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vnilink::JobState;
use vnilink_testkit::files::{drive_to_end, payload, received};
use vnilink_testkit::{traced, Engine, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(60);

/// The controller sends `from` in its root to `to` in the host's.
fn upload(pair: &Pair, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    pair.controller.core.job_submit_upload(
        &pair.host.endpoint(),
        pair.controller.root_id(),
        from,
        pair.host.root_id(),
        to,
    )
}

/// Every file under `folder` whose bytes are `bytes`.
fn files_with_bytes(folder: &Path, bytes: &[u8]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if std::fs::read(&path).is_ok_and(|content| content == bytes) {
                found.push(path);
            }
        }
    }
    found
}

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
    std::fs::write(pair.controller.root().join("a.txt"), payload(512, 41))?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "a.txt", "dest")?;
    received(&pair.host.core);
    let root = pair.controller.root_id();
    assert!(
        pair.controller
            .core
            .rename_entry(root, "", "a.txt", None, "b.txt")
            .is_err(),
        "a source in an active transfer cannot be renamed"
    );
    pair.controller.core.job_cancel(job)?;
    pair.controller
        .core
        .rename_entry(root, "", "a.txt", None, "b.txt")?;
    assert!(
        pair.controller.root().join("b.txt").exists(),
        "a cancelled job's source can be renamed again"
    );
    trace.wait_for_records("job state", 1);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A source's folder swapped for a link to outside the root after the
/// offer, with the same name and size, is refused when it is opened: no
/// byte from outside the root reaches the receiver.
#[cfg(unix)]
#[test]
fn serving_refuses_a_parent_swapped_for_a_link() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let docs = pair.controller.root().join("docs");
    std::fs::create_dir_all(&docs)?;
    std::fs::write(docs.join("public.txt"), b"public")?;
    let outside = pair
        .controller
        .root()
        .parent()
        .ok_or("a root inside the engine's folder")?
        .join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::write(outside.join("public.txt"), b"secret")?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "docs/public.txt", "got")?;
    std::fs::remove_dir_all(&docs)?;
    std::os::unix::fs::symlink(&outside, &docs)?;
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
        files_with_bytes(&pair.host.root(), b"secret").is_empty(),
        "bytes outside the root reached the receiver"
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

/// A file renamed over a source after its offer, with the same size and
/// other bytes, fails the check made as it is served instead of reaching
/// the receiver.
#[test]
fn serving_refuses_a_source_replaced_after_its_offer() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.root().join("source.bin");
    std::fs::write(&source, payload(64 * 1024, 1701))?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "source.bin", "arrived")?;
    let decoy = payload(64 * 1024, 1702);
    let staged = pair.controller.root().join("staged.bin");
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
        files_with_bytes(&pair.host.root(), &decoy).is_empty(),
        "replacement bytes reached the receiver"
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

/// Two jobs serving the same bytes from different files: cancelling one
/// leaves the other able to fetch them.
#[test]
fn jobs_sharing_content_keep_serving_it_when_one_is_cancelled() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let bytes = payload(1024, 303);
    std::fs::write(pair.controller.root().join("a.txt"), &bytes)?;
    std::fs::write(pair.controller.root().join("b.txt"), &bytes)?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let first = upload(&pair, "a.txt", "got-a")?;
    let second = upload(&pair, "b.txt", "got-b")?;
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
        std::fs::read(pair.host.root().join("got-b/b.txt"))?,
        bytes,
        "the shared bytes land for the job still running"
    );
    trace.wait_for_records("job state", 4);
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
    "#]].assert_eq(&trace.story(&["files"]));
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
    std::fs::write(pair.controller.root().join("payload.bin"), &bytes)?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "payload.bin", "")?;
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
        std::fs::read(pair.host.root().join("payload.bin"))?,
        bytes,
        "forgetting a job keeps what it landed"
    );

    let second = payload(64 * 1024, 708);
    std::fs::write(pair.controller.root().join("partial.bin"), &second)?;
    let job = upload(&pair, "partial.bin", "")?;
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
        !pair.host.root().join("partial.bin").exists(),
        "a forgotten partial never lands"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("payload.bin"))?,
        bytes,
        "releasing a partial keeps what landed before"
    );
    trace.wait_for_records("job state", 3);
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
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}
