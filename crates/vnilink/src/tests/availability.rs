use super::support::{pair_for_control, start_direct};
use crate::availability::{AvailabilityReason, BackendState, Capability};
use crate::{ServiceKind, SystemPolicy};

/// Needs loopback cores, so it runs with the approved test environment,
/// not the restricted sandbox. Wires grants, policies and installed
/// backends through the query a native caller uses.
#[test]
fn core_query_combines_backends_policies_and_grants() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    let peer = controller.endpoint_id();
    // Local view skips grants: the Files engine is always present.
    let local = host
        .capability_availability(None, Capability::Files)
        .unwrap();
    assert!(local.available);
    // pair_for_control grants view/control only: Files needs Transfer.
    let files = host
        .capability_availability(Some(&peer), Capability::Files)
        .unwrap();
    assert!(!files.available);
    assert_eq!(files.reasons, [AvailabilityReason::DeniedPeerGrant]);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    assert!(
        host.capability_availability(Some(&peer), Capability::Files)
            .unwrap()
            .available
    );
    // No factories installed: implementation and grant both block Terminal.
    let terminal = host
        .capability_availability(Some(&peer), Capability::Terminal)
        .unwrap();
    assert_eq!(
        terminal.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ]
    );
    // Default system policy allows lock but denies restart; neither has a
    // backend or a grant here.
    let lock = host
        .capability_availability(Some(&peer), Capability::SystemLock)
        .unwrap();
    assert_eq!(
        lock.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ]
    );
    let restart = host
        .capability_availability(Some(&peer), Capability::SystemRestart)
        .unwrap();
    assert_eq!(
        restart.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DisabledLocalPolicy,
            AvailabilityReason::DeniedPeerGrant,
        ]
    );
    host.set_system_policy(SystemPolicy {
        allow_restart: true,
        ..SystemPolicy::default()
    })
    .unwrap();
    let restart = host
        .capability_availability(Some(&peer), Capability::SystemRestart)
        .unwrap();
    assert_eq!(
        restart.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ]
    );
    // Recording is implemented; only consent blocks it.
    let recording = host
        .capability_availability(Some(&peer), Capability::Recording)
        .unwrap();
    assert_eq!(recording.reasons, [AvailabilityReason::DeniedPeerGrant]);
    host.set_recording_sharing(true).unwrap();
    assert!(
        host.capability_availability(Some(&peer), Capability::Recording)
            .unwrap()
            .available
    );
    // View is granted but no capture backend is installed on a bare core.
    let view = host
        .capability_availability(Some(&peer), Capability::DesktopView)
        .unwrap();
    assert_eq!(view.reasons, [AvailabilityReason::MissingImplementation]);
    let stranger = start_direct();
    assert!(host
        .capability_availability(Some(&stranger.endpoint_id()), Capability::Files)
        .is_err());
}

/// Needs loopback cores like the query test above. Published adapter facts
/// flow into the query in reason order; clearing restores unknown; an
/// overlong failure detail truncates at publish, not at read.
#[test]
fn published_backend_state_flows_into_the_query_and_clears() {
    let host = start_direct();
    let before = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert_eq!(before.reasons, [AvailabilityReason::MissingImplementation]);
    host.report_backend_state(
        Capability::Camera,
        BackendState {
            os_denied: true,
            device_missing: true,
            failed: Some("consent prompt refused".into()),
        },
    )
    .unwrap();
    let blocked = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert!(!blocked.available);
    assert_eq!(
        blocked.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedOsPermission,
            AvailabilityReason::UnavailableDevice,
            AvailabilityReason::BackendFailed {
                detail: "consent prompt refused".into(),
            },
        ]
    );
    host.report_os_permission(Capability::Camera, false)
        .unwrap();
    let consent_updated = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert!(!consent_updated
        .reasons
        .contains(&AvailabilityReason::DeniedOsPermission));
    assert!(consent_updated
        .reasons
        .contains(&AvailabilityReason::UnavailableDevice));
    assert!(consent_updated
        .reasons
        .iter()
        .any(|reason| matches!(reason, AvailabilityReason::BackendFailed { .. })));
    host.report_backend_failure(Capability::Camera, None)
        .unwrap();
    let failure_cleared = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert!(failure_cleared
        .reasons
        .contains(&AvailabilityReason::UnavailableDevice));
    assert!(!failure_cleared
        .reasons
        .iter()
        .any(|reason| matches!(reason, AvailabilityReason::BackendFailed { .. })));
    host.clear_backend_state(Capability::Camera).unwrap();
    let after = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    assert_eq!(after.reasons, [AvailabilityReason::MissingImplementation]);
    host.report_backend_state(
        Capability::Camera,
        BackendState {
            failed: Some("é".repeat(300)),
            ..BackendState::default()
        },
    )
    .unwrap();
    let truncated = host
        .capability_availability(None, Capability::Camera)
        .unwrap();
    let Some(AvailabilityReason::BackendFailed { detail }) = truncated.reasons.last() else {
        panic!(
            "expected a trailing backend failure, got {:?}",
            truncated.reasons
        );
    };
    assert!(detail.len() <= 256);
    assert!(detail.chars().all(|c| c == 'é'));
}

#[test]
fn codec_flags_and_input_geometry_do_not_claim_capture_or_clipboard_implementation() {
    struct Input;
    impl crate::HostDesktop for Input {
        fn monitor(&self) -> (u16, u16) {
            (64, 64)
        }
        fn apply(&mut self, _: crate::InputEvent) -> Result<(), crate::Error> {
            Ok(())
        }
    }
    struct Clipboard;
    impl crate::HostClipboard for Clipboard {
        fn set_text(&mut self, _: &str) -> Result<(), crate::Error> {
            Ok(())
        }
    }
    let core = start_direct();
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(64, 64, "test"))
        .unwrap();
    core.set_desktop(Box::new(Input)).unwrap();
    assert!(
        !core
            .capability_availability(None, Capability::DesktopView)
            .unwrap()
            .available
    );
    core.set_capture(Box::new(super::support::ShutdownCapture))
        .unwrap();
    assert!(
        core.capability_availability(None, Capability::DesktopView)
            .unwrap()
            .available
    );
    assert!(
        !core
            .capability_availability(None, Capability::Clipboard)
            .unwrap()
            .available
    );
    core.set_clipboard(Box::new(Clipboard)).unwrap();
    assert!(
        core.capability_availability(None, Capability::Clipboard)
            .unwrap()
            .available
    );
}

#[test]
fn a_host_without_screen_consent_tells_peers_it_needs_permission() {
    let core = start_direct();
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(64, 64, "test"))
        .unwrap();
    // No capture could start: on its own that reads as not implemented.
    let missing = core
        .capability_availability(None, Capability::DesktopView)
        .unwrap();
    assert_eq!(missing.remote_status(), crate::RemoteStatus::Unsupported);
    // The owner has not granted screen capture: that is what a peer is told.
    core.report_os_permission(Capability::DesktopView, true)
        .unwrap();
    let denied = core
        .capability_availability(None, Capability::DesktopView)
        .unwrap();
    assert_eq!(denied.remote_status(), crate::RemoteStatus::NeedsPermission);
}
