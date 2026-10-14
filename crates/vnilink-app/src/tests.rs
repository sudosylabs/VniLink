use crate::{new_quality, FfiKeyboardMode, FfiPointerButton, FfiSessionRole, VnilinkCore};
use crate::{FfiError, FfiEvent, FfiVideoCodecCaps};

fn full_codecs() -> FfiVideoCodecCaps {
    vnilink::VideoCodecCaps::h264_full(4096, 4096, "test").into()
}

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
    core.set_video_codecs(full_codecs()).unwrap();
    assert!(core.capabilities().h264_decode);
    // Encode also needs a capture adapter, which foreign callers install
    // from platform Rust: without one it stays false.
    assert!(!core.capabilities().h264_encode);
    assert!(!core.snapshot().unwrap().hosting);
    core.start_hosting().unwrap();
    assert!(core.snapshot().unwrap().hosting);
    assert!(core.hosted_sessions().unwrap().is_empty());
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
    // Check a guaranteed mismatch; the preview token never crosses the FFI.
    let wrong_check = if file.check == "000" { "001" } else { "000" };
    assert!(matches!(
        reader.confirm_pairing(preview.id, wrong_check.to_string()),
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
        core.authorize(other.endpoint_id(), true, true, false, false),
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
    assert!(core.submit_pointer(1, 1, 10, 10, 1, 1).is_err());
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
    assert!(core.fs_list(1, 1, 0, "".to_string(), 0, 20).is_err());
    assert!(core
        .job_submit_upload("bogus".to_string(), 0, "a.txt".into(), 0, "a.txt".into())
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
fn independent_controller_handles_cross_the_ffi() {
    use crate::FfiSessionPhase;
    use std::time::{Duration, Instant};
    struct Capture;
    impl vnilink::HostCapture for Capture {
        fn monitor(&self) -> (u16, u16) {
            (640, 480)
        }
    }
    let make = || VnilinkCore {
        inner: std::sync::Arc::new(
            vnilink::Core::start(vnilink::CoreConfig {
                listen: vnilink::Listen::Loopback,
                relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                    "http://127.0.0.1:9",
                )
                .unwrap()])
                .unwrap(),
                ..vnilink::CoreConfig::default()
            })
            .unwrap(),
        ),
        previews: std::sync::Mutex::new(std::collections::HashMap::new()),
        next_preview: std::sync::atomic::AtomicU64::new(1),
        audio_coordinator: std::sync::Arc::new(crate::audio_native::AudioCoordinator::default()),
    };
    let controller = make();
    let a = make();
    let b = make();
    controller.set_video_codecs(full_codecs()).unwrap();
    for host in [&a, &b] {
        host.inner.set_capture(Box::new(Capture)).unwrap();
        host.set_video_codecs(full_codecs()).unwrap();
        host.start_hosting().unwrap();
        controller
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(controller.create_invitation(60).unwrap())
            .unwrap();
        host.authorize(controller.endpoint_id(), true, true, false, false)
            .unwrap();
    }
    let pending_a = controller.begin_connect(a.endpoint_id()).unwrap();
    let pending_b = controller.begin_connect(b.endpoint_id()).unwrap();
    assert_ne!(pending_a.id, pending_b.id);
    let wait = |id, phase| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let session = controller.controller_session(id).unwrap();
            if session.phase == phase {
                break session;
            }
            assert!(
                Instant::now() < deadline,
                "FFI session timed out: {session:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    let sa = wait(pending_a.id, FfiSessionPhase::Active);
    let sb = wait(pending_b.id, FfiSessionPhase::Active);
    assert_eq!(controller.controller_sessions().unwrap().len(), 2);
    assert!(controller.disconnect().is_err());
    let va = controller
        .subscribe_session_video(sa.id, sa.generation)
        .unwrap();
    let vb = controller
        .subscribe_session_video(sb.id, sb.generation)
        .unwrap();
    a.publish_video(
        a.snapshot().unwrap().incoming.unwrap().id,
        true,
        11,
        vec![1],
    )
    .unwrap();
    b.publish_video(
        b.snapshot().unwrap().incoming.unwrap().id,
        true,
        22,
        vec![2],
    )
    .unwrap();
    assert_eq!(va.recv_timeout(3000).unwrap().unwrap().payload, vec![1]);
    assert_eq!(vb.recv_timeout(3000).unwrap().unwrap().payload, vec![2]);
    controller.disconnect_session(sa.id).unwrap();
    wait(sa.id, FfiSessionPhase::Closed);
    controller.forget_controller_session(sa.id).unwrap();
    assert!(controller.controller_session(sa.id).is_err());
    assert_eq!(controller.controller_session(sb.id).unwrap(), sb);
    controller.shutdown();
    a.shutdown();
    b.shutdown();
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
fn file_roots_advertise_transfer_and_events_flow() {
    let core = open_core();
    let root = std::env::temp_dir().join(format!("vnl-ffi-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    core.set_file_roots(vec![root.to_string_lossy().into_owned()])
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

#[derive(Clone, Default)]
struct TestCredentialStore(std::sync::Arc<std::sync::Mutex<Option<Vec<u8>>>>);
impl crate::CredentialStore for TestCredentialStore {
    fn load(&self) -> Result<Option<Vec<u8>>, FfiError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, record: Vec<u8>) -> Result<(), FfiError> {
        *self.0.lock().unwrap() = Some(record);
        Ok(())
    }
}
#[test]
fn native_store_keeps_identity_names_and_opaque_relay_credentials() {
    let store = TestCredentialStore::default();
    let core = VnilinkCore::open_persistent(Box::new(store.clone())).unwrap();
    let id = core.endpoint_id();
    let peer = open_core();
    let device = core
        .accept_invitation(peer.create_invitation(60).unwrap())
        .unwrap();
    core.set_peer_pet_name(device.endpoint_id.clone(), Some("Work Mac".into()))
        .unwrap();
    core.save_custom_relays(vec![crate::FfiRelayInput {
        url: "https://relay.example/".into(),
        authenticated: true,
        replacement_token: "test-token".into(),
    }])
    .unwrap();
    let relays = core.saved_relays().unwrap();
    assert!(relays[0].authenticated);
    assert!(!format!("{relays:?}").contains("test-token"));
    core.save_custom_relays(vec![crate::FfiRelayInput {
        url: relays[0].url.clone(),
        authenticated: true,
        replacement_token: String::new(),
    }])
    .unwrap();
    core.shutdown();
    let reopened = VnilinkCore::open_persistent(Box::new(store)).unwrap();
    assert_eq!(reopened.endpoint_id(), id);
    assert_eq!(
        reopened.snapshot().unwrap().devices[0].pet_name.as_deref(),
        Some("Work Mac")
    );
    assert!(reopened.saved_relays().unwrap()[0].authenticated);
    reopened
        .save_custom_relays(vec![crate::FfiRelayInput {
            url: relays[0].url.clone(),
            authenticated: false,
            replacement_token: String::new(),
        }])
        .unwrap();
    assert!(!reopened.saved_relays().unwrap()[0].authenticated);
    reopened.shutdown();
}

#[test]
fn file_roots_and_local_operations_cross_the_ffi() {
    use crate::{FfiConflictChoice, FfiEntryKind, FfiJobState};

    let dir = std::env::temp_dir().join(format!("vnilink-app-files-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("root")).unwrap();
    std::fs::write(dir.join("root/notes.txt"), b"ffi-bytes").unwrap();
    let core = open_core();
    core.set_blob_directory(dir.join("blobs").to_string_lossy().into_owned())
        .unwrap();
    assert!(core.file_roots().unwrap().is_empty());
    assert!(core.list_dir(0, "".into(), 0, 50).is_err());
    core.set_file_roots(vec![dir.join("root").to_string_lossy().into_owned()])
        .unwrap();
    let roots = core.file_roots().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].id, 0);

    let page = core.list_dir(0, "".into(), 0, 50).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.entries[0].name, "notes.txt");
    assert_eq!(page.entries[0].kind, FfiEntryKind::File);
    assert_eq!(page.entries[0].size, 9);
    assert!(!format!("{:?}", page).contains("notes"));

    core.create_dir(0, "".into(), "docs".into()).unwrap();
    let token = page.entries[0].token;
    core.rename_entry(
        0,
        "".into(),
        "notes.txt".into(),
        Some(token),
        "renamed.txt".into(),
    )
    .unwrap();
    assert_eq!(
        std::fs::read(dir.join("root/renamed.txt")).unwrap(),
        b"ffi-bytes"
    );

    // A stale token is refused without touching the replacement (remove
    // plus recreate makes a new object; truncate would keep the inode).
    std::fs::rename(dir.join("root/renamed.txt"), dir.join("root/previous.txt")).unwrap();
    std::fs::write(dir.join("root/renamed.txt"), b"changed").unwrap();
    let stale = core.rename_entry(
        0,
        "".into(),
        "renamed.txt".into(),
        Some(token),
        "x.txt".into(),
    );
    assert!(matches!(stale, Err(FfiError::InvalidSetting { .. })));
    assert!(!dir.join("root/x.txt").exists());

    // A no-session job submit maps cleanly; FFI stays usable afterwards.
    // Pair first so the failure is the missing session, not pairing.
    let peer = open_core();
    let token = core.create_invitation(60).unwrap();
    peer.accept_invitation(token).unwrap();
    let back = peer.create_invitation(60).unwrap();
    core.accept_invitation(back).unwrap();
    assert!(matches!(
        core.job_submit_upload(peer.endpoint_id(), 0, "".into(), 0, "x".into()),
        Err(FfiError::NoSession)
    ));
    assert!(core.job_list().unwrap().is_empty());
    assert!(matches!(
        core.job_snapshot(7),
        Err(FfiError::InvalidSetting { .. })
    ));
    let _ = FfiConflictChoice::KeepBoth;
    let _ = FfiJobState::Complete;
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn power_policy_roundtrip_and_registration_do_not_claim_active_inhibition() {
    use crate::{FfiPowerPhase, FfiPowerPolicy};
    let core = open_core();
    let policy = FfiPowerPolicy {
        incoming_display: true,
        incoming_system: false,
        outgoing_display: false,
        outgoing_system: true,
        incoming_idle_secs: 73,
        outgoing_idle_secs: 94,
        lock_after_control: true,
    };
    core.set_power_policy(policy).unwrap();
    assert_eq!(core.power_policy().unwrap(), policy);
    assert!(matches!(
        core.set_power_policy(FfiPowerPolicy {
            incoming_idle_secs: u32::MAX,
            ..policy
        }),
        Err(FfiError::InvalidSetting { .. })
    ));
    assert_eq!(core.power_policy().unwrap(), policy);
    core.enable_native_power(true).unwrap();
    assert_eq!(core.power_status().unwrap().len(), 2);
    assert!(core
        .power_status()
        .unwrap()
        .iter()
        .all(|s| s.owners == 0 && s.phase == FfiPowerPhase::Idle));
    core.enable_native_power(false).unwrap();
}

#[test]
fn backend_state_publish_flows_into_availability_and_clears() {
    use crate::{FfiAvailabilityReason, FfiBackendState, FfiCapability};
    let core = open_core();
    core.report_backend_state(
        FfiCapability::Camera,
        FfiBackendState {
            os_denied: true,
            device_missing: false,
            failed: None,
        },
    )
    .unwrap();
    let blocked = core
        .capability_availability(None, FfiCapability::Camera)
        .unwrap();
    assert!(!blocked.available);
    assert!(blocked
        .reasons
        .contains(&FfiAvailabilityReason::DeniedOsPermission));
    core.clear_backend_state(FfiCapability::Camera).unwrap();
    let cleared = core
        .capability_availability(None, FfiCapability::Camera)
        .unwrap();
    assert!(!cleared
        .reasons
        .contains(&FfiAvailabilityReason::DeniedOsPermission));
}
