//! Headless soak: repeated connect/service/open/close/disconnect cycles must
//! leave no residue. The leak checks are the registries (controller
//! services/sessions/jobs empty and forgettable after every cycle, hosted
//! services empty, hosted sessions at the documented ≤1-finished steady
//! state). Linux also prints RSS/thread samples as CI evidence. Logs and
//! machine-specific artifacts stay out of the tree: this file asserts
//! counts, not artifacts.

use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{Core, Grants, ServiceKind, SessionPhase};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CYCLES: u32 = 25;

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "soak condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn active(core: &Core, id: u64) {
    wait(|| core.controller_session(id).unwrap().phase == SessionPhase::Active);
}

fn closed(core: &Core, id: u64) {
    wait(|| core.controller_session(id).unwrap().phase == SessionPhase::Closed);
}

/// Resident set size in KiB and thread count, Linux only. Parsed from
/// `/proc/self/status` with the standard library so the soak needs no new
/// dependency.
#[cfg(target_os = "linux")]
fn process_stats() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let mut rss = 0;
    let mut threads = 0;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = rest.split_whitespace().next().unwrap().parse().unwrap();
        }
        if let Some(rest) = line.strip_prefix("Threads:") {
            threads = rest.split_whitespace().next().unwrap().parse().unwrap();
        }
    }
    assert!(rss > 0 && threads > 0);
    (rss, threads)
}

#[test]
fn repeated_sessions_leave_no_residue() {
    let controller = start_direct();
    let host = start_direct();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        ..Grants::default()
    })
    .unwrap();

    // One warmup cycle so allocator and runtime caches settle before the
    // baseline; the bounds below measure growth across cycles, not startup.
    soak_cycle(&controller, &host);
    #[cfg(target_os = "linux")]
    let (baseline_rss, baseline_threads) = process_stats();
    #[cfg(target_os = "linux")]
    let mut mid_rss = 0;

    for cycle in 0..CYCLES {
        soak_cycle(&controller, &host);
        assert!(
            controller.controller_services().unwrap().is_empty(),
            "cycle {cycle}: controller services leaked"
        );
        assert!(
            host.hosted_services().unwrap().is_empty(),
            "cycle {cycle}: hosted services leaked"
        );
        assert!(
            controller.controller_sessions().unwrap().is_empty(),
            "cycle {cycle}: controller sessions leaked"
        );
        // Finished hosted records sweep lazily on the next admission, so
        // the steady state is at most the just-finished cycle: bounded,
        // never growing.
        let hosted = host.hosted_sessions().unwrap();
        assert!(
            hosted.len() <= 1 && hosted.iter().all(|s| s.phase == SessionPhase::Closed),
            "cycle {cycle}: hosted sessions leaked: {}",
            hosted.len()
        );
        assert!(
            controller.job_list().unwrap().is_empty(),
            "cycle {cycle}: jobs leaked"
        );
        #[cfg(target_os = "linux")]
        if cycle == CYCLES / 2 {
            mid_rss = process_stats().0;
        }
    }

    // Process stats are printed, not asserted: RSS and thread counts are
    // process-global, so parallel tests dominate them (measured +2.8 MiB
    // decelerating with flat threads alone vs +19 MiB under full load for
    // identical cycles). The registries above are the precise retention
    // checks — hosted emptiness in particular proves the serve tasks ended,
    // since only their guard drops remove those records. The printed
    // numbers are CI evidence.
    #[cfg(target_os = "linux")]
    {
        let (end_rss, end_threads) = process_stats();
        eprintln!(
            "soak rss KiB: baseline={baseline_rss} mid={mid_rss} end={end_rss} threads={baseline_threads}->{end_threads}"
        );
    }

    controller.shutdown();
    host.shutdown();
}

/// One full lifecycle: connect, attach Desktop, open it, drive one frame
/// and one input round-trip, close everything, forget every record.
fn soak_cycle(controller: &Core, host: &Core) {
    let pending = controller.begin_connect(&host.endpoint_id()).unwrap();
    active(controller, pending.id);
    let session = controller.controller_session(pending.id).unwrap();
    let desktop = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap();
    let handle = controller
        .open_desktop_service(desktop.id, desktop.generation)
        .unwrap();
    let incoming = host
        .hosted_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.peer_id == controller.endpoint_id())
        .unwrap();
    host.publish_video(
        incoming.id,
        crate::EncodedFrame::h264(true, 1, vec![7]).unwrap(),
    )
    .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            crate::InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    handle.close();
    wait(|| {
        controller
            .controller_service(desktop.id)
            .unwrap()
            .phase
            .finished()
    });
    controller.forget_controller_service(desktop.id).unwrap();
    controller.disconnect_session(session.id).unwrap();
    closed(controller, session.id);
    controller.forget_controller_session(session.id).unwrap();
    // The host side settles once the controller is gone: hosted records
    // finish via the service tasks and the session record closes. There is
    // no hosted forget; finished records sweep on the next admission.
    wait(|| host.hosted_services().unwrap().is_empty());
    wait(|| {
        host.hosted_sessions()
            .unwrap()
            .iter()
            .all(|s| s.phase == SessionPhase::Closed)
    });
}
