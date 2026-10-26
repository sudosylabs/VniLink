//! Moving trees between the computers: exact uploads and downloads, conflicts,
//! cancelling, resuming and restarting, as the receiver and the sender see them.

use expect_test::expect;
use std::path::Path;
use std::time::Duration;

use vnilink::{ConflictChoice, Core, JobSnapshot, JobState};
use vnilink_testkit::files::{drive_to_end, received, tree, write_files, Entry};
use vnilink_testkit::{traced, wait_for, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(60);

/// A tree with nesting, an empty folder, a name outside ASCII and a file of
/// several megabytes.
fn write_tree(folder: &Path) -> std::io::Result<()> {
    write_files(
        folder,
        &[
            ("top.txt", 1024),
            ("docs/read me-ß.txt", 4096),
            ("docs/2026/big.bin", 9 * 1024 * 1024),
            ("pack/sub/a.txt", 512),
        ],
    );
    std::fs::create_dir_all(folder.join("empty"))
}

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

/// The controller fetches `from` in the host's root to `to` in its own.
fn download(pair: &Pair, from: &str, to: &str) -> Result<u64, vnilink::Error> {
    pair.controller.core.job_submit_download(
        &pair.host.endpoint(),
        pair.host.root_id(),
        from,
        pair.controller.root_id(),
        to,
    )
}

/// The one job `core` holds, as the sender of a download submitted by its
/// peer: the host registers it while the submission is answered.
fn the_only_job(core: &Core) -> Result<u64, vnilink::Error> {
    let jobs = core.job_list()?;
    assert_eq!(jobs.len(), 1, "the peer holds exactly this job: {jobs:?}");
    Ok(jobs[0].id)
}

/// Polls `job` until it waits for a decision, which it must within
/// `polls` polls: each poll advances a job by one step.
fn asking_within(core: &Core, job: u64, polls: usize) -> Result<JobSnapshot, vnilink::Error> {
    for _ in 0..polls {
        let snapshot = core.job_poll(job)?;
        if snapshot.state == JobState::WaitingDecision {
            return Ok(snapshot);
        }
    }
    panic!("the job did not ask about a conflict within {polls} polls");
}

#[test]
fn an_upload_lands_exactly_and_keeps_its_source() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.root().join("tree");
    write_tree(&source)?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "tree", "arrived")?;
    let receiver = received(&pair.host.core);
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    assert_eq!(
        got.done_bytes, got.total_bytes,
        "the receiver holds every byte"
    );
    assert_eq!(
        sent.done_bytes, sent.total_bytes,
        "the sender shows what the receiver holds, not only that it finished"
    );
    assert!(
        got.total_bytes > 9 * 1024 * 1024,
        "the job counts the whole tree: {got:?}"
    );
    assert!(
        got.label.starts_with("tree/docs/2026/big.bin") || got.label.starts_with("tree/docs/read"),
        "a queue row shows the first path: {}",
        got.label
    );
    assert!(
        got.label.contains("more"),
        "a queue row counts the other files: {}",
        got.label
    );
    assert!(
        !format!("{got:?}").contains("arrived"),
        "the receiver's job keeps no internal destination path: {got:?}"
    );
    assert_eq!(
        tree(&pair.host.root().join("arrived/tree")),
        tree(&source),
        "the tree lands exactly"
    );
    assert!(
        source.join("docs/2026/big.bin").exists(),
        "sending leaves every source file in place"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=4096 size=4096
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=9437184 size=9437184
          files: job state done_bytes=9442816 files_done=4 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=4096 took=4096
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=9437184 took=9437184
          files: job state done_bytes=9442816 files_done=4 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.concurrent_story(&["files"]));
    Ok(())
}

#[test]
fn a_download_lands_exactly_as_the_host_sender_mirrors_it() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.host.root().join("tree");
    write_tree(&source)?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller
        .core
        .create_dir(pair.controller.root_id(), "", "got")?;
    let job = download(&pair, "tree", "got")?;
    let host_job = the_only_job(&pair.host.core)?;
    let (sent, got) = drive_to_end(
        &pair.host.core,
        host_job,
        &pair.controller.core,
        job,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver completes: {got:?}"
    );
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the host's sender mirrors completion: {sent:?}"
    );
    assert_eq!(
        tree(&pair.controller.root().join("got/tree")),
        tree(&source),
        "the tree lands exactly"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=4096 took=4096
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=9437184 took=9437184
          files: job state done_bytes=9442816 files_done=4 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=4096 size=4096
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=9437184 size=9437184
          files: job state done_bytes=9442816 files_done=4 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&trace.concurrent_story(&["files"]));
    Ok(())
}

/// Each name already at the destination waits for its own choice, and
/// files that were not sent stay as they were.
#[test]
fn conflicts_follow_each_choice_and_leave_other_files_alone() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.root().join("set");
    write_files(&source, &[("a.txt", 100), ("b.txt", 100), ("c.txt", 100)]);
    let destination = pair.host.root().join("dest/set");
    std::fs::create_dir_all(&destination)?;
    std::fs::write(destination.join("a.txt"), b"old-a")?;
    std::fs::write(destination.join("b.txt"), b"old-b")?;
    std::fs::write(destination.join("c.txt"), b"old-c")?;
    std::fs::write(destination.join("unrelated.txt"), b"unrelated")?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "set", "dest")?;
    let receiver = received(&pair.host.core);

    // Each answer covers its own file, so every path is asked about: the
    // first at the first poll, each next within ten.
    for (name, choice, polls) in [
        ("set/a.txt", ConflictChoice::Replace, 1),
        ("set/b.txt", ConflictChoice::Skip, 10),
        ("set/c.txt", ConflictChoice::KeepBoth, 10),
    ] {
        let asked = asking_within(&pair.host.core, receiver, polls)?;
        assert_eq!(
            asked.waiting_file.as_deref(),
            Some(name),
            "conflicts are asked one file at a time, in order: {asked:?}"
        );
        pair.host.core.job_resolve(receiver, choice, false)?;
    }
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );

    assert_eq!(
        std::fs::read(destination.join("a.txt"))?,
        std::fs::read(source.join("a.txt"))?,
        "Replace lands the new file"
    );
    assert_eq!(
        std::fs::read(destination.join("b.txt"))?,
        b"old-b",
        "Skip keeps the original"
    );
    assert_eq!(
        std::fs::read(destination.join("c.txt"))?,
        b"old-c",
        "Keep both keeps the original under its name"
    );
    assert_eq!(
        std::fs::read(destination.join("unrelated.txt"))?,
        b"unrelated",
        "a file the job does not send is untouched"
    );
    let kept = std::fs::read_dir(&destination)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("c (2)"))
        })
        .ok_or("Keep both lands the new file under a new name")?;
    assert_eq!(
        std::fs::read(kept)?,
        std::fs::read(source.join("c.txt"))?,
        "the renamed file holds the new contents"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=100 size=100
          files: file served ended=sent from=0 sent=100 size=100
          files: file served ended=sent from=0 sent=100 size=100
          files: job state done_bytes=300 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state done_bytes=300 files_done=2 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A receiver that quits with files left finds its job interrupted when it
/// starts again, saying why, and resuming it fetches only what is missing.
#[test]
fn a_receiver_restarted_mid_download_resumes_the_files_left() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let source = pair.host.root().join("set");
    write_files(
        &source,
        &[("one.txt", 1024), ("two.txt", 1024), ("three.txt", 1024)],
    );
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller
        .core
        .create_dir(pair.controller.root_id(), "", "got")?;
    let job = download(&pair, "set", "got")?;

    // Two files land and the third waits: the receiver quits here.
    pair.controller.core.job_poll(job)?;
    pair.controller.core.job_poll(job)?;
    let mid = pair.controller.core.job_snapshot(job)?;
    assert_eq!(mid.files_done, 2, "each poll lands one file: {mid:?}");
    pair.controller.restart();

    let jobs = pair.controller.core.job_list()?;
    assert_eq!(
        jobs.len(),
        1,
        "the job's record survives the restart: {jobs:?}"
    );
    assert_eq!(
        jobs[0].state,
        JobState::Interrupted,
        "a job VniLink quit during is interrupted: {jobs:?}"
    );
    // A clean shutdown ends the session first; a crash leaves the record
    // to say VniLink closed. Either way the job says why it stopped.
    assert!(
        matches!(
            jobs[0].error.as_deref(),
            Some(
                "the connection to the other computer ended" | "VniLink closed during the transfer"
            )
        ),
        "an interrupted job says why it stopped: {jobs:?}"
    );
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller.core.job_resume(job)?;
    let host_job = the_only_job(&pair.host.core)?;
    let (sent, got) = drive_to_end(
        &pair.host.core,
        host_job,
        &pair.controller.core,
        job,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the resumed job completes: {got:?}"
    );
    assert_eq!(
        sent.state,
        JobState::Complete,
        "its sender completes: {sent:?}"
    );
    assert_eq!(got.files_done, 3, "every file counts as done: {got:?}");
    assert_eq!(
        tree(&pair.controller.root().join("got/set")),
        tree(&source),
        "the tree lands exactly"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=1024 size=1024 took=0
          files: job state done_bytes=3072 files_done=3 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=1024 sent=0 size=1024
          files: job state done_bytes=3072 files_done=3 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Cancelling while a large file is in flight keeps the file that already
/// landed, publishes nothing of the one in flight, and resuming finishes
/// the same tree.
#[test]
fn a_cancelled_download_keeps_what_landed_and_shows_nothing_partial() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.host.root().join("set");
    write_files(&source, &[("one.txt", 2048), ("two.txt", 12 * 1024 * 1024)]);
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller
        .core
        .create_dir(pair.controller.root_id(), "", "got")?;
    let job = download(&pair, "set", "got")?;
    let controller = &pair.controller.core;
    controller.job_poll(job)?;
    // The 12 MiB fetch takes far longer than starting a thread and
    // delivering the cancel, so the cancel lands on a live fetch or a
    // pending poll.
    std::thread::scope(|scope| -> TestResult {
        let poll = scope.spawn(|| controller.job_poll(job));
        controller.job_cancel(job)?;
        let stopped = poll.join().map_err(|_| "the poll panicked")??;
        assert_eq!(
            stopped.state,
            JobState::Cancelled,
            "the poll in flight ends cancelled: {stopped:?}"
        );
        Ok(())
    })?;
    let after = controller.job_poll(job)?;
    assert_eq!(
        after.state,
        JobState::Cancelled,
        "a cancelled job stays cancelled: {after:?}"
    );
    let landed = pair.controller.root().join("got/set");
    assert_eq!(
        std::fs::read(landed.join("one.txt"))?,
        std::fs::read(source.join("one.txt"))?,
        "the finished file stays"
    );
    assert!(
        !landed.join("two.txt").exists(),
        "the file in flight never appears, not even partly"
    );
    for entry in std::fs::read_dir(&landed)? {
        let name = entry?.file_name();
        assert!(
            !name.to_string_lossy().ends_with(".part"),
            "staging leaked into the destination: {name:?}"
        );
    }
    // The host's stopped serve records its end after the job was cancelled.
    trace.wait_for_records("file served", 2);

    controller.job_resume(job)?;
    let host_job = the_only_job(&pair.host.core)?;
    let (_, got) = drive_to_end(&pair.host.core, host_job, controller, job, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the resumed job completes: {got:?}"
    );
    assert_eq!(tree(&landed), tree(&source), "the tree lands exactly");
    trace.wait_for_records("job state", 2);
    let fetches = trace.named("file received");
    let field = |index: usize, key: &str| fetches[index][key].as_u64().unwrap_or(u64::MAX);
    let size = 12 * 1024 * 1024;
    assert!(
        field(1, "took") < size && field(1, "resumed_at") == 0,
        "the cancelled fetch kept part of the file: {fetches:?}"
    );
    assert!(
        field(2, "resumed_at") <= field(1, "took")
            && field(2, "resumed_at") + field(2, "took") == size,
        "the resumed fetch continues from what was kept: {fetches:?}"
    );
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=2048 took=2048
          files: job state done_bytes=2048 files_done=1 from=transferring job=1 sender=false to=cancelled
          files: file received outcome=cancelled resumed_at=… size=12582912 took=…
          files: job state from=cancelled job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=… size=12582912 took=…
          files: job state done_bytes=12584960 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=2048 size=2048
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
          files: file served ended=the receiver left from=… sent=… size=12582912
          files: job state from=cancelled job=1 sender=true to=serving
          files: file served ended=sent from=… sent=… size=12582912
          files: job state done_bytes=12584960 files_done=2 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&counts_hidden(&trace.story(&["files"]), size));
    Ok(())
}

/// `story` with the byte counts of the file of `size` bytes hidden: where a
/// cancel lands in it differs from run to run, so a test bounds them.
fn counts_hidden(story: &str, size: u64) -> String {
    story
        .lines()
        .map(|line| {
            if !line.contains(&format!(" size={size}")) {
                return line.to_owned();
            }
            line.split(' ')
                .map(|word| match word.split_once('=') {
                    Some((key @ ("took" | "sent" | "resumed_at" | "from"), value))
                        if value.parse::<u64>().is_ok() =>
                    {
                        format!("{key}=…")
                    }
                    _ => word.to_owned(),
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .map(|line| line + "\n")
        .collect()
}

/// A destination that refuses the replacement fails the job and keeps the
/// original; once writable, resuming lands the new file.
#[test]
fn a_refused_publication_keeps_the_original_and_resumes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.host.root().join("set");
    write_files(&source, &[("new.txt", 512)]);
    let destination = pair.controller.root().join("got/set");
    std::fs::create_dir_all(&destination)?;
    std::fs::write(destination.join("new.txt"), b"original")?;

    // Windows enforces read-only on the file rather than Unix folder
    // permissions. Restored even when an assert fails.
    #[cfg(windows)]
    let writable = {
        struct RestorePermissions(std::path::PathBuf, std::fs::Permissions);
        impl Drop for RestorePermissions {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, self.1.clone());
            }
        }
        let path = destination.join("new.txt");
        let original = std::fs::metadata(&path)?.permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly)?;
        RestorePermissions(path, original)
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o555))?;
        // Running as root writes through permissions: nothing to prove.
        if std::fs::write(destination.join("probe.txt"), b"p").is_ok() {
            let _ = std::fs::remove_file(destination.join("probe.txt"));
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
            return Ok(());
        }
    }

    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = download(&pair, "set", "got")?;
    let host_job = the_only_job(&pair.host.core)?;
    // Replace, so publication is attempted.
    asking_within(&pair.controller.core, job, 10)?;
    pair.controller
        .core
        .job_resolve(job, ConflictChoice::Replace, false)?;
    let (_, got) = drive_to_end(
        &pair.host.core,
        host_job,
        &pair.controller.core,
        job,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Failed,
        "a refused publication fails the job: {got:?}"
    );
    assert_eq!(
        std::fs::read(destination.join("new.txt"))?,
        b"original",
        "a failed publication keeps the original"
    );

    #[cfg(windows)]
    drop(writable);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
    }
    pair.controller.core.job_resume(job)?;
    let (_, got) = drive_to_end(
        &pair.host.core,
        host_job,
        &pair.controller.core,
        job,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the resumed job completes: {got:?}"
    );
    assert_eq!(
        std::fs::read(destination.join("new.txt"))?,
        std::fs::read(source.join("new.txt"))?,
        "the resumed job replaces the original"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=failed
          files: job state from=failed job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=512 files_done=1 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=512 files_done=0 from=serving job=1 sender=true to=failed
          files: job state from=failed job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=512 files_done=1 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A transfer is refused before anything is scanned without a session, a
/// source that exists, or a root the computer knows.
#[test]
fn submitting_needs_a_session_an_existing_source_and_a_known_root() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    assert!(
        matches!(upload(&pair, "", "x"), Err(vnilink::Error::NoSession)),
        "no transfer without a session"
    );
    pair.controller.core.connect(&pair.host.endpoint())?;
    assert!(
        matches!(
            upload(&pair, "missing", "x"),
            Err(vnilink::Error::InvalidSetting(_))
        ),
        "no transfer from a source that does not exist"
    );
    assert!(
        matches!(
            pair.controller.core.job_submit_download(
                &pair.host.endpoint(),
                pair.host.root_id(),
                "",
                u64::MAX,
                "x",
            ),
            Err(vnilink::Error::InvalidSetting(_))
        ),
        "no transfer into a root this computer does not know"
    );
    expect![[r#""#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Nested empty folders land although they carry no file.
#[test]
fn an_empty_folder_tree_lands_whole() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::create_dir_all(pair.controller.root().join("empty/nested"))?;
    write_files(&pair.controller.root().join("keep"), &[("a.txt", 1)]);
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "empty", "arrived")?;
    let receiver = received(&pair.host.core);
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    assert_eq!(
        tree(&pair.host.root().join("arrived")),
        [
            ("empty".to_owned(), Entry::Folder),
            ("empty/nested".to_owned(), Entry::Folder),
        ]
        .into(),
        "the empty hierarchy lands, and nothing beside it"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=complete
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A sender that cancels before its receiver fetched anything ends the job
/// on both sides, and nothing lands.
#[test]
fn a_job_its_sender_cancels_before_delivery_lands_nothing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.root(), &[("a.txt", 1024)]);
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "a.txt", "got")?;
    let receiver = received(&pair.host.core);
    pair.controller.core.job_cancel(job)?;
    // The cancel reached the receiver inside `job_cancel`; one poll settles it.
    let after = pair.host.core.job_poll(receiver)?;
    assert_eq!(
        after.state,
        JobState::Cancelled,
        "the receiver follows its sender's cancel: {after:?}"
    );
    assert!(
        !pair.host.root().join("got/a.txt").exists(),
        "a cancelled file never lands"
    );
    let sent = pair.controller.core.job_snapshot(job)?;
    assert_eq!(
        sent.state,
        JobState::Cancelled,
        "the sender is cancelled: {sent:?}"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// The application's background drivers alone finish a job: no window
/// polls it.
#[test]
fn driving_every_job_finishes_one_without_window_polls() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.root(), &[("a.txt", 1024)]);
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "a.txt", "got")?;
    let receiver = received(&pair.host.core);
    let (sent, got) = wait_for("both sides to end", TRANSFER, || {
        pair.controller.core.drive_jobs().ok()?;
        pair.host.core.drive_jobs().ok()?;
        let sent = pair.controller.core.job_snapshot(job).ok()?;
        let got = pair.host.core.job_snapshot(receiver).ok()?;
        (sent.state.is_terminal() && got.state.is_terminal()).then_some((sent, got))
    });
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver completes: {got:?}"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("got/a.txt"))?,
        std::fs::read(pair.controller.root().join("a.txt"))?,
        "the file lands exactly"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: job state done_bytes=1024 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: job state done_bytes=1024 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// An upload's conflict reaches its sender, which answers it without a
/// window on the receiving computer.
#[test]
fn an_upload_conflict_reaches_its_sender_who_answers_it() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.root(), &[("a.txt", 100)]);
    let destination = pair.host.root().join("dest");
    std::fs::create_dir_all(&destination)?;
    std::fs::write(destination.join("a.txt"), b"old")?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "a.txt", "dest")?;
    let receiver = received(&pair.host.core);
    let asked = pair.host.core.job_poll(receiver)?;
    assert_eq!(
        asked.state,
        JobState::WaitingDecision,
        "the receiver meets the conflict first: {asked:?}"
    );
    let mirrored = pair.controller.core.job_poll(job)?;
    assert_eq!(
        mirrored.state,
        JobState::WaitingDecision,
        "the sender learns of the conflict from the receiver's status: {mirrored:?}"
    );
    assert_eq!(
        mirrored.waiting_file.as_deref(),
        Some("a.txt"),
        "the sender names the file in conflict"
    );
    pair.controller
        .core
        .job_resolve(job, ConflictChoice::Replace, false)?;
    let (sent, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the receiver applies the sender's choice: {got:?}"
    );
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    assert_eq!(
        std::fs::read(destination.join("a.txt"))?,
        std::fs::read(pair.controller.root().join("a.txt"))?,
        "Replace lands the new file"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=waiting-decision
          files: job state from=waiting-decision job=1 sender=true to=transferring
          files: file served ended=sent from=0 sent=100 size=100
          files: job state done_bytes=100 files_done=1 from=transferring job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: job state done_bytes=100 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A job its sender cancelled is cancelled on both sides; resuming the
/// receiver resumes the sender, and the files land.
#[test]
fn a_job_its_sender_cancelled_resumes_from_its_receiver() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(
        &pair.controller.root().join("set"),
        &[("a.txt", 512), ("b.txt", 512)],
    );
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "set", "got")?;
    let receiver = received(&pair.host.core);
    pair.controller.core.job_cancel(job)?;
    let sent = pair.controller.core.job_snapshot(job)?;
    assert_eq!(
        sent.state,
        JobState::Cancelled,
        "the sender cancels: {sent:?}"
    );
    let got = pair.host.core.job_poll(receiver)?;
    assert_eq!(
        got.state,
        JobState::Cancelled,
        "the receiver follows: {got:?}"
    );
    // The receiver's resume tells the sender, which offers again; resuming
    // the sender too is harmless, and keeps a raced notice from mattering.
    pair.host.core.job_resume(receiver)?;
    let _ = pair.controller.core.job_resume(job);
    let (sent, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        got.state,
        JobState::Complete,
        "the resumed receiver completes: {got:?}"
    );
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the resumed sender completes: {sent:?}"
    );
    assert_eq!(
        tree(&pair.host.root().join("got/set")),
        tree(&pair.controller.root().join("set")),
        "the tree lands exactly"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
          files: job state from=cancelled job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=cancelled
          files: job state from=cancelled job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A sender restarted after one file of a nested source resumes the files
/// under that source, not under the root.
#[test]
fn a_restarted_sender_resumes_a_nested_source() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    write_files(
        &pair.controller.root().join("docs/pack"),
        &[("sub/a.txt", 512), ("b.txt", 512)],
    );
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "docs/pack", "got")?;
    let receiver = received(&pair.host.core);
    // One file lands; then the sender quits, and the receiver keeps its job.
    pair.host.core.job_poll(receiver)?;
    pair.controller.restart();
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller.core.job_resume(job)?;
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the restarted sender completes: {sent:?}"
    );
    assert_eq!(
        tree(&pair.host.root().join("got/pack")),
        tree(&pair.controller.root().join("docs/pack")),
        "the source's own folder lands, with its nested files"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=512 sent=0 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=512 size=512 took=0
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Staging and destination on different filesystems, where this computer
/// has them (`/dev/shm` on Linux): publication copies across and lands the
/// file. Elsewhere it renames, and lands the file the same.
#[test]
fn publication_across_filesystems_lands_the_file() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let shm = Path::new("/dev/shm").join(format!("vnilink-testkit-staging-{}", std::process::id()));
    let shm_usable = cfg!(target_os = "linux") && std::fs::create_dir_all(&shm).is_ok();
    struct Remove(std::path::PathBuf);
    impl Drop for Remove {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _staging = shm_usable.then(|| Remove(shm.clone()));
    if shm_usable {
        // The host receives, so its staging crosses to its root.
        pair.host.core.set_transfer_directory(&shm)?;
    }
    write_files(&pair.controller.root(), &[("a.txt", 2048)]);
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "a.txt", "got")?;
    let receiver = received(&pair.host.core);
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender completes: {sent:?}"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("got/a.txt"))?,
        std::fs::read(pair.controller.root().join("a.txt"))?,
        "the file lands exactly"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=2048 size=2048
          files: job state done_bytes=2048 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=2048 took=2048
          files: job state done_bytes=2048 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A receiver answering Keep both while its sender already mirrors the
/// conflict lets the sender go on, and both files stay.
#[test]
fn a_receivers_keep_both_unblocks_a_sender_mirroring_the_conflict() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::write(pair.controller.root().join("file.txt"), b"incoming")?;
    std::fs::create_dir_all(pair.host.root().join("dest"))?;
    std::fs::write(pair.host.root().join("dest/file.txt"), b"original")?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = upload(&pair, "file.txt", "dest")?;
    let receiver = received(&pair.host.core);
    let asked = pair.host.core.job_poll(receiver)?;
    assert_eq!(
        asked.state,
        JobState::WaitingDecision,
        "the receiver asks: {asked:?}"
    );
    let mirrored = pair.controller.core.job_poll(job)?;
    assert_eq!(
        mirrored.state,
        JobState::WaitingDecision,
        "the sender mirrors the question: {mirrored:?}"
    );
    pair.host
        .core
        .job_resolve(receiver, ConflictChoice::KeepBoth, false)?;
    let (sent, got) = drive_to_end(
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
    assert_eq!(
        sent.state,
        JobState::Complete,
        "the sender leaves the mirrored question and completes: {sent:?}"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("dest/file.txt"))?,
        b"original",
        "Keep both keeps the original under its name"
    );
    assert_eq!(
        std::fs::read(pair.host.root().join("dest/file (2).txt"))?,
        b"incoming",
        "Keep both lands the new file under the next free name"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=serving job=1 sender=true to=waiting-decision
          files: file served ended=sent from=0 sent=8 size=8
          files: job state from=waiting-decision job=1 sender=true to=serving
          files: job state done_bytes=8 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=8 took=8
          files: job state done_bytes=8 files_done=1 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Two small files in the host's `set` folder, for tests that stop a
/// download between them; the controller takes one at a time, fetching none ahead.
fn two_files_one_at_a_time(pair: &Pair) -> std::io::Result<()> {
    pair.controller.core.fetch_one_file_at_a_time();
    write_files(
        &pair.host.root().join("set"),
        &[("one.txt", 512), ("two.txt", 512)],
    );
    std::fs::create_dir_all(pair.controller.root().join("got"))
}

/// A host that stops sharing files after the first of two files lands
/// interrupts the download there, landing nothing more; sharing again, the
/// download resumes to the exact tree.
#[test]
fn a_download_the_host_stops_sharing_resumes_once_shared_again() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    two_files_one_at_a_time(&pair)?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.connect(&pair.host.endpoint())?;
    let job = download(&pair, "set", "got")?;
    controller.job_poll(job)?;
    host.set_service_sharing(vnilink::ServiceKind::Files, false)?;
    let stopped = controller.job_poll(job)?;
    assert_eq!(
        stopped.state,
        JobState::Interrupted,
        "the next poll stops instead of fetching: {stopped:?}"
    );
    assert_eq!(stopped.files_done, 1, "the first file had landed");
    assert_eq!(
        stopped.error.as_deref(),
        Some("the other computer stopped sending"),
        "the job says why it stopped"
    );
    assert!(
        !pair.controller.root().join("got/set/two.txt").exists(),
        "nothing fetched after the withdrawal lands"
    );

    let (sharing, _) = vnilink_testkit::FILES;
    host.set_desktop_sharing(sharing)?;
    // The withdrawal dropped the host's offers: its sender offers again
    // before the receiver fetches again.
    let sender = the_only_job(host)?;
    host.job_resume(sender)?;
    controller.job_resume(job)?;
    let (_, got) = drive_to_end(host, sender, controller, job, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the download resumes: {got:?}"
    );
    assert_eq!(
        tree(&pair.controller.root().join("got/set")),
        tree(&pair.host.root().join("set")),
        "the resumed download lands the exact tree"
    );
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// A host that forgets the controller after the first of two files lands
/// ends the download there: the connection ended, and the second file
/// never appears.
#[test]
fn a_download_whose_host_forgets_the_controller_lands_nothing_more() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    two_files_one_at_a_time(&pair)?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    controller.connect(&pair.host.endpoint())?;
    let job = download(&pair, "set", "got")?;
    controller.job_poll(job)?;
    host.revoke(&pair.controller.endpoint())?;
    let stopped = controller.job_poll(job)?;
    assert_eq!(
        stopped.state,
        JobState::Interrupted,
        "the download stops with its connection: {stopped:?}"
    );
    assert_eq!(
        stopped.error.as_deref(),
        Some("the connection to the other computer ended"),
        "the job says why it stopped"
    );
    assert!(
        controller.job_snapshot(job)?.files_done <= 1,
        "no file after the first counts as landed"
    );
    assert!(
        !pair.controller.root().join("got/set/two.txt").exists(),
        "the second file is refused at publication"
    );
    expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
    "#]]
    .assert_eq(&trace.story(&["files"]));
    Ok(())
}

/// Shares `engine`'s whole disk rather than its one folder, and names
/// `inside` (a path in its folder) from the disk's root: the root's id and
/// the path within it.
fn whole_disk(
    engine: &vnilink_testkit::Engine,
    inside: &str,
) -> Result<(u64, String), vnilink::Error> {
    engine.core.enable_filesystem_access()?;
    let folder = engine.root().join(inside);
    let (id, base) = engine
        .core
        .file_roots()?
        .into_iter()
        .find(|(_, path)| folder.starts_with(path))
        .expect("a disk root holds the engine's folder");
    let path = folder
        .strip_prefix(&base)
        .expect("inside the root")
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok((id, path))
}

/// As [`a_restarted_sender_resumes_a_nested_source`], with each computer
/// sharing its whole disk: the restarted sender finds its nested source
/// again from the disk's root, by its full path.
#[test]
fn a_restarted_sender_sharing_its_whole_disk_resumes_a_nested_source() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    write_files(
        &pair.controller.root().join("docs/pack"),
        &[("sub/a.txt", 512), ("b.txt", 512)],
    );
    let (source_root, source) = whole_disk(&pair.controller, "docs/pack")?;
    std::fs::create_dir_all(pair.host.root().join("got"))?;
    let (destination_root, destination) = whole_disk(&pair.host, "got")?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    let job = pair.controller.core.job_submit_upload(
        &pair.host.endpoint(),
        source_root,
        &source,
        destination_root,
        &destination,
    )?;
    let receiver = received(&pair.host.core);
    // One file lands; then the sender quits, and the receiver keeps its job.
    pair.host.core.job_poll(receiver)?;
    pair.controller.restart();
    pair.controller.core.enable_filesystem_access()?;
    pair.controller.core.connect(&pair.host.endpoint())?;
    pair.controller.core.job_resume(job)?;
    let (sent, got) = drive_to_end(
        &pair.controller.core,
        job,
        &pair.host.core,
        receiver,
        TRANSFER,
    );
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the restarted sender and its receiver complete"
    );
    assert_eq!(
        tree(&pair.host.root().join("got/pack")),
        tree(&pair.controller.root().join("docs/pack")),
        "the source's own folder lands, with its nested files"
    );
    trace.wait_for_records("job state", 2);
    expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=512 sent=0 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=512 size=512 took=0
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]].assert_eq(&trace.story(&["files"]));
    Ok(())
}
