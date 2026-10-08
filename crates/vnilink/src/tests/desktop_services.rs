use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{Core, EncodedFrame, InputEvent, PointerButton, ServiceKind, SessionPhase};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until, "Desktop condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn host_owner(core: &Core) -> crate::ServiceInfo {
    core.hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServiceKind::Desktop)
        .unwrap()
}

#[test]
fn neutral_desktop_stream_fallback_cursor_and_files_are_independent() {
    struct Capture;
    impl crate::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
        fn separate_cursor(&self) -> bool {
            true
        }
    }
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(std::env::temp_dir().join(format!(
            "desktop-files-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&scratch.0).unwrap();
    let host = start_direct();
    let controller = start_direct();
    controller.set_cursor_receiver(true).unwrap();
    host.set_capture(Box::new(Capture)).unwrap();
    host.set_file_roots(vec![scratch.0.clone()]).unwrap();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Files, true)
        .unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let files_owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .open_files_service(files_owner.id, files_owner.generation)
        .unwrap();
    let remote = host_owner(&host);
    host.force_desktop_stream(remote.id, remote.generation);
    let video = desktop.subscribe_video().unwrap();
    let image = Arc::new(crate::CursorImage::new(2, 2, (1, 1), vec![255; 16]).unwrap());
    let cursor = crate::CursorState {
        position: (25, 40),
        visible: true,
        image: Some(image),
    };
    host.publish_desktop_cursor(remote.id, remote.generation, cursor.clone())
        .unwrap();
    // Large reliable video bodies coexist with cursor metadata on the same stream.
    let payload = vec![42; 200_000];
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        EncodedFrame::h264(true, 1, payload.clone()).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        payload
    );
    wait(|| desktop.cursor().unwrap().as_deref() == Some(&cursor));
    let moved = crate::CursorState {
        position: (50, 60),
        ..cursor
    };
    host.publish_desktop_cursor(remote.id, remote.generation, moved.clone())
        .unwrap();
    wait(|| desktop.cursor().unwrap().as_deref() == Some(&moved));
    desktop.request_keyframe().unwrap();
    wait(|| {
        host.take_desktop_keyframe_request(remote.id, remote.generation)
            .unwrap()
    });
    desktop.close();
    wait(|| {
        host.hosted_services()
            .unwrap()
            .iter()
            .all(|s| s.kind != ServiceKind::Desktop)
    });
    files.mkdir(0, "", "still-connected").unwrap();
    assert!(scratch.0.join("still-connected").is_dir());
    assert!(!files.is_cancelled());
    assert_eq!(controller.controller_session(parent.id).unwrap(), parent);
}

#[test]
fn neutral_desktop_opens_reopens_without_reconnecting_and_releases_input() {
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: applied.clone(),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Desktop, false)
        .unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let session = controller.controller_session(pending.id).unwrap();
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .desktop
    );
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap();
    assert!(matches!(
        controller.open_desktop_service(owner.id, owner.generation),
        Err(crate::Error::Unauthorized)
    ));
    assert!(host.hosted_services().unwrap().is_empty());
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Desktop, true)
        .unwrap();
    let first = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let remote = host_owner(&host);
    assert_eq!(first.desktop_info().unwrap().monitor_width, 1920);
    assert_eq!(first.desktop_info().unwrap().monitor_height, 1080);
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_none());
    assert!(!host
        .host_desktop_active(
            host.snapshot().unwrap().incoming.unwrap().id,
            remote.generation
        )
        .unwrap());
    let video = first.subscribe_video().unwrap();
    assert!(matches!(first.subscribe_video(), Err(crate::Error::Busy)));
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        EncodedFrame::h264(true, 1, vec![1]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        vec![1]
    );
    let key = InputEvent::key_down(crate::input::usage::A);
    let button = InputEvent::button_down(PointerButton::Left);
    first.submit_input(key).unwrap();
    first.submit_input(button).unwrap();
    wait(|| applied.lock().unwrap().len() == 2);
    first.release_input().unwrap();
    wait(|| applied.lock().unwrap().len() == 4);
    assert_eq!(
        *applied.lock().unwrap(),
        vec![
            key,
            button,
            InputEvent::button_up(PointerButton::Left),
            InputEvent::key_up(crate::input::usage::A)
        ]
    );
    first.submit_input(key).unwrap();
    wait(|| applied.lock().unwrap().len() == 5);
    first.close();
    wait(|| applied.lock().unwrap().len() == 6 && host.hosted_services().unwrap().is_empty());
    assert!(first.submit_input(key).is_err());
    assert!(video.try_recv().is_none());
    assert!(host
        .publish_desktop_video(
            remote.id,
            remote.generation,
            EncodedFrame::h264(true, 2, vec![2]).unwrap()
        )
        .is_err());
    let next = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap();
    let second = controller
        .open_desktop_service(next.id, next.generation)
        .unwrap();
    let new_remote = host_owner(&host);
    assert_ne!(new_remote.id, remote.id);
    first.close();
    assert!(!second.is_cancelled());
    let second_video = second.subscribe_video().unwrap();
    host.publish_desktop_video(
        new_remote.id,
        new_remote.generation,
        EncodedFrame::h264(true, 3, vec![3]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        second_video
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .payload,
        vec![3]
    );
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    second.submit_input(key).unwrap();
    wait(|| applied.lock().unwrap().len() == 7);
    host.authorize_service(&controller.endpoint_id(), ServiceKind::Desktop, false)
        .unwrap();
    wait(|| second.is_cancelled() && applied.lock().unwrap().len() == 8);
    assert_eq!(controller.controller_session(session.id).unwrap(), session);
    assert!(second_video.try_recv().is_none());
}

#[test]
fn neutral_desktop_reconnect_replaces_runtime_and_fences_old_handle() {
    let host = start_direct();
    let controller = start_direct();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let session = controller.controller_session(pending.id).unwrap();
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .unwrap();
    let old = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    controller.drop_controller_link(session.id).unwrap();
    wait(|| {
        controller.controller_session(session.id).unwrap().phase == SessionPhase::Active
            && controller
                .controller_session(session.id)
                .unwrap()
                .generation
                > session.generation
    });
    let owner = controller.controller_service(owner.id).unwrap();
    let fresh = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    old.close();
    assert!(!fresh.is_cancelled());
    let remote = host_owner(&host);
    let video = fresh.subscribe_video().unwrap();
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        EncodedFrame::h264(true, 4, vec![4]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        vec![4]
    );
    assert!(old.subscribe_video().is_err());
    assert!(controller
        .controller_session(session.id)
        .unwrap()
        .quality
        .is_none());
}

#[test]
fn closing_one_neutral_desktop_keeps_another_computer_streaming() {
    let controller = start_direct();
    let hosts = [start_direct(), start_direct()];
    let mut sessions = Vec::new();
    let mut handles = Vec::new();
    let mut videos = Vec::new();
    for host in &hosts {
        host.set_desktop(Box::new(RecordingDesktop {
            events: Arc::new(Mutex::new(Vec::new())),
        }))
        .unwrap();
        pair_for_control(host, &controller);
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
        let parent = controller.controller_session(pending.id).unwrap();
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let handle = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        videos.push(handle.subscribe_video().unwrap());
        handles.push(handle);
        sessions.push(parent);
    }
    for (index, host) in hosts.iter().enumerate() {
        let owner = host_owner(host);
        host.publish_desktop_video(
            owner.id,
            owner.generation,
            EncodedFrame::h264(true, 1, vec![index as u8]).unwrap(),
        )
        .unwrap();
    }
    for (index, video) in videos.iter().enumerate() {
        assert_eq!(
            video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
            vec![index as u8]
        );
    }
    handles[0].close();
    wait(|| hosts[0].hosted_services().unwrap().is_empty());
    assert!(!handles[1].is_cancelled());
    let kept = host_owner(&hosts[1]);
    hosts[1]
        .publish_desktop_video(
            kept.id,
            kept.generation,
            EncodedFrame::h264(true, 2, vec![9]).unwrap(),
        )
        .unwrap();
    assert_eq!(
        videos[1]
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .payload,
        vec![9]
    );
    for session in sessions {
        assert_eq!(controller.controller_session(session.id).unwrap(), session);
    }
}

#[test]
fn idle_neutral_parent_allows_adapter_setup_but_active_desktop_refuses_it() {
    struct Capture;
    impl crate::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
    }
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    host.set_capture(Box::new(Capture)).unwrap();
    let input = || {
        Box::new(RecordingDesktop {
            events: Arc::new(Mutex::new(Vec::new())),
        })
    };
    host.set_desktop(input()).unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    assert!(matches!(
        host.set_capture(Box::new(Capture)),
        Err(crate::Error::Busy)
    ));
    assert!(matches!(host.set_desktop(input()), Err(crate::Error::Busy)));
    desktop.close();
    wait(|| host.set_desktop(input()).is_ok());
    host.set_capture(Box::new(Capture)).unwrap();
    assert_eq!(controller.controller_session(parent.id).unwrap(), parent);
}

#[test]
fn desktop_keyboard_modes_shortcuts_and_focus_release_are_independent() {
    use super::keyboard_support::{wait_seen, RecordingDesktop as KeyboardDesktop, Seen};
    use crate::input::usage;
    use crate::{KeyboardMode, LocalKey};
    let controller = start_direct();
    let hosts = [start_direct(), start_direct()];
    let seen = [
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    ];
    let mut desktops = Vec::new();
    let mut parents = Vec::new();
    for (index, host) in hosts.iter().enumerate() {
        host.set_desktop(Box::new(KeyboardDesktop {
            seen: seen[index].clone(),
            fail_text: false,
            fail_chord: false,
        }))
        .unwrap();
        pair_for_control(host, &controller);
        controller
            .set_keyboard_mode(if index == 0 {
                KeyboardMode::Map
            } else {
                KeyboardMode::Character
            })
            .unwrap();
        let parent = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        wait(|| controller.controller_session(parent.id).unwrap().phase == SessionPhase::Active);
        let parent = controller.controller_session(parent.id).unwrap();
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        desktops.push(
            controller
                .open_desktop_service(owner.id, owner.generation)
                .unwrap(),
        );
        parents.push(parent);
    }
    let first = &desktops[0];
    let second = &desktops[1];
    assert_eq!(first.keyboard_mode().unwrap(), KeyboardMode::Map);
    assert_eq!(second.keyboard_mode().unwrap(), KeyboardMode::Character);
    first.set_swap_control_and_command(true).unwrap();
    assert!(!second.swap_control_and_command().unwrap());
    let key = |usage, down, text| LocalKey { usage, down, text };
    first
        .submit_local_key(key(usage::LEFT_META, true, ""))
        .unwrap();
    first.submit_local_key(key(usage::A, true, "a")).unwrap();
    first.submit_local_key(key(usage::A, true, "a")).unwrap();
    assert!(first.set_keyboard_mode(KeyboardMode::Character).is_err());
    assert!(first.set_swap_control_and_command(false).is_err());
    wait_seen(
        &seen[0],
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::A)),
            Seen::Event(InputEvent::key_down(usage::A)),
        ],
    );
    first.release_input().unwrap();
    // Submit a fresh key immediately: release must precede it on the wire.
    first.submit_input(InputEvent::key_down(usage::B)).unwrap();
    wait_seen(
        &seen[0],
        &[
            Seen::Event(InputEvent::key_down(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::A)),
            Seen::Event(InputEvent::key_down(usage::A)),
            Seen::Event(InputEvent::key_up(usage::A)),
            Seen::Event(InputEvent::key_up(usage::LEFT_CONTROL)),
            Seen::Event(InputEvent::key_down(usage::B)),
        ],
    );
    second
        .submit_local_key(key(usage::GRAVE, true, "@"))
        .unwrap();
    wait_seen(&seen[1], &[Seen::Text("@".into())]);
    assert!(second.set_keyboard_mode(KeyboardMode::Map).is_err());
    second
        .submit_local_key(key(usage::GRAVE, false, ""))
        .unwrap();
    second.set_keyboard_mode(KeyboardMode::Translate).unwrap();
    second
        .submit_local_key(key(usage::GRAVE, true, "@"))
        .unwrap();
    second
        .submit_local_key(key(usage::GRAVE, false, ""))
        .unwrap();
    wait_seen(&seen[1], &[Seen::Text("@".into()), Seen::Chord("@".into())]);
    first.close();
    wait(|| hosts[0].hosted_services().unwrap().is_empty());
    assert!(!second.is_cancelled());
    assert_eq!(second.keyboard_mode().unwrap(), KeyboardMode::Translate);
    let next = controller
        .attach_controller_service(parents[0].id, parents[0].generation, ServiceKind::Desktop)
        .unwrap();
    let fresh = controller
        .open_desktop_service(next.id, next.generation)
        .unwrap();
    assert_eq!(fresh.keyboard_mode().unwrap(), KeyboardMode::Character);
    assert!(!fresh.swap_control_and_command().unwrap());
    first.close();
    assert!(!fresh.is_cancelled());
}
