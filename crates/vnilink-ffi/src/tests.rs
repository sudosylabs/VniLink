use crate::{new_quality, FfiKeyboardMode, FfiPointerButton, FfiSessionRole, VnilinkCore};
use crate::{FfiError, FfiEvent};

fn open_core() -> VnilinkCore {
    VnilinkCore::create(true, vec![]).expect("public-relay core starts")
}

#[test]
fn relay_selection_is_validated() {
    assert!(matches!(
        VnilinkCore::create(false, vec![]),
        Err(FfiError::InvalidRelay { .. })
    ));
    assert!(matches!(
        VnilinkCore::create(false, vec!["not a url".to_string()]),
        Err(FfiError::InvalidRelayUrl { .. })
    ));
    let core = open_core();
    assert!(core.snapshot().unwrap().public_relay);
    assert!(core.set_relay_custom(vec![]).is_err());
}

#[test]
fn snapshot_capabilities_and_hosting_lifecycle() {
    let core = open_core();
    assert!(!core.capabilities().h264_decode);
    core.set_h264_support(true, true).unwrap();
    assert!(core.capabilities().h264_decode);
    // Encode also needs a capture adapter, which foreign callers install
    // from platform Rust: without one it stays false.
    assert!(!core.capabilities().h264_encode);
    assert!(!core.snapshot().unwrap().hosting);
    core.start_hosting().unwrap();
    assert!(core.snapshot().unwrap().hosting);
    core.stop_hosting().unwrap();
    assert!(!core.snapshot().unwrap().hosting);
    assert!(!core.endpoint_id().is_empty());
}

#[test]
fn quality_constructor_enforces_the_limits() {
    new_quality(2_000, 30, 1920, 1080).unwrap();
    assert!(new_quality(0, 30, 1920, 1080).is_err());
    assert!(new_quality(2_000, 0, 1920, 1080).is_err());
    assert!(new_quality(2_000, 30, 0, 1080).is_err());
}

#[test]
fn pairing_handles_and_errors() {
    let writer = open_core();
    let reader = open_core();
    let file = writer.export_pairing_file(900, true).unwrap();
    assert!(!file.bytes.is_empty());
    assert_eq!(file.check.len(), 3);
    let preview = reader.open_pairing_file(file.bytes.clone()).unwrap();
    assert_eq!(preview.check, file.check);
    // Wrong digits keep the preview server-side; the token never crossed.
    assert!(matches!(
        reader.confirm_pairing(preview.id, "000".to_string()),
        Err(FfiError::PairingCheckMismatch)
    ));
    assert!(reader.open_pairing_file(vec![0, 1, 2]).is_err());
    assert!(matches!(
        reader.confirm_pairing(999_999, "042".to_string()),
        Err(FfiError::InvalidPairingFile)
    ));
    assert!(matches!(
        reader.accept_invitation("vnl1.zz".to_string()),
        Err(FfiError::InvalidInvitation { .. })
    ));
    assert!(reader.control_request().unwrap().is_none());
    assert_eq!(reader.control_check().len(), 3);
    assert!(matches!(
        reader.confirm_control_request("042".to_string()),
        Err(FfiError::NoControlRequest)
    ));
    assert!(matches!(
        reader.reject_control_request(),
        Err(FfiError::NoControlRequest)
    ));
}

#[test]
fn unpaired_peers_fail_before_dialing() {
    let core = open_core();
    let other = open_core();
    assert!(matches!(
        core.connect(other.endpoint_id()),
        Err(FfiError::NotPaired)
    ));
    assert!(matches!(core.disconnect(), Err(FfiError::NoSession)));
    assert!(matches!(
        core.authorize(other.endpoint_id(), true, false, false),
        Err(FfiError::NotPaired)
    ));
    assert!(matches!(
        core.revoke(other.endpoint_id()),
        Err(FfiError::NotPaired)
    ));
    assert!(matches!(
        core.unpair(other.endpoint_id()),
        Err(FfiError::NotPaired)
    ));
}

#[test]
fn input_clipboard_transfer_and_video_fail_without_a_session() {
    let core = open_core();
    assert!(matches!(
        core.submit_key(1, 1, 0x04, true),
        Err(FfiError::NoSession) | Err(FfiError::StaleSession)
    ));
    assert!(core.submit_pointer(1, 1, 10, 10).is_err());
    assert!(core
        .submit_button(1, 1, FfiPointerButton::Left, true)
        .is_err());
    assert!(core.submit_wheel(1, 1, 0, -1).is_err());
    assert!(core.submit_text(1, 1, "hi".to_string()).is_err());
    assert!(core
        .submit_local_key(1, 1, 0x04, true, "a".to_string())
        .is_err());
    assert!(core.submit_clipboard(1, 1, "hi".to_string()).is_err());
    assert!(core.clipboard_text().unwrap().is_none());
    assert!(core
        .submit_file(1, 1, "a.txt".to_string(), vec![1], false)
        .is_err());
    assert!(core
        .publish_file(1, "a.txt".to_string(), vec![1], false)
        .is_err());
    assert!(core
        .publish_video(1, true, 0, vec![0, 0, 0, 1, 0x65, 0x00])
        .is_err());
    assert!(core
        .update_session_quality(
            1,
            FfiSessionRole::Controller,
            new_quality(2_000, 30, 1920, 1080).unwrap()
        )
        .is_err());
    assert!(core.session_paths(1).is_err());
}

#[test]
fn subscriptions_poll_empty_and_report_lag() {
    let core = open_core();
    let events = core.subscribe_events().unwrap();
    assert!(events.poll(16).is_empty());
    assert_eq!(events.lagged(), 0);
    let video = core.subscribe_video().unwrap();
    assert!(video.try_recv().is_none());
    assert!(video.recv_timeout(10).unwrap().is_none());
    assert_eq!(video.lagged(), 0);
    // Second video subscription is busy; events allow several subscribers.
    assert!(core.subscribe_video().is_err());
    let _ = core.subscribe_events().unwrap();
}

#[test]
fn keyboard_mode_and_swap_round_trip() {
    let core = open_core();
    assert!(matches!(
        core.keyboard_mode().unwrap(),
        crate::FfiKeyboardMode::Character
    ));
    core.set_keyboard_mode(FfiKeyboardMode::Map).unwrap();
    assert!(matches!(
        core.keyboard_mode().unwrap(),
        crate::FfiKeyboardMode::Map
    ));
    assert!(!core.swap_control_and_command().unwrap());
    core.set_swap_control_and_command(true).unwrap();
    assert!(core.swap_control_and_command().unwrap());
}

#[test]
fn transfer_directory_and_events_flow() {
    let core = open_core();
    let root = std::env::temp_dir().join(format!("vnl-ffi-{}", std::process::id()));
    core.set_transfer_directory(root.to_string_lossy().to_string())
        .unwrap();
    assert!(core.snapshot().unwrap().capabilities.file_transfer);
    let events = core.subscribe_events().unwrap();
    core.start_hosting().unwrap();
    let hosting = events.poll(16);
    assert!(hosting
        .iter()
        .any(|event| matches!(event, FfiEvent::HostingChanged { hosting: true })));
    core.shutdown();
}

#[test]
fn local_names_are_exposed_in_snapshots_and_can_be_cleared() {
    let core = open_core();
    let other = open_core();
    let token = other.create_invitation(60).unwrap();
    let device = core.accept_invitation(token).unwrap();
    core.set_peer_pet_name(device.endpoint_id.clone(), Some("My laptop".into()))
        .unwrap();
    let named = core
        .snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|d| d.endpoint_id == device.endpoint_id)
        .unwrap();
    assert_eq!(named.pet_name.as_deref(), Some("My laptop"));
    assert_eq!(
        named.hostname,
        core.peer_hostname(device.endpoint_id.clone()).unwrap()
    );
    assert_eq!(named.grants, device.grants);
    core.set_peer_pet_name(device.endpoint_id.clone(), None)
        .unwrap();
    assert!(core.snapshot().unwrap().devices[0].pet_name.is_none());
    core.unpair(device.endpoint_id.clone()).unwrap();
    assert!(matches!(
        core.set_peer_pet_name(device.endpoint_id, Some("gone".into())),
        Err(FfiError::NotPaired)
    ));
}
