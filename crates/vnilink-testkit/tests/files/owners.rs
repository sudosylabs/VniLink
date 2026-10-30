// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What a Files service holds for its owner, and what it does not:
//! transfers a lost owner left, and a download whose preparation takes
//! long.

use expect_test::expect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use vnilink::probes::{ScanHooks, MAX_ACTIVE_JOBS};
use vnilink::{Core, Error, JobState, ServicePhase};
use vnilink_testkit::files::{drive_to_end, payload};
use vnilink_testkit::{holds, lock, traced, wait_for, Pair, TestResult};

const TRANSFER: Duration = Duration::from_secs(30);

/// How long the owner tests waited for the engines to settle.
const SETTLED_WITHIN: Duration = Duration::from_secs(8);

/// Transfers left interrupted when their Files owner went wait to resume
/// and hold back nothing: with as many as may be active, one more is
/// accepted, and they all stay there to be taken over.
#[test]
fn interrupted_transfers_leave_room_for_new_ones() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let controller = &pair.controller.core;
    let session = pair.connect_services();
    let (lost, pipe) = pair.controller.open_files(&session)?;
    for index in 0..MAX_ACTIVE_JOBS {
        let name = format!("waiting-{index}.bin");
        std::fs::write(
            pair.controller.folder().join(&name),
            payload(512, 600 + index as u64),
        )?;
        controller.files_service_upload(
            lost.id,
            lost.generation,
            pair.controller.location().root,
            &pair.controller.location().path(""),
            &[name],
            pair.host.location().root,
            &pair.host.location().path(""),
        )?;
    }
    controller.close_controller_service(lost.id, lost.generation)?;
    drop(pipe);
    wait_for(
        "the lost owner's jobs to be interrupted",
        SETTLED_WITHIN,
        || {
            controller
                .job_list()
                .expect("the controller's jobs")
                .iter()
                .all(|job| job.state == JobState::Interrupted)
                .then_some(())
        },
    );
    wait_for("the host to release the lost owner", SETTLED_WITHIN, || {
        pair.host.hosted(None).is_empty().then_some(())
    });

    let (fresh, _pipe) = pair.controller.open_files(&session)?;
    std::fs::write(
        pair.controller.folder().join("ninth.bin"),
        payload(512, 699),
    )?;
    let ninth = controller.files_service_upload(
        fresh.id,
        fresh.generation,
        pair.controller.location().root,
        &pair.controller.location().path(""),
        &["ninth.bin".to_owned()],
        pair.host.location().root,
        &pair.host.location().path(""),
    )?;
    assert_eq!(
        controller.job_snapshot(ninth)?.state,
        JobState::Serving,
        "interrupted transfers leave room for a new one"
    );
    let waiting = controller
        .job_list()?
        .into_iter()
        .filter(|job| job.state == JobState::Interrupted)
        .count();
    assert_eq!(
        waiting, MAX_ACTIVE_JOBS,
        "every interrupted transfer stays to be taken over"
    );
    // Each side records the eight interruptions, and the ninth job starts.
    trace.wait_for_records("job state", 2 * MAX_ACTIVE_JOBS + MAX_ACTIVE_JOBS + 1);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=true to=serving
          files: job state from=preparing job=2 sender=true to=serving
          files: job state from=preparing job=3 sender=true to=serving
          files: job state from=preparing job=4 sender=true to=serving
          files: job state from=preparing job=5 sender=true to=serving
          files: job state from=preparing job=6 sender=true to=serving
          files: job state from=preparing job=7 sender=true to=serving
          files: job state from=preparing job=8 sender=true to=serving
          files: job state from=preparing job=9 sender=true to=serving
          files: job state from=serving job=1 sender=true to=interrupted
          files: job state from=serving job=2 sender=true to=interrupted
          files: job state from=serving job=3 sender=true to=interrupted
          files: job state from=serving job=4 sender=true to=interrupted
          files: job state from=serving job=5 sender=true to=interrupted
          files: job state from=serving job=6 sender=true to=interrupted
          files: job state from=serving job=7 sender=true to=interrupted
          files: job state from=serving job=8 sender=true to=interrupted
        host:
          files: job state from=transferring job=1 sender=false to=interrupted
          files: job state from=transferring job=2 sender=false to=interrupted
          files: job state from=transferring job=3 sender=false to=interrupted
          files: job state from=transferring job=4 sender=false to=interrupted
          files: job state from=transferring job=5 sender=false to=interrupted
          files: job state from=transferring job=6 sender=false to=interrupted
          files: job state from=transferring job=7 sender=false to=interrupted
          files: job state from=transferring job=8 sender=false to=interrupted
    "#]],
        || trace.concurrent_story(&["files"]),
    );
    Ok(())
}

/// Set once the host's preparation parked before reading anything.
static PREPARATION_PARKED: AtomicBool = AtomicBool::new(false);
/// Set when the test lets preparations read, and woken.
static PREPARATION_RELEASED: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Parks a download's preparation before it reads anything, until released
/// or cancelled. It looks at `cancelled` every few milliseconds, as a scan
/// probe must.
fn park_preparation(unit: u64, cancelled: &AtomicBool) {
    if unit != 0 {
        return;
    }
    PREPARATION_PARKED.store(true, Ordering::Release);
    let mut released = lock(&PREPARATION_RELEASED.0);
    while !*released && !cancelled.load(Ordering::Acquire) {
        released = PREPARATION_RELEASED
            .1
            .wait_timeout(released, Duration::from_millis(5))
            .unwrap_or_else(|poison| poison.into_inner())
            .0;
    }
}

/// Parks preparations again until [`release_preparations`].
fn hold_preparations() {
    PREPARATION_PARKED.store(false, Ordering::Release);
    *lock(&PREPARATION_RELEASED.0) = false;
}

fn release_preparations() {
    *lock(&PREPARATION_RELEASED.0) = true;
    PREPARATION_RELEASED.1.notify_all();
}

/// The host's preparations parked, until dropped.
struct ParkedPreparations<'a>(&'a Core);

impl<'a> ParkedPreparations<'a> {
    fn install(host: &'a Core) -> Self {
        hold_preparations();
        host.set_prepare_scan_hooks(Some(ScanHooks {
            dir_probe: Some(park_preparation),
            finish_probe: None,
        }));
        Self(host)
    }
}

impl Drop for ParkedPreparations<'_> {
    fn drop(&mut self) {
        self.0.set_prepare_scan_hooks(None);
        release_preparations();
    }
}

/// The ids of `core`'s jobs that are preparing.
fn preparing(core: &Core) -> Result<Vec<u64>, Error> {
    Ok(core
        .job_list()?
        .into_iter()
        .filter(|job| job.state == JobState::Preparing)
        .map(|job| job.id)
        .collect())
}

/// A download whose preparation takes long is a Preparing job at once: the
/// request is answered while the other computer has not read a byte, so
/// the Files owner is neither held for the whole preparation nor closed
/// when it outlasts the request's bound. Asking again changes nothing until
/// it is prepared; then it finishes. A cancel while preparing stops the
/// preparation on both sides.
#[test]
fn a_long_download_preparation_shows_and_finishes() -> TestResult {
    let trace = traced();
    let pair = Pair::for_files();
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let bytes = payload(4096, 801);
    std::fs::write(pair.host.folder().join("slow.bin"), &bytes)?;
    std::fs::write(pair.host.folder().join("second.bin"), payload(4096, 802))?;
    let session = pair.connect_services();
    let (owner, _pipe) = pair.controller.open_files(&session)?;
    let parked = ParkedPreparations::install(host);
    let download = |name: &str| {
        controller.files_service_download(
            owner.id,
            owner.generation,
            pair.host.location().root,
            &pair.host.location().path(""),
            &[name.to_owned()],
            pair.controller.location().root,
            &pair.controller.location().path(""),
        )
    };

    let job = download("slow.bin")?;
    assert!(
        PREPARATION_PARKED.load(Ordering::Acquire),
        "the download is answered only once its preparation started"
    );
    assert_eq!(
        controller.job_snapshot(job)?.state,
        JobState::Preparing,
        "a long preparation shows at once"
    );
    let host_job = *preparing(host)?.first().ok_or("the host's preparing job")?;
    holds(
        "the download to stay preparing",
        Duration::from_millis(1500),
        || {
            controller
                .job_snapshot(job)
                .is_ok_and(|snapshot| snapshot.state == JobState::Preparing)
        },
    );
    let asked = controller.job_poll(job)?;
    assert_eq!(
        asked.state,
        JobState::Preparing,
        "asking again changes nothing until it is prepared"
    );
    assert_eq!(asked.files_total, 0, "nothing is offered while preparing");
    assert_eq!(
        controller.controller_service(owner.id)?.phase,
        ServicePhase::Active,
        "a long preparation keeps the Files owner"
    );
    release_preparations();
    wait_for("the download to be prepared", SETTLED_WITHIN, || {
        (controller.job_poll(job).expect("the job's poll").state != JobState::Preparing)
            .then_some(())
    });
    let (sent, got) = drive_to_end(host, host_job, controller, job, TRANSFER);
    assert_eq!(
        (got.state, sent.state),
        (JobState::Complete, JobState::Complete),
        "once prepared, the download completes on both sides"
    );
    assert_eq!(
        std::fs::read(pair.controller.folder().join("slow.bin"))?,
        bytes,
        "the prepared file lands exactly"
    );

    hold_preparations();
    let second = download("second.bin")?;
    assert_eq!(
        controller.job_snapshot(second)?.state,
        JobState::Preparing,
        "the second download is preparing"
    );
    let host_second = *preparing(host)?
        .first()
        .ok_or("the host's second preparing job")?;
    assert_eq!(
        controller.job_cancel(second)?.state,
        JobState::Cancelled,
        "a preparing download can be cancelled"
    );
    wait_for("the host to cancel its preparation", SETTLED_WITHIN, || {
        (host
            .job_snapshot(host_second)
            .expect("the host's job")
            .state
            == JobState::Cancelled)
            .then_some(())
    });
    holds(
        "the cancelled preparation to stay stopped",
        Duration::from_millis(300),
        || {
            host.job_snapshot(host_second)
                .is_ok_and(|snapshot| snapshot.state == JobState::Cancelled)
                && !pair.controller.folder().join("second.bin").exists()
        },
    );
    drop(parked);
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          files: job state from=preparing job=1 sender=false to=transferring
          files: file received outcome=received resumed_at=0 size=4096 took=4096
          files: job state done_bytes=4096 files_done=1 from=transferring job=1 sender=false to=complete
          files: job state done_bytes=0 files_done=0 from=preparing job=2 sender=false to=cancelled
        host:
          files: job state from=preparing job=1 sender=true to=serving
          files: file served ended=sent from=0 sent=4096 size=4096
          files: job state done_bytes=4096 files_done=1 from=serving job=1 sender=true to=complete
          files: job state done_bytes=0 files_done=0 from=preparing job=2 sender=true to=cancelled
          files: a download prepared after its answer failed job=2 status=4
    "#]],
        || trace.story(&["files"]),
    );
    Ok(())
}
