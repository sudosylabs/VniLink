// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! A peer that shuts down mid-transfer: the other side learns it from the
//! shutdown itself, and says so, not from a dial to the stopped engine
//! that fails. How soon it learns is the files bench's to measure.
//!
//! Jobs here are driven by hand (`job_poll`), so the shutdown falls
//! between two exact files of a transfer.

use expect_test::expect;

use vnilink::{JobState, SessionPhase};
use vnilink_testkit::files::{download, payload, poll_within, received, upload};
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

/// What a job whose peer shut down says: it heard the shutdown, rather
/// than failing to reach the peer.
const SHUT_DOWN: &str = "the other computer shut down";

/// What a job says whose connection the peer's shutdown ended.
const CONNECTION_ENDED: &str = "the connection to the other computer ended";

/// What a job says whose fetch the peer refused as it shut down.
const SENDER_STOPPED: &str = "the other computer stopped sending";

/// A host that shuts down mid-download ends the controller's services
/// connection, and the controller's job is interrupted at once: it does
/// not dial the stopped host again.
#[test]
fn a_host_that_shuts_down_mid_download_interrupts_the_job_at_once() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    std::fs::write(pair.host.folder().join("one.txt"), payload(512, 1801))?;
    std::fs::write(pair.host.folder().join("two.txt"), payload(512, 1802))?;
    let controller = &pair.controller.core;
    let files = pair.open_files();
    controller.create_dir(
        pair.controller.location().root,
        &pair.controller.location().path(""),
        "got",
    )?;
    let job = download(&pair, &files, "", "got")?;
    poll_within(controller, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    // The second file is fetched beside the first: the host shuts down
    // once it has arrived too, and the job has more to do.
    trace.wait_for_records("file received", 2);
    pair.host.core.shutdown();
    // The next poll learns it from the connection the shutdown ended, not
    // from a dial that cannot reach the host.
    let snapshot = poll_within(controller, job, 1, "the job to stop", |s| {
        s.state.is_terminal() || s.state == JobState::Interrupted
    });
    assert_eq!(
        snapshot.state,
        JobState::Interrupted,
        "a job whose sender shut down is interrupted, to resume later"
    );
    // A fetch that reaches the host as it shuts down is refused, which
    // reads as the sender having stopped; each is its end, heard.
    assert!(
        matches!(
            snapshot.error.as_deref(),
            Some(CONNECTION_ENDED | SHUT_DOWN | SENDER_STOPPED)
        ),
        "the job heard its sender's end rather than failing to reach it: {snapshot:?}"
    );
    let ended = wait_for("the connection to end", DEFAULT_WAIT, || {
        let phase = controller
            .controller_session(files.connection.id)
            .ok()?
            .phase;
        matches!(phase, SessionPhase::Closed | SessionPhase::Failed(_)).then_some(phase)
    });
    assert_eq!(
        ended,
        SessionPhase::Closed,
        "a host that shut down ends the connection; it is not dialed again"
    );
    vnilink_testkit::assert_story(
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
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}

/// A controller that shuts down mid-upload ends the link and the Files
/// service the host's receiving job belongs to: the job is interrupted at
/// once, keeps what landed for the controller to adopt when it is back,
/// and says the other computer shut down.
#[test]
fn a_controller_that_shuts_down_mid_upload_interrupts_the_receiver_and_says_why() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    // The host receives one file at a time, to stop between the two.
    host.fetch_one_file_at_a_time();
    let source = pair.controller.folder().join("set");
    std::fs::create_dir_all(&source)?;
    std::fs::write(source.join("one.txt"), payload(512, 1803))?;
    std::fs::write(source.join("two.txt"), payload(512, 1804))?;
    let files = pair.open_files();
    upload(&pair, &files, "set", "arrived")?;
    let job = received(host);
    poll_within(host, job, 20, "the first file to land", |s| {
        s.files_done == 1
    });
    controller.shutdown();
    // The next poll learns it: the shutdown, not a failed dial.
    let snapshot = host.job_poll(job)?;
    assert_eq!(
        (snapshot.state, snapshot.error.as_deref()),
        (JobState::Interrupted, Some(SHUT_DOWN)),
        "a receiver whose sender shut down is interrupted, to be adopted, and says why"
    );
    assert_eq!(snapshot.files_done, 1, "what landed is kept");
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=512 size=512
          files: job state from=serving job=1 sender=true to=interrupted
        host:
          files: file received outcome=received resumed_at=0 size=512 took=512
          files: job state from=transferring job=1 sender=false to=interrupted
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
