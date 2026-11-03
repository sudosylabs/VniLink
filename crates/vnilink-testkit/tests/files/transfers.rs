// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Moving trees between the computers: exact uploads and downloads, conflicts,
//! cancelling, resuming and restarting, as the receiver and the sender see them.

use expect_test::expect;
use std::path::Path;

use vnilink::{ConflictChoice, Error, JobState};
use vnilink_testkit::files::{
    check, download, drive_to_end, only_job, poll_within, source, tree, upload, write_files, Entry,
    Job, Moved, Moving, Way, TRANSFER,
};
use vnilink_testkit::{traced, wait_for, Pair, TestResult};

/// A tree with nesting, an empty folder, a name outside ASCII and a file of
/// several megabytes, moved from `tree` to `to` by hand.
fn tree_to(way: Way, to: &str) -> Moving<'_> {
    Moving {
        way,
        files: &[
            ("top.txt", 1024),
            ("docs/read me-ß.txt", 4096),
            ("docs/2026/big.bin", 9 * 1024 * 1024),
            ("pack/sub/a.txt", 512),
        ],
        folders: &["empty"],
        from: "tree",
        to,
        drivers: false,
    }
}

#[test]
fn an_upload_lands_exactly_and_keeps_its_source() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.folder().join("tree");
    let Moved { sent, got, .. } = check(
        &trace,
        &pair,
        tree_to(Way::Upload, "arrived"),
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
          files: received files flushed job=1
          files: job state done_bytes=9442816 files_done=4 from=transferring job=1 sender=false to=complete
    "#]],
    )?;
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
    assert!(
        source.join("docs/2026/big.bin").exists(),
        "sending leaves every source file in place"
    );
    Ok(())
}

/// The host's sender mirrors its controller's receiver to completion.
#[test]
fn a_download_lands_exactly_as_the_host_sender_mirrors_it() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    pair.controller.core.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    check(
        &trace,
        &pair,
        tree_to(Way::Download, "got"),
        expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=4096 took=4096
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: file received outcome=received resumed_at=0 size=9437184 took=9437184
          files: received files flushed job=1
          files: job state done_bytes=9442816 files_done=4 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=4096 size=4096
          files: file served ended=sent from=0 sent=512 size=512
          files: file served ended=sent from=0 sent=9437184 size=9437184
          files: job state done_bytes=9442816 files_done=4 from=serving job=1 sender=true to=complete
    "#]],
    )?;
    Ok(())
}

/// Each name already at the destination waits for its own choice, and
/// files that were not sent stay as they were.
#[test]
fn conflicts_follow_each_choice_and_leave_other_files_alone() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.controller.folder().join("set");
    write_files(&source, &[("a.txt", 100), ("b.txt", 100), ("c.txt", 100)]);
    let destination = pair.host.folder().join("dest/set");
    std::fs::create_dir_all(&destination)?;
    std::fs::write(destination.join("a.txt"), b"old-a")?;
    std::fs::write(destination.join("b.txt"), b"old-b")?;
    std::fs::write(destination.join("c.txt"), b"old-c")?;
    std::fs::write(destination.join("unrelated.txt"), b"unrelated")?;
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "set", "dest")?;

    // Each answer covers its own file, so every path is asked about: the
    // first at the first poll, each next within ten.
    for (name, choice, polls) in [
        ("set/a.txt", ConflictChoice::Replace, 1),
        ("set/b.txt", ConflictChoice::Skip, 10),
        ("set/c.txt", ConflictChoice::KeepBoth, 10),
    ] {
        let asked = poll_within(
            &pair.host.core,
            job.host,
            polls,
            "the job to ask about a conflict",
            |s| s.state == JobState::WaitingDecision,
        );
        assert_eq!(
            asked.waiting_file.as_deref(),
            Some(name),
            "conflicts are asked one file at a time, in order: {asked:?}"
        );
        pair.host.core.job_resolve(job.host, choice, false)?;
    }
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
    // A file's bytes land on a stream task of their own while the job asks
    // about the next conflict, so where "file received" falls among the
    // job's states is not part of the story: the states are told without
    // the receipts, and the receipts on their own.
    vnilink_testkit::assert_story(
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
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: received files flushed job=1
          files: job state done_bytes=300 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || {
            trace
                .tell(&["files"])
                .only(&["job state", "file served", "received files flushed"])
                .to_string()
        },
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: file received outcome=received resumed_at=0 size=100 took=100
          files: file received outcome=received resumed_at=0 size=100 took=100
    "#]],
        || {
            trace
                .tell(&["files"])
                .only(&["file received"])
                .of("host")
                .to_string()
        },
    );
    Ok(())
}

/// A receiver that quits with files left finds its job interrupted when it
/// starts again, saying why, and resuming it on a new Files service
/// fetches only what is missing.
#[test]
fn a_receiver_restarted_mid_download_resumes_the_files_left() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let source = pair.host.folder().join("set");
    write_files(
        &source,
        &[("one.txt", 1024), ("two.txt", 1024), ("three.txt", 1024)],
    );
    let files = pair.open_files();
    pair.controller.core.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    let job = download(&pair, &files, "set", "got")?;

    // Two files land and the third waits: the receiver quits here.
    pair.controller.core.job_poll(job)?;
    pair.controller.core.job_poll(job)?;
    let mid = pair.controller.core.job_snapshot(job)?;
    assert_eq!(mid.files_done, 2, "each poll lands one file: {mid:?}");
    // The Files tab is still open when VniLink quits: the handle outlives
    // the engine, as a window does, and goes with it.
    pair.controller.restart();
    drop(files);

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
    // A clean shutdown ends the connection first; a crash leaves the record
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
    // A service's job resumes only through a service: the restarted
    // controller's new Files adopts it first.
    let files = pair.open_files();
    let controller = &pair.controller.core;
    controller.files_service_adopt_job(files.service.id, files.service.generation, job)?;
    controller.files_service_job_resume(files.service.id, files.service.generation, job)?;
    let host_job = only_job(&pair.host.core)?;
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
        tree(&pair.controller.folder().join("got/set")),
        tree(&source),
        "the tree lands exactly"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: received files flushed job=1
          files: job state done_bytes=3072 files_done=3 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=1024 size=1024
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=1024 sent=0 size=1024
          files: job state done_bytes=3072 files_done=3 from=serving job=1 sender=true to=complete
    "#]],
        // The third file's bytes land on a stream task of their own as the
        // engine quits, so the receiver records it as received or as
        // cancelled: only the file that resumed is told apart below.
        || {
            trace
                .tell(&["files"])
                .only(&["job state", "file served", "received files flushed"])
                .to_string()
        },
    );
    assert_eq!(
        trace
            .named_with("file received", &[("resumed_at", "1024"), ("took", "0")])
            .len(),
        1,
        "the resumed receiver fetches nothing more of the file it had"
    );
    Ok(())
}

/// A receiver that stops after its last file landed and before the volume
/// was flushed has not completed the job: it resumes, fetches nothing, flushes
/// and only then completes.
#[test]
fn a_receiver_stopped_before_the_final_flush_completes_after_flushing_on_resume() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    let source = pair.host.folder().join("set");
    write_files(&source, &[("one.txt", 1024), ("two.txt", 1024)]);
    let files = pair.open_files();
    pair.controller.core.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    let job = download(&pair, &files, "set", "got")?;

    // Both files landed, each synced as it did; the job is not complete.
    pair.controller.core.job_poll(job)?;
    pair.controller.core.job_poll(job)?;
    let mid = pair.controller.core.job_snapshot(job)?;
    assert_eq!(mid.files_done, 2, "each poll lands one file: {mid:?}");
    assert_eq!(
        mid.state,
        JobState::Transferring,
        "the job completes only after the flush: {mid:?}"
    );
    pair.controller.restart();
    drop(files);
    assert_eq!(
        pair.controller.core.job_list()?[0].state,
        JobState::Interrupted,
        "the job stopped before its flush is interrupted"
    );
    assert_eq!(
        tree(&pair.controller.folder().join("got/set")),
        tree(&source),
        "the landed files stay"
    );

    let files = pair.open_files();
    let controller = &pair.controller.core;
    controller.files_service_adopt_job(files.service.id, files.service.generation, job)?;
    controller.files_service_job_resume(files.service.id, files.service.generation, job)?;
    let host_job = only_job(&pair.host.core)?;
    let (sent, got) = drive_to_end(&pair.host.core, host_job, controller, job, TRANSFER);
    assert_eq!(
        (sent.state, got.state),
        (JobState::Complete, JobState::Complete),
        "the resumed job completes on both sides"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: received files flushed job=1
          files: job state done_bytes=2048 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: file served ended=sent from=0 sent=1024 size=1024
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: job state done_bytes=2048 files_done=2 from=serving job=1 sender=true to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// Cancelling while a large file is in flight keeps the file that already
/// landed, publishes nothing of the one in flight, and resuming finishes
/// the same tree.
#[test]
fn a_cancelled_download_keeps_what_landed_and_shows_nothing_partial() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.host.folder().join("set");
    write_files(&source, &[("one.txt", 2048), ("two.txt", 12 * 1024 * 1024)]);
    let files = pair.open_files();
    pair.controller.core.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    let job = download(&pair, &files, "set", "got")?;
    let controller = &pair.controller.core;
    controller.job_poll(job)?;
    // The 12 MiB file is fetched ahead, from the first poll on, so the
    // cancel lands on a live fetch, a pending poll or a file already whole.
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
    let landed = pair.controller.folder().join("got/set");
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
    // The host's stopped serve records its end.
    trace.wait_for_records("file served", 2);

    controller.job_resume(job)?;
    let host_job = only_job(&pair.host.core)?;
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
    // The cancel may land after the whole file arrived, where a fast link
    // takes all of it in one segment; the fetch keeps what it took either way.
    assert!(
        field(1, "took") <= size && field(1, "resumed_at") == 0,
        "the cancelled fetch kept what arrived of the file: {fetches:?}"
    );
    assert!(
        field(2, "resumed_at") <= field(1, "took")
            && field(2, "resumed_at") + field(2, "took") == size,
        "the resumed fetch continues from what was kept: {fetches:?}"
    );
    let story = |engine: &str, messages: &[&str]| {
        trace
            .tell(&["files"])
            .of(engine)
            .only(messages)
            .masking(&["took", "sent", "resumed_at", "from"], move |e| {
                e["size"] == size
            })
            .to_string()
    };
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=2048 took=2048
          files: job state done_bytes=2048 files_done=1 from=transferring job=1 sender=false to=cancelled
          files: file received outcome=cancelled resumed_at=… size=12582912 took=…
          files: job state from=cancelled job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=… size=12582912 took=…
          files: job state done_bytes=12584960 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || story("controller", &["file received", "job state"]),
    );
    // The host learns of the cancel and of its receiver leaving on
    // separate streams, in either order: each is told on its own.
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=cancelled
          files: job state from=cancelled job=1 sender=true to=serving
          files: job state done_bytes=12584960 files_done=2 from=serving job=1 sender=true to=complete
    "#]],
        || story("host", &["job state"]),
    );
    // The host may have written all of the large file before the cancel
    // reached it, so that serve ends either way; the resumed one is whole.
    let served = trace.named("file served");
    let ended = |index: usize| served[index]["ended"].as_str().unwrap_or_default();
    assert_eq!(served.len(), 3, "each fetch is served once: {served:?}");
    assert_eq!(
        (ended(0), ended(2)),
        ("sent", "sent"),
        "the small file and the resumed large one are sent whole: {served:?}"
    );
    assert!(
        matches!(ended(1), "the receiver left" | "sent"),
        "the cancelled serve ends as its receiver leaves, or sent if it was already written: {served:?}"
    );
    Ok(())
}

/// A destination that refuses the replacement fails the job and keeps the
/// original; once writable, resuming lands the new file.
#[test]
fn a_refused_publication_keeps_the_original_and_resumes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let source = pair.host.folder().join("set");
    write_files(&source, &[("new.txt", 512)]);
    let destination = pair.controller.folder().join("got/set");
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
        // Root writes through permissions.
        if std::fs::write(destination.join("probe.txt"), b"p").is_ok() {
            let _ = std::fs::remove_file(destination.join("probe.txt"));
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o755))?;
            vnilink_media::needs::needs(
                "an account without root",
                Err("root writes into a read-only folder".to_owned()),
            );
            return Ok(());
        }
    }

    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Download, "set", "got")?;
    // Replace, so publication is attempted.
    poll_within(
        &pair.controller.core,
        job.controller,
        10,
        "the job to ask about a conflict",
        |s| s.state == JobState::WaitingDecision,
    );
    pair.controller
        .core
        .job_resolve(job.controller, ConflictChoice::Replace, false)?;
    let (_, got) = job.drive_to_end(&pair, TRANSFER);
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
    pair.controller.core.job_resume(job.controller)?;
    let (_, got) = job.drive_to_end(&pair, TRANSFER);
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
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=transferring job=1 sender=false to=waiting-decision
          files: job state from=waiting-decision job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=failed
          files: job state from=failed job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: received files flushed job=1
          files: job state done_bytes=512 files_done=1 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=512 files_done=0 from=serving job=1 sender=true to=failed
          files: job state from=failed job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=512 files_done=1 from=serving job=1 sender=true to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A transfer is refused before anything is scanned without a source that
/// exists, a root the computer knows, or a Files service still open.
#[test]
fn submitting_needs_a_files_service_an_existing_source_and_a_known_root() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let files = pair.open_files();
    assert!(
        matches!(
            upload(&pair, &files, "missing", "x"),
            Err(Error::InvalidSetting(_))
        ),
        "no transfer from a source that does not exist"
    );
    let (folder, names) = source(&pair.host.location().path(""));
    assert!(
        matches!(
            pair.controller.core.files_service_download(
                files.service.id,
                files.service.generation,
                pair.host.location().root,
                &folder,
                &names,
                u64::MAX,
                "x",
            ),
            Err(Error::InvalidSetting(_))
        ),
        "no transfer into a root this computer does not know"
    );
    pair.controller
        .core
        .close_controller_service(files.service.id, files.service.generation)?;
    assert!(
        matches!(upload(&pair, &files, "", "x"), Err(Error::NoSession)),
        "no transfer without a Files service"
    );
    vnilink_testkit::assert_story(expect![[r#""#]], || trace.story(&["files"]));
    Ok(())
}

/// Nested empty folders land although they carry no file.
#[test]
fn an_empty_folder_tree_lands_whole() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::create_dir_all(pair.controller.folder().join("empty/nested"))?;
    write_files(&pair.controller.folder().join("keep"), &[("a.txt", 1)]);
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "empty", "arrived")?;
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        tree(&pair.host.folder().join("arrived")),
        [
            ("empty".to_owned(), Entry::Folder),
            ("empty/nested".to_owned(), Entry::Folder),
        ]
        .into(),
        "the empty hierarchy lands, and nothing beside it"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state done_bytes=0 files_done=0 from=serving job=1 sender=true to=complete
        host:
          files: job state done_bytes=0 files_done=0 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A sender that cancels before its receiver fetched anything ends the job
/// on both sides, and nothing lands.
#[test]
fn a_job_its_sender_cancels_before_delivery_lands_nothing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.folder(), &[("a.txt", 1024)]);
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "a.txt", "got")?;
    pair.controller.core.job_cancel(job.controller)?;
    // The cancel reached the receiver inside `job_cancel`; one poll settles it.
    let after = pair.host.core.job_poll(job.host)?;
    assert_eq!(
        after.state,
        JobState::Cancelled,
        "the receiver follows its sender's cancel: {after:?}"
    );
    assert!(
        !pair.host.folder().join("got/a.txt").exists(),
        "a cancelled file never lands"
    );
    let sent = pair.controller.core.job_snapshot(job.controller)?;
    assert_eq!(
        sent.state,
        JobState::Cancelled,
        "the sender is cancelled: {sent:?}"
    );
    trace.wait_for_records("job state", 2);
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

/// The application's background drivers alone finish a job: no window
/// polls it.
#[test]
fn driving_every_job_finishes_one_without_window_polls() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.folder(), &[("a.txt", 1024)]);
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "a.txt", "got")?;
    let (sent, got) = wait_for("both sides to end", TRANSFER, || {
        pair.controller.core.drive_jobs().ok()?;
        pair.host.core.drive_jobs().ok()?;
        let sent = pair.controller.core.job_snapshot(job.controller).ok()?;
        let got = pair.host.core.job_snapshot(job.host).ok()?;
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
        std::fs::read(pair.host.folder().join("got/a.txt"))?,
        std::fs::read(pair.controller.folder().join("a.txt"))?,
        "the file lands exactly"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=1024 size=1024
          files: job state done_bytes=1024 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=1024 took=1024
          files: received files flushed job=1
          files: job state done_bytes=1024 files_done=1 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// An upload's conflict reaches its sender, which answers it without a
/// window on the receiving computer.
#[test]
fn an_upload_conflict_reaches_its_sender_who_answers_it() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(&pair.controller.folder(), &[("a.txt", 100)]);
    let destination = pair.host.folder().join("dest");
    std::fs::create_dir_all(&destination)?;
    std::fs::write(destination.join("a.txt"), b"old")?;
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "a.txt", "dest")?;
    let asked = pair.host.core.job_poll(job.host)?;
    assert_eq!(
        asked.state,
        JobState::WaitingDecision,
        "the receiver meets the conflict first: {asked:?}"
    );
    let mirrored = pair.controller.core.job_poll(job.controller)?;
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
        .job_resolve(job.controller, ConflictChoice::Replace, false)?;
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        std::fs::read(pair.controller.folder().join("a.txt"))?,
        "Replace lands the new file"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
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
          files: received files flushed job=1
          files: job state done_bytes=100 files_done=1 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A job its sender cancelled is cancelled on both sides; resuming the
/// receiver resumes the sender, and the files land.
#[test]
fn a_job_its_sender_cancelled_resumes_from_its_receiver() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    write_files(
        &pair.controller.folder().join("set"),
        &[("a.txt", 512), ("b.txt", 512)],
    );
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "set", "got")?;
    pair.controller.core.job_cancel(job.controller)?;
    let sent = pair.controller.core.job_snapshot(job.controller)?;
    assert_eq!(
        sent.state,
        JobState::Cancelled,
        "the sender cancels: {sent:?}"
    );
    let got = pair.host.core.job_poll(job.host)?;
    assert_eq!(
        got.state,
        JobState::Cancelled,
        "the receiver follows: {got:?}"
    );
    // The receiver's resume tells the sender, which offers again; resuming
    // the sender too is harmless, and keeps a raced notice from mattering.
    pair.host.core.job_resume(job.host)?;
    let _ = pair.controller.core.job_resume(job.controller);
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        tree(&pair.host.folder().join("got/set")),
        tree(&pair.controller.folder().join("set")),
        "the tree lands exactly"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
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
          files: received files flushed job=1
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A sender restarted after one file of a nested source resumes the files
/// under that source on a new Files service, found again by its full path
/// from the disk's root.
#[test]
fn a_restarted_sender_resumes_a_nested_source() -> TestResult {
    let trace = traced();
    let mut pair = Pair::for_files();
    write_files(
        &pair.controller.folder().join("docs/pack"),
        &[("sub/a.txt", 512), ("b.txt", 512)],
    );
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "docs/pack", "got")?;
    // One file lands; then the sender quits, and the receiver keeps its job.
    pair.host.core.job_poll(job.host)?;
    // The sender records each serve as it ends, after the receiver has it.
    trace.wait_for_records("file served", 2);
    pair.controller.restart();
    drop(files);
    // The new Files adopts the job before it resumes.
    let files = pair.open_files();
    let controller = &pair.controller.core;
    controller.files_service_adopt_job(
        files.service.id,
        files.service.generation,
        job.controller,
    )?;
    controller.files_service_job_resume(
        files.service.id,
        files.service.generation,
        job.controller,
    )?;
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        tree(&pair.host.folder().join("got/pack")),
        tree(&pair.controller.folder().join("docs/pack")),
        "the source's own folder lands, with its nested files"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
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
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=512 size=512 took=0
          files: received files flushed job=1
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
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
        // The host receives, so its staging crosses to its folder.
        pair.host.core.set_transfer_directory(&shm)?;
    }
    write_files(&pair.controller.folder(), &[("a.txt", 2048)]);
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "a.txt", "got")?;
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        std::fs::read(pair.host.folder().join("got/a.txt"))?,
        std::fs::read(pair.controller.folder().join("a.txt"))?,
        "the file lands exactly"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=2048 size=2048
          files: job state done_bytes=2048 files_done=1 from=serving job=1 sender=true to=complete
        host:
          files: file received outcome=received resumed_at=0 size=2048 took=2048
          files: received files flushed job=1
          files: job state done_bytes=2048 files_done=1 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A receiver answering Keep both while its sender already mirrors the
/// conflict lets the sender go on, and both files stay.
#[test]
fn a_receivers_keep_both_unblocks_a_sender_mirroring_the_conflict() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::write(pair.controller.folder().join("file.txt"), b"incoming")?;
    std::fs::create_dir_all(pair.host.folder().join("dest"))?;
    std::fs::write(pair.host.folder().join("dest/file.txt"), b"original")?;
    let files = pair.open_files();
    let job = Job::submit(&pair, &files, Way::Upload, "file.txt", "dest")?;
    let asked = pair.host.core.job_poll(job.host)?;
    assert_eq!(
        asked.state,
        JobState::WaitingDecision,
        "the receiver asks: {asked:?}"
    );
    let mirrored = pair.controller.core.job_poll(job.controller)?;
    assert_eq!(
        mirrored.state,
        JobState::WaitingDecision,
        "the sender mirrors the question: {mirrored:?}"
    );
    pair.host
        .core
        .job_resolve(job.host, ConflictChoice::KeepBoth, false)?;
    let (sent, got) = job.drive_to_end(&pair, TRANSFER);
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
        std::fs::read(pair.host.folder().join("dest/file.txt"))?,
        b"original",
        "Keep both keeps the original under its name"
    );
    assert_eq!(
        std::fs::read(pair.host.folder().join("dest/file (2).txt"))?,
        b"incoming",
        "Keep both lands the new file under the next free name"
    );
    trace.wait_for_records("job state", 2);
    vnilink_testkit::assert_story(
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
          files: received files flushed job=1
          files: job state done_bytes=8 files_done=1 from=transferring job=1 sender=false to=complete
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// Two small files in the host's `set` folder, for tests that stop a
/// download between them; the controller takes one at a time, fetching none ahead.
fn two_files_one_at_a_time(pair: &Pair) -> std::io::Result<()> {
    pair.controller.core.fetch_one_file_at_a_time();
    write_files(
        &pair.host.folder().join("set"),
        &[("one.txt", 512), ("two.txt", 512)],
    );
    std::fs::create_dir_all(pair.controller.folder().join("got"))
}

/// A host that stops sharing files after the first of two files lands
/// interrupts the download there, landing nothing more, and ends the Files
/// service it rode; sharing again, a new Files service adopts the download
/// and resumes it to the exact tree.
#[test]
fn a_download_the_host_stops_sharing_resumes_once_shared_again() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    two_files_one_at_a_time(&pair)?;
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let files = pair.open_files();
    let job = download(&pair, &files, "set", "got")?;
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
        !pair.controller.folder().join("got/set/two.txt").exists(),
        "nothing fetched after the withdrawal lands"
    );

    // The withdrawal ended the Files service on both sides, and the
    // connection stays: a new Files service on it adopts the interrupted
    // download, and resuming it there has the host's sender, adopted with
    // it, offer again before the receiver fetches again.
    host.set_service_sharing(vnilink::ServiceKind::Files, true)?;
    pair.host.wait_released(Some(vnilink::ServiceKind::Files));
    let (fresh, _pipe) = pair.controller.open_files(&files.connection)?;
    controller.files_service_adopt_job(fresh.id, fresh.generation, job)?;
    controller.files_service_job_resume(fresh.id, fresh.generation, job)?;
    let sender = only_job(host)?;
    let (_, got) = drive_to_end(host, sender, controller, job, TRANSFER);
    assert_eq!(
        got.state,
        JobState::Complete,
        "the download resumes: {got:?}"
    );
    assert_eq!(
        tree(&pair.controller.folder().join("got/set")),
        tree(&pair.host.folder().join("set")),
        "the resumed download lands the exact tree"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=interrupted job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: received files flushed job=1
          files: job state done_bytes=1024 files_done=2 from=transferring job=1 sender=false to=complete
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=interrupted job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state done_bytes=1024 files_done=2 from=serving job=1 sender=true to=complete
    "#]],
        || trace.story(&["files"]),
    );
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
    let files = pair.open_files();
    let job = download(&pair, &files, "set", "got")?;
    controller.job_poll(job)?;
    host.revoke(&pair.controller.endpoint())?;
    let stopped = controller.job_poll(job)?;
    assert_eq!(
        stopped.state,
        JobState::Interrupted,
        "the download stops with its connection: {stopped:?}"
    );
    // The host ends the Files service and then the link; the controller
    // learns whichever reaches it first.
    assert!(
        matches!(
            stopped.error.as_deref(),
            Some(
                "the other computer stopped sending" | "the connection to the other computer ended"
            )
        ),
        "the job says the other computer ended it: {stopped:?}"
    );
    assert!(
        controller.job_snapshot(job)?.files_done <= 1,
        "no file after the first counts as landed"
    );
    assert!(
        !pair.controller.folder().join("got/set/two.txt").exists(),
        "the second file is refused at publication"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
