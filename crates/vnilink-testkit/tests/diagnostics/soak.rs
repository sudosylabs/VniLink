//! Headless soak: connecting, opening a desktop, driving a frame and a key
//! through it, and closing everything again, many times over, leaves no
//! residue. The registries are the leak checks: controller services,
//! sessions and jobs empty and forgettable after every cycle, hosted
//! services empty, hosted sessions at the documented steady state of at
//! most one finished. Linux also prints resident memory and thread counts
//! as CI evidence; this file asserts counts, not artifacts.

use vnilink::{EncodedFrame, InputEvent, ServiceKind, SessionPhase};
use vnilink_testkit::desktop::InputLog;
use vnilink_testkit::session::service_ended;
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

const CYCLES: u32 = 25;

/// Resident set size in KiB and thread count, Linux only, read from
/// `/proc/self/status` with the standard library.
#[cfg(target_os = "linux")]
fn process_stats() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").expect("the process status");
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0)
    };
    let (rss, threads) = (field("VmRSS:"), field("Threads:"));
    assert!(
        rss > 0 && threads > 0,
        "the process reports its memory and threads"
    );
    (rss, threads)
}

/// Twenty-five sessions, after one to warm up, each leave the registries
/// as they found them.
#[test]
fn repeated_sessions_leave_no_residue() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let _applied = InputLog::install(&pair.host, (1920, 1080))?;

    // One warmup cycle so allocator and runtime caches settle before the
    // baseline; the bounds below measure growth across cycles, not startup.
    soak_cycle(&pair)?;
    #[cfg(target_os = "linux")]
    let (baseline_rss, baseline_threads) = process_stats();
    #[cfg(target_os = "linux")]
    let mut mid_rss = 0;

    let (host, controller) = (&pair.host.core, &pair.controller.core);
    for cycle in 0..CYCLES {
        soak_cycle(&pair)?;
        assert!(
            controller.controller_services()?.is_empty(),
            "cycle {cycle}: controller services leaked"
        );
        assert!(
            host.hosted_services()?.is_empty(),
            "cycle {cycle}: hosted services leaked"
        );
        assert!(
            controller.controller_sessions()?.is_empty(),
            "cycle {cycle}: controller sessions leaked"
        );
        // Finished hosted records sweep lazily on the next admission, so
        // the steady state is at most the just-finished cycle: bounded,
        // never growing.
        let hosted = host.hosted_sessions()?;
        assert!(
            hosted.len() <= 1 && hosted.iter().all(|s| s.phase == SessionPhase::Closed),
            "cycle {cycle}: hosted sessions leaked: {}",
            hosted.len()
        );
        assert!(
            controller.job_list()?.is_empty(),
            "cycle {cycle}: jobs leaked"
        );
        #[cfg(target_os = "linux")]
        if cycle == CYCLES / 2 {
            mid_rss = process_stats().0;
        }
    }

    // Process stats are printed, not asserted: RSS and thread counts are
    // process-global, so parallel tests dominate them. The registries
    // above are the precise retention checks; hosted emptiness in
    // particular proves the serve tasks ended, since only their guard
    // drops remove those records. The printed numbers are CI evidence.
    #[cfg(target_os = "linux")]
    {
        let (end_rss, end_threads) = process_stats();
        eprintln!(
            "soak rss KiB: baseline={baseline_rss} mid={mid_rss} end={end_rss} threads={baseline_threads}->{end_threads}"
        );
    }
    Ok(())
}

/// One full lifecycle: connect, attach a Desktop, open it, drive one frame
/// and one key, close everything and forget every record.
fn soak_cycle(pair: &Pair) -> TestResult {
    let (host, controller) = (&pair.host.core, &pair.controller.core);
    let session = pair.connect_control();
    let desktop = pair.attach(&session, ServiceKind::Desktop);
    let handle = controller.open_desktop_service(desktop.id, desktop.generation)?;
    let incoming = host
        .hosted_sessions()?
        .into_iter()
        .find(|s| s.peer_id == pair.controller.endpoint())
        .ok_or("the host holds the controller's session")?;
    host.publish_video(incoming.id, EncodedFrame::h264(true, 1, vec![7])?)?;
    controller.submit_input(
        session.id,
        session.generation,
        InputEvent::key_down(vnilink::usage::A),
    )?;
    handle.close();
    wait_for("the desktop to finish", DEFAULT_WAIT, || {
        let phase = controller
            .controller_service(desktop.id)
            .expect("the desktop's record")
            .phase;
        service_ended(&phase).then_some(())
    });
    controller.forget_controller_service(desktop.id)?;
    controller.disconnect_session(session.id)?;
    wait_for("the session to close", DEFAULT_WAIT, || {
        (controller
            .controller_session(session.id)
            .expect("the session's record")
            .phase
            == SessionPhase::Closed)
            .then_some(())
    });
    controller.forget_controller_session(session.id)?;
    // The host settles once the controller is gone: hosted records finish
    // through the service tasks and the session record closes. There is
    // no hosted forget; finished records sweep on the next admission.
    pair.host.wait_released(None);
    wait_for("the host's sessions to close", DEFAULT_WAIT, || {
        host.hosted_sessions()
            .expect("the host's sessions")
            .iter()
            .all(|s| s.phase == SessionPhase::Closed)
            .then_some(())
    });
    Ok(())
}
