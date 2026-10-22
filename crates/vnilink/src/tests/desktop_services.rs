use super::filesystem_location::FilesystemLocation;
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
    host.enable_filesystem_access().unwrap();
    let location = FilesystemLocation::new(host.file_roots().unwrap(), &scratch.0);
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
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
    files
        .mkdir(location.root, &location.path(""), "still-connected")
        .unwrap();
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
    host.set_service_sharing(ServiceKind::Desktop, false)
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
    host.set_service_sharing(ServiceKind::Desktop, true)
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
    host.set_service_sharing(ServiceKind::Desktop, false)
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
    let key = |usage, down, text| LocalKey::new(usage, down, text);
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

#[test]
fn pointer_policy_is_owner_scoped_and_swapped_drag_releases_the_same_button() {
    let controller = start_direct();
    let host = start_direct();
    let seen = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: seen.clone(),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
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
    let policy = crate::PointerSettings {
        sensitivity_percent: 50,
        reverse_wheel: true,
        swap_buttons: true,
    };
    desktop.set_pointer_settings(policy).unwrap();
    assert!(desktop
        .submit_input(InputEvent::precise_wheel(9000, 0))
        .is_err());
    desktop.submit_input(InputEvent::motion(1, 0)).unwrap();
    desktop
        .submit_input(InputEvent::button_down(PointerButton::Left))
        .unwrap();
    assert!(desktop
        .set_pointer_settings(crate::PointerSettings::default())
        .is_err());
    desktop.submit_input(InputEvent::motion(1, 0)).unwrap();
    desktop
        .submit_input(InputEvent::precise_wheel(15, -30))
        .unwrap();
    desktop
        .submit_input(InputEvent::button_down(PointerButton::Back))
        .unwrap();
    wait(|| seen.lock().unwrap().len() == 4);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            InputEvent::button_down(PointerButton::Right),
            InputEvent::motion(1, 0),
            InputEvent::precise_wheel(-15, 30),
            InputEvent::button_down(PointerButton::Back),
        ]
    );
    desktop.release_input().unwrap();
    wait(|| seen.lock().unwrap().len() == 6);
    assert_eq!(
        &seen.lock().unwrap()[4..],
        &[
            InputEvent::button_up(PointerButton::Back),
            InputEvent::button_up(PointerButton::Right),
        ]
    );
    desktop
        .set_pointer_settings(crate::PointerSettings::default())
        .unwrap();
    desktop.close();
    wait(|| host.hosted_services().unwrap().is_empty());
    assert!(desktop.set_pointer_settings(policy).is_err());
    let fresh = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let fresh = controller
        .open_desktop_service(fresh.id, fresh.generation)
        .unwrap();
    assert_eq!(
        fresh.pointer_settings().unwrap(),
        crate::PointerSettings::default()
    );
    fresh.close();
    controller.shutdown();
    host.shutdown();
}

#[test]
fn explicit_control_handoff_releases_held_input_without_closing_viewers() {
    use crate::DesktopControlState::{Contended, Controlling, Viewing};
    let host = start_direct();
    let peers = [start_direct(), start_direct(), start_direct()];
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: applied.clone(),
    }))
    .unwrap();
    let mut desktops = Vec::new();
    let mut owners = Vec::new();
    let mut videos = Vec::new();
    for peer in &peers {
        pair_for_control(&host, peer);
        let parent = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        wait(|| peer.controller_session(parent.id).unwrap().phase == SessionPhase::Active);
        let parent = peer.controller_session(parent.id).unwrap();
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        let desktop = Arc::new(
            peer.open_desktop_service(owner.id, owner.generation)
                .unwrap(),
        );
        let hosted = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.peer_id == peer.endpoint_id() && s.kind == ServiceKind::Desktop)
            .unwrap();
        videos.push(desktop.subscribe_video().unwrap());
        owners.push(hosted);
        desktops.push(desktop);
    }
    wait(|| {
        desktops[0].control_state().unwrap() == Controlling
            && desktops[1].control_state().unwrap() == Contended
            && desktops[2].control_state().unwrap() == Contended
    });
    let down = InputEvent::key_down(crate::input::usage::A);
    let up = InputEvent::key_up(crate::input::usage::A);
    desktops[0].submit_input(down).unwrap();
    wait(|| *applied.lock().unwrap() == vec![down]);
    desktops[1].acquire_control().unwrap();
    desktops[1].request_keyframe().unwrap();
    wait(|| {
        host.take_desktop_keyframe_request(owners[1].id, owners[1].generation)
            .unwrap()
    });
    assert_eq!(desktops[1].control_state().unwrap(), Contended);
    // Host-directed handoff releases A before the next owner can press A.
    host.transfer_desktop_control(
        owners[0].id,
        owners[0].generation,
        owners[1].id,
        owners[1].generation,
    )
    .unwrap();
    wait(|| {
        desktops[0].control_state().unwrap() == Viewing
            && desktops[1].control_state().unwrap() == Controlling
    });
    assert_eq!(*applied.lock().unwrap(), vec![down, up]);
    desktops[1].submit_input(down).unwrap();
    wait(|| *applied.lock().unwrap() == vec![down, up, down]);
    assert!(matches!(
        host.transfer_desktop_control(
            owners[0].id,
            owners[0].generation,
            owners[2].id,
            owners[2].generation
        ),
        Err(crate::Error::Busy)
    ));
    desktops[0].close();
    wait(|| host.hosted_services().unwrap().len() == 2);
    assert_eq!(
        *applied.lock().unwrap(),
        vec![down, up, down],
        "old cleanup must not release the new owner's A"
    );
    desktops[1].yield_control().unwrap();
    wait(|| desktops[1].control_state().unwrap() == Viewing);
    assert_eq!(*applied.lock().unwrap(), vec![down, up, down, up]);
    // Both viewers explicitly compete for a free lease; exactly one wins.
    std::thread::scope(|scope| {
        for desktop in &desktops[1..] {
            scope.spawn(|| desktop.acquire_control().unwrap());
        }
    });
    wait(|| {
        let states: Vec<_> = desktops[1..]
            .iter()
            .map(|d| d.control_state().unwrap())
            .collect();
        states.iter().filter(|s| **s == Controlling).count() == 1 && states.contains(&Contended)
    });
    for index in 1..3 {
        let owner = &owners[index];
        host.publish_desktop_video(
            owner.id,
            owner.generation,
            EncodedFrame::h264(true, 10, vec![index as u8]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            videos[index]
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .payload,
            vec![index as u8]
        );
        assert!(!desktops[index].is_cancelled());
    }
    assert!(desktops[0].acquire_control().is_err());
    assert!(host
        .transfer_desktop_control(
            owners[0].id,
            owners[0].generation,
            owners[1].id,
            owners[1].generation
        )
        .is_err());
}

#[test]
fn physical_capture_startup_is_fenced_to_its_original_desktop_consumers() {
    use std::sync::atomic::AtomicBool;
    struct Capture;
    impl crate::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
    }
    let host = start_direct();
    host.set_capture(Box::new(Capture)).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    assert!(matches!(
        host.desktop_capture_startup(0, stop.clone(), Duration::from_secs(5)),
        Err(crate::Error::NoSession)
    ));
    let peers = [start_direct(), start_direct()];
    let mut parents = Vec::new();
    let mut desktops = Vec::new();
    for peer in &peers {
        pair_for_control(&host, peer);
        let pending = peer.begin_connect_services(&host.endpoint_id()).unwrap();
        wait(|| peer.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
        let parent = peer.controller_session(pending.id).unwrap();
        let owner = peer
            .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
            .unwrap();
        desktops.push(
            peer.open_desktop_service(owner.id, owner.generation)
                .unwrap(),
        );
        parents.push(parent);
    }
    let startup = host
        .desktop_capture_startup(0, stop.clone(), Duration::from_secs(5))
        .unwrap();
    desktops[0].close();
    wait(|| host.video_targets().unwrap().len() == 1);
    assert!(startup.check().is_ok());
    desktops[1].close();
    wait(|| host.video_targets().unwrap().is_empty());
    assert!(matches!(startup.check(), Err(crate::Error::NoSession)));
    let owner = peers[0]
        .attach_controller_service(parents[0].id, parents[0].generation, ServiceKind::Desktop)
        .unwrap();
    let replacement = peers[0]
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    assert!(matches!(startup.check(), Err(crate::Error::NoSession)));
    let fresh = host
        .desktop_capture_startup(0, stop.clone(), Duration::from_secs(5))
        .unwrap();
    assert!(fresh.check().is_ok());
    host.shutdown();
    assert!(matches!(fresh.check(), Err(crate::Error::NoSession)));
    drop(replacement);
}

#[test]
fn view_only_desktop_cannot_acquire_input_and_input_revoke_preserves_video() {
    let host = start_direct();
    let controller = start_direct();
    let events = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: events.clone(),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    let grants = crate::Grants {
        view: true,
        control: false,
        clipboard: false,
        transfer: false,
    };
    host.set_desktop_sharing(grants).unwrap();
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
    let remote = host_owner(&host);
    assert_eq!(
        desktop.control_state().unwrap(),
        crate::DesktopControlState::Viewing
    );
    desktop.acquire_control().unwrap();
    desktop
        .submit_input(InputEvent::key_down(crate::input::usage::A))
        .unwrap();
    host.take_desktop_keyframe_request(remote.id, remote.generation)
        .unwrap();
    desktop.request_keyframe().unwrap();
    wait(|| {
        host.take_desktop_keyframe_request(remote.id, remote.generation)
            .unwrap()
    });
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(
        desktop.control_state().unwrap(),
        crate::DesktopControlState::Viewing
    );
    host.set_desktop_sharing(crate::Grants {
        control: true,
        ..grants
    })
    .unwrap();
    desktop.acquire_control().unwrap();
    wait(|| desktop.control_state().unwrap() == crate::DesktopControlState::Controlling);
    desktop
        .submit_input(InputEvent::key_down(crate::input::usage::B))
        .unwrap();
    wait(|| !events.lock().unwrap().is_empty());
    host.set_desktop_sharing(grants).unwrap();
    wait(|| desktop.control_state().unwrap() == crate::DesktopControlState::Viewing);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            InputEvent::key_down(crate::input::usage::B),
            InputEvent::key_up(crate::input::usage::B)
        ]
    );
    host.force_desktop_stream(remote.id, remote.generation);
    let video = desktop.subscribe_video().unwrap();
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        EncodedFrame::h264(true, 1, vec![7]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video.recv_timeout(Duration::from_secs(5)).unwrap().payload,
        vec![7]
    );
    host.set_service_sharing(ServiceKind::Desktop, false)
        .unwrap();
    wait(|| desktop.is_cancelled());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn measured_receiver_pressure_adapts_only_its_owner_and_sibling_keeps_streaming() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    let open = |controller: &Core| {
        pair_for_control(&host, controller);
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
        let session = controller.controller_session(pending.id).unwrap();
        let owner = controller
            .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
            .unwrap();
        let desktop = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let remote = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.peer_id == controller.endpoint_id() && s.kind == ServiceKind::Desktop)
            .unwrap();
        host.force_desktop_stream(remote.id, remote.generation);
        (desktop, remote)
    };
    let (slow, slow_host) = open(&first);
    let (fast, fast_host) = open(&second);
    let _slow_video = slow.subscribe_video().unwrap();
    let fast_video = fast.subscribe_video().unwrap();
    let initial = slow.desktop_info().unwrap().quality;
    host.publish_desktop_video(
        slow_host.id,
        slow_host.generation,
        EncodedFrame::h264(true, 1, vec![31; 1000]).unwrap(),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    let mut timestamp = 2;
    while slow.desktop_info().unwrap().quality.max_frame_rate == initial.max_frame_rate {
        assert!(
            Instant::now() < until,
            "measured receiver pressure did not adapt"
        );
        host.report_desktop_producer_duration(fast_host.id, fast_host.generation, 17, 29)
            .unwrap();
        fast.report_decode_duration(1000).unwrap();
        host.publish_desktop_video(
            fast_host.id,
            fast_host.generation,
            EncodedFrame::h264(true, timestamp, vec![42; 1000]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fast_video
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .payload,
            vec![42; 1000]
        );
        timestamp += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    let changed = slow.desktop_info().unwrap().quality;
    assert!(changed.max_frame_rate < initial.max_frame_rate);
    assert_eq!(changed.max_width, initial.max_width);
    assert_eq!(changed.max_height, initial.max_height);
    assert_eq!(changed.max_bitrate_kbps, initial.max_bitrate_kbps);
    assert_eq!(
        slow.media_status().unwrap().reason,
        crate::AdaptationReason::ReceiverPressure
    );
    assert!(slow.media_status().unwrap().receiver_queue_ms >= 150);
    // Host feedback and receiver sampling have independent one-second ticks.
    // Continue the real workload until the receiver has sampled that feedback;
    // observing the slow owner's adaptation does not synchronize its sibling.
    while fast.media_status().unwrap().encode_us != 29
        || fast.media_status().unwrap().capture_us != 17
        || fast.media_status().unwrap().decode_us != 1000
    {
        assert!(Instant::now() < until, "producer feedback was not sampled");
        host.report_desktop_producer_duration(fast_host.id, fast_host.generation, 17, 29)
            .unwrap();
        fast.report_decode_duration(1000).unwrap();
        host.publish_desktop_video(
            fast_host.id,
            fast_host.generation,
            EncodedFrame::h264(true, timestamp, vec![42; 1000]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fast_video
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .payload,
            vec![42; 1000]
        );
        timestamp += 1;
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(fast.media_status().unwrap().received_fps > 0);
    assert_eq!(fast.desktop_info().unwrap().quality, initial);
    assert_eq!(fast.media_status().unwrap().encode_us, 29);
    assert_eq!(fast.media_status().unwrap().capture_us, 17);
    assert_eq!(fast.media_status().unwrap().decode_us, 1000);
    assert_eq!(slow.media_status().unwrap().encode_us, 0);
    assert!(fast.report_decode_duration(5_000_001).is_err());
    slow.close();
    assert!(slow.media_status().is_err());
    assert!(slow.report_decode_duration(1000).is_err());
    host.publish_desktop_video(
        fast_host.id,
        fast_host.generation,
        EncodedFrame::h264(true, timestamp, vec![77; 1000]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fast_video
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .payload,
        vec![77; 1000]
    );
}

/// STD05: addressed close by service id terminates only the addressed
/// Desktop owner; the sibling keeps its adoption and its stream, and
/// both parent connections stay alive.
#[test]
fn addressed_desktop_close_terminates_only_the_addressed_owner() {
    let host = start_direct();
    let first = start_direct();
    let second = start_direct();
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::new(Mutex::new(Vec::new())),
    }))
    .unwrap();
    let open = |controller: &Core| {
        pair_for_control(&host, controller);
        let pending = controller
            .begin_connect_services(&host.endpoint_id())
            .unwrap();
        wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
        let session = controller.controller_session(pending.id).unwrap();
        let owner = controller
            .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
            .unwrap();
        let desktop = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let remote = host
            .hosted_services()
            .unwrap()
            .into_iter()
            .find(|s| s.peer_id == controller.endpoint_id() && s.kind == ServiceKind::Desktop)
            .unwrap();
        host.force_desktop_stream(remote.id, remote.generation);
        (desktop, owner, remote, session)
    };
    let (slow, _slow_owner, slow_host, slow_session) = open(&first);
    let (fast, fast_owner, fast_host, fast_session) = open(&second);
    let slow_video = slow.subscribe_video().unwrap();
    let fast_video = fast.subscribe_video().unwrap();
    host.publish_desktop_video(
        slow_host.id,
        slow_host.generation,
        EncodedFrame::h264(true, 1, vec![31; 64]).unwrap(),
    )
    .unwrap();
    host.publish_desktop_video(
        fast_host.id,
        fast_host.generation,
        EncodedFrame::h264(true, 2, vec![32; 64]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        slow_video
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        1
    );
    assert_eq!(
        fast_video
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        2
    );
    // Host-side addressed close: only the addressed owner ends.
    host.close_hosted_service(slow_host.id, slow_host.generation)
        .unwrap();
    wait(|| slow.is_cancelled());
    host.publish_desktop_video(
        fast_host.id,
        fast_host.generation,
        EncodedFrame::h264(true, 3, vec![33; 64]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fast_video
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .timestamp_us,
        3,
        "sibling stream died with the addressed close"
    );
    // Controller-side addressed close: only the addressed owner ends.
    second
        .close_controller_service(fast_owner.id, fast_owner.generation)
        .unwrap();
    wait(|| fast.is_cancelled());
    // Both parents survive service-scoped closes.
    assert_eq!(
        first.controller_session(slow_session.id).unwrap().phase,
        SessionPhase::Active
    );
    assert_eq!(
        second.controller_session(fast_session.id).unwrap().phase,
        SessionPhase::Active
    );
    host.shutdown();
    first.shutdown();
    second.shutdown();
}

#[test]
fn one_connection_streams_two_selected_monitors_independently() {
    use super::support::take_matching;
    use crate::{CoreEvent, DisplayInfo, Error};
    fn screen(id: u64, x: i32, width: u16) -> DisplayInfo {
        DisplayInfo {
            id,
            origin_x: x,
            origin_y: 0,
            width,
            height: 1080,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 1,
        }
    }
    struct Capture;
    impl crate::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (1920, 1080)
        }
    }
    let host = start_direct();
    let controller = start_direct();
    host.set_capture(Box::new(Capture)).unwrap();
    pair_for_control(&host, &controller);
    // Two synthetic sources behind one host.
    host.set_topology(1, &[screen(1, 0, 1920), screen(2, 1920, 1280)])
        .unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    // A second Desktop owner on the same connection is admitted, not Busy.
    let owner_a = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop_a = controller
        .open_desktop_service(owner_a.id, owner_a.generation)
        .unwrap();
    let owner_b = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop_b = controller
        .open_desktop_service(owner_b.id, owner_b.generation)
        .unwrap();
    wait(|| {
        desktop_a.topology().ok().is_some_and(|d| d.len() == 2)
            && desktop_b.topology().ok().is_some_and(|d| d.len() == 2)
    });
    // Two display ids on one owner stay a synchronous InvalidSetting.
    assert!(matches!(
        desktop_a.select_displays(&[1, 2]),
        Err(Error::InvalidSetting(_))
    ));
    desktop_a.select_displays(&[1]).unwrap();
    desktop_b.select_displays(&[2]).unwrap();
    // Resolve each host runtime through its applied selection.
    let remotes = || {
        host.hosted_services()
            .unwrap()
            .into_iter()
            .filter(|s| s.kind == ServiceKind::Desktop)
            .collect::<Vec<_>>()
    };
    wait(|| remotes().len() == 2);
    let remote_for = |display: u64| {
        remotes()
            .into_iter()
            .find(|r| {
                host.hosted_display_selection(r.id, r.generation)
                    .ok()
                    .as_deref()
                    == Some(&[display][..])
            })
            .unwrap()
    };
    wait(|| {
        host.hosted_display_selection(remotes()[0].id, remotes()[0].generation)
            .ok()
            .is_some_and(|s| s.len() == 1)
            && host
                .hosted_display_selection(remotes()[1].id, remotes()[1].generation)
                .ok()
                .is_some_and(|s| s.len() == 1)
    });
    let remote_a = remote_for(1);
    let remote_b = remote_for(2);
    host.force_desktop_stream(remote_a.id, remote_a.generation);
    host.force_desktop_stream(remote_b.id, remote_b.generation);
    // The native encoder sees one capture target per selected source.
    let mut targets = host
        .video_targets()
        .unwrap()
        .into_iter()
        .filter(|t| t.service)
        .map(|t| t.display_id)
        .collect::<Vec<_>>();
    targets.sort();
    assert_eq!(targets, vec![1, 2]);
    assert_eq!(host.capture_displays().unwrap(), vec![1, 2]);
    // Exactly one remote-control lease exists across both owners.
    wait(|| {
        let states = [
            desktop_a.control_state().unwrap(),
            desktop_b.control_state().unwrap(),
        ];
        states
            .iter()
            .filter(|s| **s == crate::DesktopControlState::Controlling)
            .count()
            == 1
            && states
                .iter()
                .any(|s| *s == crate::DesktopControlState::Contended)
    });
    // Two simultaneous decoded streams with distinguishable content.
    let video_a = desktop_a.subscribe_video().unwrap();
    let video_b = desktop_b.subscribe_video().unwrap();
    for round in 0..3u8 {
        host.publish_desktop_video(
            remote_a.id,
            remote_a.generation,
            EncodedFrame::h264(true, u64::from(round), vec![31; 1000]).unwrap(),
        )
        .unwrap();
        host.publish_desktop_video(
            remote_b.id,
            remote_b.generation,
            EncodedFrame::h264(true, u64::from(round), vec![42; 1000]).unwrap(),
        )
        .unwrap();
        assert_eq!(
            video_a
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .payload,
            vec![31; 1000]
        );
        assert_eq!(
            video_b
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .payload,
            vec![42; 1000]
        );
    }
    // Owners beyond the bound stay Busy.
    let owner_c = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let owner_d = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    assert!(matches!(
        controller.attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop),
        Err(Error::Busy)
    ));
    controller
        .close_controller_service(owner_c.id, owner_c.generation)
        .unwrap();
    controller
        .close_controller_service(owner_d.id, owner_d.generation)
        .unwrap();
    // Removing one source spares the other owner's stream.
    host.set_topology(2, &[screen(2, 0, 1280)]).unwrap();
    assert!(host
        .hosted_display_selection(remote_a.id, remote_a.generation)
        .unwrap()
        .is_empty());
    assert_eq!(
        host.hosted_display_fault(remote_a.id, remote_a.generation)
            .unwrap(),
        Some("the selected display is gone")
    );
    assert_eq!(
        host.hosted_display_selection(remote_b.id, remote_b.generation)
            .unwrap(),
        vec![2]
    );
    assert_eq!(
        host.hosted_display_fault(remote_b.id, remote_b.generation)
            .unwrap(),
        None
    );
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        EncodedFrame::h264(true, 9, vec![43; 500]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video_b
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .payload,
        vec![43; 500]
    );
    // Stale geometry on the survivor is refused; its selection stands.
    let host_events = host.subscribe().unwrap();
    desktop_b.select_displays(&[1]).unwrap();
    assert!(matches!(
        take_matching(&host_events, |event| matches!(
            event,
            CoreEvent::InputFailed { .. }
        )),
        CoreEvent::InputFailed { .. }
    ));
    assert_eq!(
        host.hosted_display_selection(remote_b.id, remote_b.generation)
            .unwrap(),
        vec![2]
    );
    // Closing one owner spares the other's stream.
    desktop_a.close();
    wait(|| remotes().len() == 1);
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        EncodedFrame::h264(true, 10, vec![44; 500]).unwrap(),
    )
    .unwrap();
    assert_eq!(
        video_b
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .payload,
        vec![44; 500]
    );
    assert_eq!(host.capture_displays().unwrap(), vec![2]);
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
}

/// SPEC04/11/12: twin same-sized sources stream independently with exact
/// source stamps; pointer ids are exact per owner; closing or switching
/// one spares the sibling. Dimensions can never distinguish these twins,
/// so the stamps (not geometry) carry the source.
#[test]
fn twin_sources_stream_independently_with_exact_stamps() {
    use super::support::{take_matching, RecordingDesktop};
    use crate::{CoreEvent, DisplayInfo, InputEvent};
    use std::sync::{Arc, Mutex};
    fn screen(id: u64, x: i32) -> DisplayInfo {
        DisplayInfo {
            id,
            origin_x: x,
            origin_y: 0,
            width: 1920,
            height: 1080,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 5,
        }
    }
    let host = start_direct();
    let controller = start_direct();
    let applied = Arc::new(Mutex::new(Vec::new()));
    host.set_desktop(Box::new(RecordingDesktop {
        events: Arc::clone(&applied),
    }))
    .unwrap();
    pair_for_control(&host, &controller);
    // Twins: identical geometry, distinct ids.
    host.set_topology(5, &[screen(1, 0), screen(2, 1920)])
        .unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    let owner_a = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop_a = controller
        .open_desktop_service(owner_a.id, owner_a.generation)
        .unwrap();
    let owner_b = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Desktop)
        .unwrap();
    let desktop_b = controller
        .open_desktop_service(owner_b.id, owner_b.generation)
        .unwrap();
    wait(|| {
        desktop_a.topology().ok().is_some_and(|d| d.len() == 2)
            && desktop_b.topology().ok().is_some_and(|d| d.len() == 2)
    });
    desktop_a.select_displays(&[1]).unwrap();
    desktop_b.select_displays(&[2]).unwrap();
    let remotes = || {
        host.hosted_services()
            .unwrap()
            .into_iter()
            .filter(|s| s.kind == ServiceKind::Desktop)
            .collect::<Vec<_>>()
    };
    wait(|| remotes().len() == 2);
    let remote_for = |display: u64| {
        remotes()
            .into_iter()
            .find(|r| {
                host.hosted_display_selection(r.id, r.generation)
                    .ok()
                    .as_deref()
                    == Some(&[display][..])
            })
            .unwrap()
    };
    wait(|| {
        remotes().iter().all(|r| {
            host.hosted_display_selection(r.id, r.generation)
                .ok()
                .is_some_and(|s| s.len() == 1)
        })
    });
    let remote_a = remote_for(1);
    let remote_b = remote_for(2);
    host.force_desktop_stream(remote_a.id, remote_a.generation);
    host.force_desktop_stream(remote_b.id, remote_b.generation);
    // Accepted-stream metadata carries id and revision per target.
    let mut targets = host
        .video_targets()
        .unwrap()
        .into_iter()
        .filter(|t| t.service)
        .map(|t| (t.display_id, t.revision))
        .collect::<Vec<_>>();
    targets.sort();
    assert_eq!(targets, vec![(1, 5), (2, 5)]);
    // Pointer input needs the Controlling owner; contended input is
    // discarded before acceptance, so acquire explicitly.
    desktop_a.acquire_control().unwrap();
    wait(|| desktop_a.control_state().unwrap() == crate::DesktopControlState::Controlling);
    // Stamped frames route exactly, with distinct content per source.
    let video_a = desktop_a.subscribe_video().unwrap();
    let video_b = desktop_b.subscribe_video().unwrap();
    host.publish_desktop_video(
        remote_a.id,
        remote_a.generation,
        EncodedFrame::h264(true, 1, vec![31; 1000])
            .unwrap()
            .with_source(1, 5),
    )
    .unwrap();
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        EncodedFrame::h264(true, 1, vec![42; 1000])
            .unwrap()
            .with_source(2, 5),
    )
    .unwrap();
    let got_a = video_a.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(got_a.payload, vec![31; 1000]);
    assert_eq!((got_a.display_id, got_a.revision), (1, 5));
    let got_b = video_b.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(got_b.payload, vec![42; 1000]);
    assert_eq!((got_b.display_id, got_b.revision), (2, 5));
    // An unstamped frame gains the live revision but never an invented id.
    host.publish_desktop_video(
        remote_a.id,
        remote_a.generation,
        EncodedFrame::h264(true, 2, vec![33; 100]).unwrap(),
    )
    .unwrap();
    let got = video_a.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!((got.display_id, got.revision), (0, 5));
    // Exact pointer ids: the twin id on display 1 applies; display 2 on
    // the display-1 owner is refused as not selected.
    desktop_a
        .submit_input(InputEvent::pointer(100, 100, 1, 5))
        .unwrap();
    wait(|| applied.lock().unwrap().len() == 1);
    assert_eq!(
        applied.lock().unwrap().as_slice(),
        &[InputEvent::pointer(100, 100, 1, 5)]
    );
    let host_events = host.subscribe().unwrap();
    desktop_a
        .submit_input(InputEvent::pointer(2000, 100, 2, 5))
        .unwrap();
    assert!(matches!(
        take_matching(&host_events, |event| matches!(
            event,
            CoreEvent::InputFailed { .. }
        )),
        CoreEvent::InputFailed {
            message: "that display is not selected",
            ..
        }
    ));
    assert_eq!(applied.lock().unwrap().len(), 1);
    // Closing one owner spares the other's stamped stream.
    desktop_a.close();
    wait(|| remotes().len() == 1);
    assert!(host
        .publish_desktop_video(
            remote_a.id,
            remote_a.generation,
            EncodedFrame::h264(true, 3, vec![31; 100]).unwrap(),
        )
        .is_err());
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        EncodedFrame::h264(true, 3, vec![43; 500])
            .unwrap()
            .with_source(2, 5),
    )
    .unwrap();
    let survivor = video_b.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(survivor.payload, vec![43; 500]);
    assert_eq!((survivor.display_id, survivor.revision), (2, 5));
    // Switching the survivor to display 1 re-sources its stream.
    desktop_b.select_displays(&[1]).unwrap();
    wait(|| {
        host.hosted_display_selection(remote_b.id, remote_b.generation)
            .ok()
            .as_deref()
            == Some(&[1][..])
    });
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        EncodedFrame::h264(true, 4, vec![44; 500])
            .unwrap()
            .with_source(1, 5),
    )
    .unwrap();
    let switched = video_b.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(switched.payload, vec![44; 500]);
    assert_eq!((switched.display_id, switched.revision), (1, 5));
    assert_eq!(host.capture_displays().unwrap(), vec![1]);
}

/// What the host says of itself, and what this window's decoder reports,
/// become the viewer's conditions; a clean loopback adds no link strain.
#[test]
fn host_and_decoder_reports_become_the_viewers_conditions() {
    use crate::Condition;
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
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .unwrap();
    let remote = host_owner(&host);
    host.force_desktop_stream(remote.id, remote.generation);
    let video = desktop.subscribe_video().unwrap();
    host.report_desktop_encoder(
        remote.id,
        remote.generation,
        crate::EncoderImplementation {
            name: "test",
            hardware: Some(false),
        },
    )
    .unwrap();
    host.report_secure_desktop(true).unwrap();
    desktop.report_decoder(true, 0).unwrap();
    let mut timestamp = 1;
    let mut stream_until = |done: &dyn Fn(crate::ConditionSet) -> bool| {
        let until = Instant::now() + Duration::from_secs(10);
        while !done(desktop.media_status().unwrap().conditions) {
            assert!(
                Instant::now() < until,
                "the conditions did not follow the reports"
            );
            host.publish_desktop_video(
                remote.id,
                remote.generation,
                EncodedFrame::h264(true, timestamp, vec![9; 500]).unwrap(),
            )
            .unwrap();
            video.recv_timeout(Duration::from_secs(1)).unwrap();
            timestamp += 1;
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    stream_until(&|set| {
        set.contains(Condition::EncoderSoftware) && set.contains(Condition::HostSecureDesktop)
    });
    let set = desktop.media_status().unwrap().conditions;
    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [
            Condition::HostSecureDesktop,
            Condition::EncoderSoftware,
            Condition::DecoderSoftware
        ]
    );
    let signals: Vec<_> = desktop
        .conditions()
        .unwrap()
        .iter()
        .map(|entry| entry.evidence.signal)
        .collect();
    assert_eq!(
        signals,
        [
            "host_secure_desktop",
            "host_encoder_hardware",
            "decoder_hardware"
        ]
    );
    host.report_secure_desktop(false).unwrap();
    desktop.report_decoder(false, 0).unwrap();
    stream_until(&|set| set.iter().collect::<Vec<_>>() == [Condition::EncoderSoftware]);
    // The window's decoder restarted twice: it shows as recovering.
    desktop.report_decoder(false, 2).unwrap();
    stream_until(&|set| set.contains(Condition::DecoderRecovering));
    assert!(
        desktop.report_decoder(false, 1).is_err(),
        "a restart count that goes back belongs to another decoder"
    );
    desktop.close();
}

/// Each way a host ends a picture reaches the viewer as its own reason;
/// a picture this computer closed has none.
#[test]
fn the_viewer_learns_why_the_host_ended_its_picture() {
    use crate::{DesktopEnd, StreamEnd, StreamFailure};
    #[derive(Clone, Copy, Debug)]
    enum HostAction {
        Close,
        Fail(StreamFailure),
        StopSharing,
        ViewerCloses,
    }
    for (action, expected) in [
        (
            HostAction::Close,
            Some(DesktopEnd::Host(StreamEnd::Stopped)),
        ),
        (
            HostAction::Fail(StreamFailure::Encoder),
            Some(DesktopEnd::Host(StreamEnd::Failed(StreamFailure::Encoder))),
        ),
        (
            HostAction::Fail(StreamFailure::Permission),
            Some(DesktopEnd::Host(StreamEnd::Failed(
                StreamFailure::Permission,
            ))),
        ),
        (
            HostAction::StopSharing,
            Some(DesktopEnd::Host(StreamEnd::Revoked)),
        ),
        (HostAction::ViewerCloses, None),
    ] {
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
        let desktop = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        let remote = host_owner(&host);
        assert_eq!(desktop.end(), None, "a live picture has no end");
        match action {
            HostAction::Close => host
                .close_hosted_service(remote.id, remote.generation)
                .unwrap(),
            HostAction::Fail(failure) => host
                .fail_hosted_service(remote.id, remote.generation, failure)
                .unwrap(),
            HostAction::StopSharing => host
                .set_service_sharing(ServiceKind::Desktop, false)
                .unwrap(),
            HostAction::ViewerCloses => desktop.close(),
        }
        wait(|| desktop.is_cancelled());
        if expected.is_some() {
            wait(|| desktop.end().is_some());
        } else {
            std::thread::sleep(Duration::from_millis(300));
        }
        assert_eq!(desktop.end(), expected, "{action:?}");
    }
}

#[test]
fn a_stream_end_is_one_known_byte() {
    use crate::{StreamEnd, StreamFailure};
    for end in [
        StreamEnd::Failed(StreamFailure::Encoder),
        StreamEnd::Failed(StreamFailure::Capture),
        StreamEnd::Failed(StreamFailure::Permission),
        StreamEnd::Failed(StreamFailure::DisplayGone),
        StreamEnd::Stopped,
        StreamEnd::Shutdown,
        StreamEnd::Revoked,
    ] {
        assert_eq!(StreamEnd::decode(end.encode()).unwrap(), end);
    }
    for unknown in [0, 8, 255] {
        assert!(matches!(
            StreamEnd::decode(unknown),
            Err(crate::Error::Malformed)
        ));
    }
    assert_eq!(StreamEnd::for_close(crate::CloseCause::PeerClose), None);
    assert_eq!(
        StreamEnd::for_close(crate::CloseCause::DeviceLoss),
        Some(StreamEnd::Failed(StreamFailure::Capture))
    );
}
