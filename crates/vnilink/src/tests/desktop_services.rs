use super::filesystem_location::FilesystemLocation;
use super::support::{pair_for_control, start_direct, RecordingDesktop};
use crate::{Core, EncodedFrame, ServiceKind, SessionPhase};
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
