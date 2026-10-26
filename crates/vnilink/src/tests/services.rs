use super::support::{pair_for_control, start_direct};
use crate::{Core, ServiceKind, SessionInfo, SessionPhase};
use std::time::{Duration, Instant};

fn wait(condition: impl FnMut() -> bool) {
    wait_named("service condition", condition);
}

fn wait_named(name: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < deadline, "{name} timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn active(core: &Core, id: u64) -> SessionInfo {
    wait(|| core.controller_session(id).unwrap().phase == SessionPhase::Active);
    core.controller_session(id).unwrap()
}

#[test]
fn remote_service_negotiation_rejects_incompatible_or_stale_requests() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let session = active(
        &controller,
        controller.begin_connect(&host.endpoint_id()).unwrap().id,
    );
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    let header = |version: u8, generation: u64, kind: u8| {
        let mut bytes = vec![8, version, 1];
        bytes.extend_from_slice(&incoming.id.to_be_bytes());
        bytes.extend_from_slice(&generation.to_be_bytes());
        bytes.extend_from_slice(&123u64.to_be_bytes());
        bytes.push(kind);
        bytes
    };
    for (bytes, expected) in [
        (header(2, incoming.generation, 2), 5),
        (header(1, incoming.generation + 1, 2), 3),
        (header(1, incoming.generation, 3), 6),
        (header(1, incoming.generation, 4), 6),
        (header(1, incoming.generation, 255), 1),
        (header(1, incoming.generation, 2), 2),
    ] {
        assert_eq!(
            controller
                .test_service_frame(session.id, session.generation, bytes)
                .unwrap(),
            vec![1, expected, 0]
        );
        assert!(host.hosted_services().unwrap().is_empty());
        assert_eq!(controller.controller_session(session.id).unwrap(), session);
    }
    // Permission and backend availability are distinct. An explicit grant
    // changes Denied to Unsupported without creating a shell/camera backend.
    host.set_service_sharing(ServiceKind::Terminal, true)
        .unwrap();
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 3)
            )
            .unwrap(),
        vec![1, 1, 0]
    );
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 4)
            )
            .unwrap(),
        vec![1, 6, 0]
    );
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 2)
            )
            .unwrap(),
        vec![1, 2, 0]
    );
    host.set_service_sharing(ServiceKind::Camera, true).unwrap();
    assert_eq!(
        controller
            .test_service_frame(
                session.id,
                session.generation,
                header(1, incoming.generation, 4)
            )
            .unwrap(),
        vec![1, 1, 0]
    );
    assert!(host.hosted_services().unwrap().is_empty());
    assert!(controller
        .test_service_frame(session.id, session.generation, vec![8, 1])
        .is_err());
    assert!(
        controller
            .remote_service_support(session.id, session.generation)
            .unwrap()
            .files
    );
}
