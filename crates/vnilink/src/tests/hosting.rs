use std::sync::{Arc, Mutex};

use crate::error::Error;
use crate::input::InputEvent;
use crate::session::{Grants, SessionPhase, SessionRole};

use super::support::*;

#[test]
fn input_for_the_wrong_session_or_generation_is_rejected() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id + 1,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation + 1,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::StaleSession)
    ));
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller.disconnect().unwrap();
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_up(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn stopping_hosting_disables_new_sessions_even_when_none_is_active() {
    let idle = start_direct();
    idle.stop_hosting().unwrap();
    assert!(!idle.snapshot().unwrap().hosting);

    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    assert!(matches!(host.stop_hosted_session(), Err(Error::NoSession)));
    assert!(host.snapshot().unwrap().hosting);
    host.stop_hosting().unwrap();
    assert!(!host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    assert!(host.snapshot().unwrap().devices[0].grants.control);
}

#[test]
fn stopping_hosting_ends_the_active_session_and_releases_keys() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    host.stop_hosting().unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    assert!(!host.snapshot().unwrap().hosting);
    assert!(host.snapshot().unwrap().devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotHosting)
    ));
    host.start_hosting().unwrap();
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}
#[test]
fn disconnect_closes_both_sides_and_releases_held_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Left),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
        ],
    );
    controller.disconnect().unwrap();
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::button_down(crate::input::PointerButton::Left),
            InputEvent::button_up(crate::input::PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    assert!(host.snapshot().unwrap().hosting);
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
    assert!(matches!(controller.disconnect(), Err(Error::NoSession)));
    let again = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(again.phase, SessionPhase::Active);
}

#[test]
fn revoke_stops_the_session_releases_input_and_blocks_reconnect() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
        )
        .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::button_down(crate::input::PointerButton::Right),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
        ],
    );
    host.revoke(&controller.endpoint_id()).unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::button_down(crate::input::PointerButton::Right),
            InputEvent::button_up(crate::input::PointerButton::Right),
            InputEvent::key_up(crate::input::usage::LEFT_SHIFT),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    let devices = host.snapshot().unwrap().devices;
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].endpoint_id, controller.endpoint_id());
    assert!(!devices[0].grants.control);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn unpair_forgets_the_device_stops_the_session_and_blocks_reconnect() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[InputEvent::key_down(crate::input::usage::LEFT_SHIFT)],
    );
    host.unpair(&controller.endpoint_id()).unwrap();
    // Held input releases like any local stop.
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::LEFT_SHIFT),
            InputEvent::key_up(crate::input::usage::LEFT_SHIFT),
        ],
    );
    wait_closed(&host, SessionRole::Host);
    wait_closed(&controller, SessionRole::Controller);
    // The device is gone, not kept with empty grants. This side forgot, so
    // the controller still dials — and the host rejects a peer it no longer
    // knows with Unauthorized instead of admitting it.
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        controller.submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        ),
        Err(Error::NoSession)
    ));
}

#[test]
fn unpairing_an_unknown_device_reports_not_paired() {
    let host = start_direct();
    let controller = start_direct();
    assert!(matches!(
        host.unpair(&controller.endpoint_id()),
        Err(Error::NotPaired)
    ));
    // Pairing still works afterwards; nothing was disturbed.
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    // The caller forgetting its peer fails before dialing.
    controller.unpair(&host.endpoint_id()).unwrap();
    wait_closed(&controller, SessionRole::Controller);
    assert!(matches!(
        controller.connect(&host.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn a_controller_unpair_ends_its_outgoing_session() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.connect(&host.endpoint_id()).unwrap();
    controller.unpair(&host.endpoint_id()).unwrap();
    assert!(controller.snapshot().unwrap().devices.is_empty());
    wait_closed(&controller, SessionRole::Controller);
    // Unpairing is local only: the host still lists the controller, and its
    // side closes because the control stream ended.
    wait_closed(&host, SessionRole::Host);
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
}

#[test]
fn removing_control_mid_session_stops_applying_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let session = controller.connect(&host.endpoint_id()).unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    wait_applied(&applied, &[InputEvent::key_down(crate::input::usage::A)]);
    host.authorize(
        &controller.endpoint_id(),
        Grants {
            view: true,
            control: false,
            clipboard: true,
            transfer: false,
        },
    )
    .unwrap();
    controller
        .submit_input(
            session.id,
            session.generation,
            InputEvent::key_down(crate::input::usage::B),
        )
        .unwrap();
    // Input revocation releases held input while preserving viewing.
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::key_up(crate::input::usage::A),
        ],
    );
    let quality = crate::protocol::Quality {
        max_bitrate_kbps: 1500,
        max_frame_rate: 20,
        max_width: 1280,
        max_height: 720,
    };
    controller
        .update_session_quality(session.id, SessionRole::Controller, quality)
        .unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while host.snapshot().unwrap().incoming.as_ref().unwrap().quality != Some(quality) {
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(
        *applied.lock().unwrap(),
        vec![
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::key_up(crate::input::usage::A)
        ]
    );
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        controller.controller_session(session.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn quality_updates_select_the_role_with_both_directions_active() {
    use crate::protocol::Quality;

    let a = start_direct();
    let b = start_direct();
    // Each core hosts the other. Local handles are unique across roles;
    // role-specific quality APIs must still update only the selected direction.
    pair_for_control(&a, &b);
    pair_for_control(&b, &a);
    let b_out = b.connect(&a.endpoint_id()).unwrap();
    let a_out = a.connect(&b.endpoint_id()).unwrap();
    let a_host_id = a.snapshot().unwrap().incoming.unwrap().id;
    assert_ne!(a_host_id, a_out.id);
    assert_ne!(b.snapshot().unwrap().incoming.unwrap().id, b_out.id);

    let controller_quality = Quality::new(1_500, 30, 1280, 720).unwrap();
    a.update_session_quality(a_out.id, SessionRole::Controller, controller_quality)
        .unwrap();
    assert_eq!(
        a.snapshot().unwrap().outgoing.unwrap().quality,
        Some(controller_quality)
    );
    // The hosted session keeps the negotiated limits, not the controller's.
    assert_ne!(
        a.snapshot().unwrap().incoming.unwrap().quality,
        Some(controller_quality)
    );

    let host_quality = Quality::new(800, 15, 640, 480).unwrap();
    a.update_session_quality(a_host_id, SessionRole::Host, host_quality)
        .unwrap();
    assert_eq!(
        a.snapshot().unwrap().incoming.unwrap().quality,
        Some(host_quality)
    );
    assert_eq!(
        a.snapshot().unwrap().outgoing.unwrap().quality,
        Some(controller_quality)
    );
}

#[test]
fn concurrent_peers_operate_independently_within_capacity() {
    let host = start_direct();
    let viewers = [start_direct(), start_direct()];
    let worker = start_direct();
    let extra = [start_direct(), start_direct()];
    for peer in viewers.iter().chain([&worker]).chain(extra.iter()) {
        pair_for_control(&host, peer);
    }
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    let root = std::env::temp_dir().join(format!("vnilink-multi-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    host.set_file_roots(vec![root.clone()]).unwrap();
    host.authorize_service(&worker.endpoint_id(), crate::ServiceKind::Files, true)
        .unwrap();
    host.authorize(
        &worker.endpoint_id(),
        Grants {
            view: true,
            control: true,
            clipboard: false,
            transfer: true,
        },
    )
    .unwrap();

    // Two viewers and one worker are live at once with distinct records.
    let va = viewers[0].connect(&host.endpoint_id()).unwrap();
    let vb = viewers[1].connect(&host.endpoint_id()).unwrap();
    let files_session = worker.connect_files(&host.endpoint_id()).unwrap();
    assert_eq!(host.hosted_sessions().unwrap().len(), 3);
    let mut peers: Vec<String> = host
        .hosted_sessions()
        .unwrap()
        .into_iter()
        .map(|s| s.peer_id)
        .collect();
    peers.sort();
    let mut expected = vec![
        viewers[0].endpoint_id(),
        viewers[1].endpoint_id(),
        worker.endpoint_id(),
    ];
    expected.sort();
    assert_eq!(peers, expected);
    let targets = host.video_targets().unwrap();
    assert_eq!(
        targets.len(),
        2,
        "a files session is not a capture consumer"
    );
    assert!(targets.iter().all(|target| !target.service));
    let target_ids: Vec<u64> = targets.iter().map(|target| target.id).collect();
    for session in host.hosted_sessions().unwrap() {
        if session.peer_id == worker.endpoint_id() {
            assert!(!target_ids.contains(&session.id));
        } else {
            assert!(target_ids.contains(&session.id));
        }
    }

    // Input control is an explicit lease: the earliest live desktop session
    // holds it (viewer A here) while viewers coexist. Each transition is
    // awaited before the next is sent, so shared-adapter order is exact.
    // Held keys are released before disconnecting, so closing a peer emits
    // no further input of its own.
    viewers[0]
        .submit_input(
            va.id,
            va.generation,
            InputEvent::key_down(crate::input::usage::A),
        )
        .unwrap();
    wait_applied(&applied, &[InputEvent::key_down(crate::input::usage::A)]);
    viewers[0]
        .submit_input(
            va.id,
            va.generation,
            InputEvent::key_up(crate::input::usage::A),
        )
        .unwrap();
    let pair = [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::key_up(crate::input::usage::A),
    ];
    wait_applied(&applied, &pair);
    // Viewer B is connected but without the lease: its input is discarded,
    // its session stays Active, and nothing reaches the adapter.
    viewers[1]
        .submit_input(
            vb.id,
            vb.generation,
            InputEvent::key_down(crate::input::usage::B),
        )
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert_eq!(
        *applied.lock().unwrap(),
        pair,
        "contended input must not reach the adapter"
    );
    assert_eq!(
        viewers[1].controller_session(vb.id).unwrap().phase,
        SessionPhase::Active
    );
    worker
        .fs_mkdir(
            files_session.id,
            files_session.generation,
            0,
            "",
            "from-worker",
        )
        .unwrap();
    assert!(root.join("from-worker").is_dir());

    // Capacity is explicit: the fifth concurrent peer is refused with Busy,
    // without disturbing the live sessions.
    let vd = extra[0].connect(&host.endpoint_id()).unwrap();
    assert_eq!(vd.phase, SessionPhase::Active);
    assert_eq!(host.hosted_sessions().unwrap().len(), 4);
    assert!(matches!(
        extra[1].connect(&host.endpoint_id()),
        Err(Error::Busy)
    ));
    assert_eq!(host.hosted_sessions().unwrap().len(), 4);
    extra[0].disconnect().unwrap();
    wait_until_hosted(&host, 3);

    // Closing the holder passes the lease deterministically: viewer B's
    // input now reaches the adapter while the worker is unaffected.
    viewers[0].disconnect().unwrap();
    wait_until_hosted(&host, 2);
    viewers[1]
        .submit_input(
            vb.id,
            vb.generation,
            InputEvent::key_down(crate::input::usage::C),
        )
        .unwrap();
    wait_applied(
        &applied,
        &[
            InputEvent::key_down(crate::input::usage::A),
            InputEvent::key_up(crate::input::usage::A),
            InputEvent::key_down(crate::input::usage::C),
        ],
    );

    // Revoking one peer stops only its session.
    host.revoke(&worker.endpoint_id()).unwrap();
    wait_until_hosted(&host, 1);
    assert_eq!(
        viewers[1].controller_session(vb.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        host.hosted_sessions().unwrap()[0].peer_id,
        viewers[1].endpoint_id()
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn wait_until_hosted(host: &crate::core::Core, count: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if host.hosted_sessions().unwrap().len() == count {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("hosted session count did not settle");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
