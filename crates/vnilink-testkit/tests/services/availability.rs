//! Whether a capability is available, as an engine answers it: its
//! backends, the host's policies, the grants a peer holds, and what the
//! platform's adapters report.

use vnilink::{
    AvailabilityReason, BackendState, Capability, Error, RemoteStatus, ServiceKind, SystemPolicy,
    VideoCodecCaps,
};
use vnilink_testkit::desktop::{InputLog, MonitorCapture, RecordingClipboard};
use vnilink_testkit::{traced, Engine, Pair, TestResult};

/// The reasons `capability` is unavailable on `engine`, to `peer` or to its
/// own computer.
fn reasons(
    engine: &Engine,
    peer: Option<&str>,
    capability: Capability,
) -> Result<Vec<AvailabilityReason>, Error> {
    Ok(engine
        .core
        .capability_availability(peer, capability)?
        .reasons)
}

/// The answer combines what is installed, the host's policy and the
/// peer's grants, in that order; a computer that is not paired gets no
/// answer.
#[test]
fn the_answer_combines_backends_policies_and_grants() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host;
    let peer = pair.controller.endpoint();
    let peer = Some(peer.as_str());
    // Asked locally, grants do not apply: the Files engine is always present.
    assert!(
        host.core
            .capability_availability(None, Capability::Files)?
            .available,
        "Files is available locally"
    );
    // The pair grants view and control only: Files needs Transfer.
    let files = host.core.capability_availability(peer, Capability::Files)?;
    assert!(!files.available, "Files needs its grant");
    assert_eq!(
        files.reasons,
        [AvailabilityReason::DeniedPeerGrant],
        "only the grant blocks Files"
    );
    host.core.set_service_sharing(ServiceKind::Files, true)?;
    assert!(
        host.core
            .capability_availability(peer, Capability::Files)?
            .available,
        "sharing Files makes it available"
    );
    // No backends installed: implementation and grant both block Terminal.
    assert_eq!(
        reasons(host, peer, Capability::Terminal)?,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ],
        "Terminal lacks a backend and a grant"
    );
    // The default system policy allows lock but denies restart; neither has
    // a backend or a grant here.
    assert_eq!(
        reasons(host, peer, Capability::SystemLock)?,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ],
        "the default policy allows lock"
    );
    assert_eq!(
        reasons(host, peer, Capability::SystemRestart)?,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DisabledLocalPolicy,
            AvailabilityReason::DeniedPeerGrant,
        ],
        "the default policy denies restart"
    );
    host.core.set_system_policy(SystemPolicy {
        allow_restart: true,
        ..SystemPolicy::default()
    })?;
    assert_eq!(
        reasons(host, peer, Capability::SystemRestart)?,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedPeerGrant,
        ],
        "a policy allowing restart lifts its block"
    );
    // Recording is implemented; only consent blocks it.
    assert_eq!(
        reasons(host, peer, Capability::Recording)?,
        [AvailabilityReason::DeniedPeerGrant],
        "only consent blocks recording"
    );
    host.core.set_recording_sharing(true)?;
    assert!(
        host.core
            .capability_availability(peer, Capability::Recording)?
            .available,
        "consenting makes recording available"
    );
    // View is granted, but no capture backend is installed.
    assert_eq!(
        reasons(host, peer, Capability::DesktopView)?,
        [AvailabilityReason::MissingImplementation],
        "view lacks only its capture"
    );
    let stranger = Engine::start("stranger");
    assert!(
        host.core
            .capability_availability(Some(&stranger.endpoint()), Capability::Files)
            .is_err(),
        "a computer that is not paired gets no answer"
    );
    Ok(())
}

/// What the platform's adapters report joins the answer in reason order;
/// each report changes only its own fact, clearing restores the unknown,
/// and an overlong failure is bounded when reported.
#[test]
fn reported_backend_state_joins_the_answer_and_clears() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let core = &host.core;
    assert_eq!(
        reasons(&host, None, Capability::Camera)?,
        [AvailabilityReason::MissingImplementation],
        "a camera without a backend is missing"
    );
    core.report_backend_state(
        Capability::Camera,
        BackendState {
            os_denied: true,
            device_missing: true,
            failed: Some("consent prompt refused".into()),
        },
    )?;
    let blocked = core.capability_availability(None, Capability::Camera)?;
    assert!(!blocked.available, "a reported block makes it unavailable");
    assert_eq!(
        blocked.reasons,
        [
            AvailabilityReason::MissingImplementation,
            AvailabilityReason::DeniedOsPermission,
            AvailabilityReason::UnavailableDevice,
            AvailabilityReason::BackendFailed {
                detail: "consent prompt refused".into(),
            },
        ],
        "each reported fact, in reason order"
    );
    core.report_os_permission(Capability::Camera, false)?;
    let consented = reasons(&host, None, Capability::Camera)?;
    assert!(
        !consented.contains(&AvailabilityReason::DeniedOsPermission),
        "a granted permission lifts its block: {consented:?}"
    );
    assert!(
        consented.contains(&AvailabilityReason::UnavailableDevice),
        "the permission leaves the device: {consented:?}"
    );
    assert!(
        consented
            .iter()
            .any(|reason| matches!(reason, AvailabilityReason::BackendFailed { .. })),
        "the permission leaves the failure: {consented:?}"
    );
    core.report_backend_failure(Capability::Camera, None)?;
    let recovered = reasons(&host, None, Capability::Camera)?;
    assert!(
        recovered.contains(&AvailabilityReason::UnavailableDevice),
        "clearing the failure leaves the device: {recovered:?}"
    );
    assert!(
        !recovered
            .iter()
            .any(|reason| matches!(reason, AvailabilityReason::BackendFailed { .. })),
        "clearing the failure lifts it: {recovered:?}"
    );
    core.clear_backend_state(Capability::Camera)?;
    assert_eq!(
        reasons(&host, None, Capability::Camera)?,
        [AvailabilityReason::MissingImplementation],
        "clearing restores the unknown"
    );
    core.report_backend_state(
        Capability::Camera,
        BackendState {
            failed: Some("é".repeat(300)),
            ..BackendState::default()
        },
    )?;
    let truncated = reasons(&host, None, Capability::Camera)?;
    let Some(AvailabilityReason::BackendFailed { detail }) = truncated.last() else {
        panic!("expected a trailing backend failure, got {truncated:?}");
    };
    assert!(detail.len() <= 256, "a failure is bounded when reported");
    assert!(
        detail.chars().all(|c| c == 'é'),
        "a failure is bounded on a character boundary"
    );
    Ok(())
}

/// Codecs and an input desktop do not make view available, nor clipboard;
/// a capture makes view available and a clipboard makes clipboard so.
#[test]
fn codecs_and_input_do_not_claim_capture_or_clipboard() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let core = &host.core;
    core.set_video_codecs(VideoCodecCaps::h264_full(64, 64, "test"))?;
    InputLog::install(&host, (64, 64))?;
    assert!(
        !core
            .capability_availability(None, Capability::DesktopView)?
            .available,
        "codecs and input do not make view available"
    );
    core.set_capture(Box::new(MonitorCapture::new((1, 1))))?;
    assert!(
        core.capability_availability(None, Capability::DesktopView)?
            .available,
        "a capture makes view available"
    );
    assert!(
        !core
            .capability_availability(None, Capability::Clipboard)?
            .available,
        "a desktop does not make the clipboard available"
    );
    core.set_clipboard(Box::new(RecordingClipboard {
        texts: Default::default(),
    }))?;
    assert!(
        core.capability_availability(None, Capability::Clipboard)?
            .available,
        "a clipboard makes the clipboard available"
    );
    Ok(())
}

/// A host whose owner has not allowed screen capture tells peers it needs
/// permission, not that it cannot.
#[test]
fn a_host_without_screen_consent_tells_peers_it_needs_permission() -> TestResult {
    let _trace = traced();
    let host = Engine::start("host");
    let core = &host.core;
    core.set_video_codecs(VideoCodecCaps::h264_full(64, 64, "test"))?;
    // No capture could start: on its own that reads as not implemented.
    assert_eq!(
        core.capability_availability(None, Capability::DesktopView)?
            .remote_status(),
        RemoteStatus::Unsupported,
        "without a capture, view is unsupported"
    );
    // The owner has not granted screen capture: that is what a peer is told.
    core.report_os_permission(Capability::DesktopView, true)?;
    assert_eq!(
        core.capability_availability(None, Capability::DesktopView)?
            .remote_status(),
        RemoteStatus::NeedsPermission,
        "without consent, view needs permission"
    );
    Ok(())
}
