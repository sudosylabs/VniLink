use super::support::{pair_for_control, start_direct};
use crate::availability::{
    evaluate, policy_descriptor, AvailabilityInputs, AvailabilityReason, BackendState, Capability,
    PolicyChangeScope, POLICIES,
};
use crate::{ServiceKind, SystemPolicy};

fn granted_inputs() -> AvailabilityInputs {
    AvailabilityInputs {
        implemented: true,
        os_denied: false,
        device_missing: false,
        policy_allowed: true,
        peer_granted: true,
        capacity: None,
        backend_failed: None,
    }
}

#[test]
fn full_facts_mean_available_with_a_plain_summary() {
    let availability = evaluate(Capability::Files, &granted_inputs());
    assert!(availability.available);
    assert!(availability.reasons.is_empty());
    assert_eq!(availability.summarize(), "files: available");
}

#[test]
fn every_blocker_is_reported_most_fundamental_first() {
    let availability = evaluate(
        Capability::Terminal,
        &AvailabilityInputs {
            implemented: false,
            os_denied: true,
            device_missing: true,
            policy_allowed: false,
            peer_granted: false,
            capacity: Some((16, 16)),
            backend_failed: Some("pty spawn refused".into()),
        },
    );
    assert!(!availability.available);
    assert_eq!(
        availability
            .reasons
            .iter()
            .map(AvailabilityReason::label)
            .collect::<Vec<_>>(),
        [
            "missing-implementation",
            "denied-os-permission",
            "unavailable-device",
            "disabled-local-policy",
            "denied-peer-grant",
            "busy-resource",
            "backend-failed",
        ]
    );
    assert_eq!(
        availability.summarize(),
        "terminal: unavailable (missing-implementation, denied-os-permission, \
         unavailable-device, disabled-local-policy, denied-peer-grant, \
         busy-resource, backend-failed)"
    );
}

#[test]
fn capacity_binds_only_at_the_limit_and_carries_counts() {
    let under = evaluate(
        Capability::Terminal,
        &AvailabilityInputs {
            capacity: Some((15, 16)),
            ..granted_inputs()
        },
    );
    assert!(under.available);
    let at_limit = evaluate(
        Capability::Terminal,
        &AvailabilityInputs {
            capacity: Some((16, 16)),
            ..granted_inputs()
        },
    );
    assert!(!at_limit.available);
    assert_eq!(
        at_limit.reasons,
        [AvailabilityReason::BusyResource {
            used: 16,
            limit: 16
        }]
    );
}

#[test]
fn backend_failure_detail_is_bounded_on_char_boundaries() {
    let availability = evaluate(
        Capability::Camera,
        &AvailabilityInputs {
            backend_failed: Some("é".repeat(300)),
            ..granted_inputs()
        },
    );
    assert!(!availability.available);
    let [AvailabilityReason::BackendFailed { detail }] = availability.reasons.as_slice() else {
        panic!(
            "expected one backend failure, got {:?}",
            availability.reasons
        );
    };
    assert!(detail.len() <= 256);
    assert!(detail.chars().all(|c| c == 'é'));
}

#[test]
fn each_capability_has_a_distinct_stable_label() {
    let capabilities = [
        Capability::DesktopView,
        Capability::DesktopControl,
        Capability::Clipboard,
        Capability::Files,
        Capability::Terminal,
        Capability::Camera,
        Capability::SystemAudio,
        Capability::Microphone,
        Capability::SystemLock,
        Capability::SystemRestart,
        Capability::SystemShutdown,
        Capability::SystemPrivacy,
        Capability::Recording,
    ];
    let mut labels: Vec<&str> = capabilities.iter().map(|c| c.label()).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), capabilities.len());
}

#[test]
fn policy_registry_names_every_durable_policy_with_its_scope() {
    assert_eq!(POLICIES.len(), 5);
    assert_eq!(
        policy_descriptor("terminal").unwrap().scope,
        PolicyChangeScope::IdleGated
    );
    assert_eq!(
        policy_descriptor("file-retention").unwrap().scope,
        PolicyChangeScope::Immediate
    );
    assert_eq!(
        policy_descriptor("file-resource").unwrap().scope,
        PolicyChangeScope::NextOwner
    );
    assert_eq!(
        policy_descriptor("power").unwrap().scope,
        PolicyChangeScope::Immediate
    );
    assert_eq!(
        policy_descriptor("system").unwrap().scope,
        PolicyChangeScope::Immediate
    );
    assert!(policy_descriptor("no-such-policy").is_err());
    for descriptor in POLICIES {
        assert!(!descriptor.summary.is_empty());
    }
}

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
    host.authorize_service(&peer, ServiceKind::Files, true)
        .unwrap();
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
    host.authorize_recording(&peer, true).unwrap();
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
