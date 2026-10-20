use super::support::{pair_for_control, start_direct, ShutdownCapture};
use crate::diagnostics::{
    build_snapshot, export_json, file_name_only, fingerprint, monotonic_ms, ConnectionSummary,
    DiagnosticSnapshot, PipelineSummary, QueueDepths, ResourceCounts, ServiceSummary,
    SnapshotInputs, MAX_EXPORT_BYTES, MAX_SNAPSHOT_LISTED, SNAPSHOT_VERSION,
};
use crate::{ServiceKind, SessionPhase};

fn wait(mut condition: impl FnMut() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !condition() {
        assert!(
            std::time::Instant::now() < until,
            "diagnostics condition timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn connection(id: u64) -> ConnectionSummary {
    ConnectionSummary {
        id,
        role: "controller".into(),
        phase: "active".into(),
        generation: 1,
        peer: fingerprint(&[id as u8; 32]),
        services: 1,
        path: "direct".into(),
        rtt_ms: Some(3),
        tx_bytes: 100,
        rx_bytes: 200,
    }
}

fn service(id: u64) -> ServiceSummary {
    ServiceSummary {
        id,
        connection_id: 1,
        kind: "files".into(),
        phase: "active".into(),
        hosted: false,
    }
}

fn pipeline(id: u64) -> PipelineSummary {
    PipelineSummary {
        service_id: id,
        hosted: true,
        codec: "h264".into(),
        width: 1920,
        height: 1080,
        control: "controlling".into(),
        ..Default::default()
    }
}

fn inputs(connections: usize, services: usize) -> SnapshotInputs {
    inputs_pipelines(connections, services, 0)
}

fn inputs_pipelines(connections: usize, services: usize, pipelines: usize) -> SnapshotInputs {
    SnapshotInputs {
        monotonic_ms: 7,
        hosting: true,
        connections: (0..connections as u64).map(connection).collect(),
        services: (0..services as u64).map(service).collect(),
        pipelines: (0..pipelines as u64).map(pipeline).collect(),
        resources: ResourceCounts {
            paired_devices: 2,
            ..Default::default()
        },
        queues: QueueDepths::default(),
    }
}

#[test]
fn fingerprint_is_short_hex_and_never_the_key() {
    let key = [0xABu8; 32];
    assert_eq!(fingerprint(&key), "abababab");
    assert_eq!(fingerprint(&[0u8; 32]), "00000000");
    assert_ne!(fingerprint(&[1u8; 32]), fingerprint(&[2u8; 32]));
}

#[test]
fn file_name_only_strips_directories_and_bounds_length() {
    assert_eq!(file_name_only("/Users/someone/Secret/file.txt"), "file.txt");
    assert_eq!(file_name_only("C:\\Users\\someone\\doc.pdf"), "doc.pdf");
    assert_eq!(file_name_only("bare"), "bare");
    assert_eq!(file_name_only("/trailing/slash/"), "?");
    assert_eq!(file_name_only(""), "?");
    let long = format!("/tmp/{}.txt", "é".repeat(200));
    let name = file_name_only(&long);
    assert!(name.len() <= 128);
    assert!(name.starts_with("é"));
}

#[test]
fn build_marks_truncation_at_the_list_cap() {
    let small = build_snapshot(inputs(2, 3));
    assert!(!small.truncated);
    assert_eq!(small.connections.len(), 2);
    assert_eq!(small.services.len(), 3);
    assert_eq!(small.version, SNAPSHOT_VERSION);
    let big = build_snapshot(inputs(MAX_SNAPSHOT_LISTED + 10, MAX_SNAPSHOT_LISTED + 1));
    assert!(big.truncated);
    assert_eq!(big.connections.len(), MAX_SNAPSHOT_LISTED);
    assert_eq!(big.services.len(), MAX_SNAPSHOT_LISTED);
    let piped = build_snapshot(inputs_pipelines(1, 1, MAX_SNAPSHOT_LISTED + 5));
    assert!(piped.truncated);
    assert_eq!(piped.pipelines.len(), MAX_SNAPSHOT_LISTED);
    // Counts describe live state, not the truncated lists.
    assert_eq!(big.resources.paired_devices, 2);
}

#[test]
fn export_round_trips_small_snapshots_as_expected_keys() {
    let snapshot = build_snapshot(inputs(1, 1));
    let json = export_json(&snapshot).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let keys: Vec<&str> = parsed
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "connections",
            "hosting",
            "monotonic_ms",
            "pipelines",
            "queues",
            "resources",
            "services",
            "truncated",
            "version"
        ]
    );
    // The exact key set is the schema: counts, labels and phases only.
    // No screen, clipboard, terminal, token, secret or path field exists.
    assert!(json.len() <= MAX_EXPORT_BYTES);
}

#[test]
fn export_sheds_lists_to_fit_the_cap_and_says_so() {
    let mut snapshot = DiagnosticSnapshot {
        version: SNAPSHOT_VERSION,
        monotonic_ms: 9,
        hosting: true,
        connections: (0..2000u64).map(connection).collect(),
        services: (0..2000u64).map(service).collect(),
        pipelines: (0..2000u64).map(pipeline).collect(),
        resources: ResourceCounts::default(),
        queues: QueueDepths::default(),
        truncated: false,
    };
    // Sanity: the untrimmed form really exceeds the cap.
    assert!(serde_json::to_string(&snapshot).unwrap().len() > MAX_EXPORT_BYTES);
    let json = export_json(&snapshot).unwrap();
    assert!(json.len() <= MAX_EXPORT_BYTES);
    snapshot = serde_json::from_str(&json).unwrap();
    assert!(snapshot.truncated);
    assert!(snapshot.connections.len() + snapshot.services.len() + snapshot.pipelines.len() < 6000);
}

#[test]
fn monotonic_clock_never_goes_backwards() {
    let first = monotonic_ms();
    let second = monotonic_ms();
    assert!(second >= first);
}

/// Needs loopback cores. A live connection and service appear with phases
/// and counts, peers as fingerprints, and the export carries no endpoint id.
#[test]
fn live_snapshot_lists_connections_services_and_counts() {
    let host = start_direct();
    let controller = start_direct();
    pair_for_control(&host, &controller);
    host.set_service_sharing(ServiceKind::Files, true).unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(pending.id).unwrap().phase == SessionPhase::Active);
    let parent = controller.controller_session(pending.id).unwrap();
    controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let snapshot = controller.diagnostic_snapshot().unwrap();
    assert_eq!(snapshot.resources.paired_devices, 1);
    assert_eq!(snapshot.resources.outgoing_connections, 1);
    assert_eq!(snapshot.resources.controller_services, 1);
    assert_eq!(snapshot.connections.len(), 1);
    let connection = &snapshot.connections[0];
    assert_eq!(connection.role, "controller");
    assert_eq!(connection.phase, "active");
    assert_eq!(connection.peer.len(), 8);
    assert_eq!(snapshot.services.len(), 1);
    assert_eq!(snapshot.services[0].kind, "files");
    assert!(!snapshot.services[0].hosted);
    let json = export_json(&snapshot).unwrap();
    assert!(!json.contains(&host.endpoint_id()));
    assert!(!json.contains(&controller.endpoint_id()));
    assert!(json.contains(&connection.peer));
    let hosted = host.diagnostic_snapshot().unwrap();
    assert_eq!(hosted.resources.incoming_connections, 1);
    assert_eq!(hosted.connections[0].role, "host");
}

/// Needs loopback cores. A live Desktop owner appears as a pipeline with
/// codec, geometry, control state and queue counters; the loopback
/// connection reports a direct path with round-trip and byte totals; the
/// input lease names its connection.
#[test]
fn live_snapshot_reports_pipeline_path_and_lease() {
    let host = start_direct();
    host.set_capture(Box::new(ShutdownCapture)).unwrap();
    let controller = start_direct();
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
    wait(|| desktop.control_state().unwrap() == crate::DesktopControlState::Controlling);
    // Publish one frame so the produce queue has something to count.
    let remote = host
        .hosted_services()
        .unwrap()
        .into_iter()
        .find(|s| s.kind == ServiceKind::Desktop)
        .unwrap();
    host.publish_desktop_video(
        remote.id,
        remote.generation,
        crate::EncodedFrame::h264(true, 1, vec![7; 64]).unwrap(),
    )
    .unwrap();
    let snapshot = host.diagnostic_snapshot().unwrap();
    assert_eq!(snapshot.pipelines.len(), 1);
    let pipeline = &snapshot.pipelines[0];
    assert!(pipeline.hosted);
    assert_eq!(pipeline.service_id, remote.id);
    assert!(pipeline.codec == "h264" || pipeline.codec == "h264-high");
    assert_eq!((pipeline.width, pipeline.height), (1, 1));
    assert_eq!(pipeline.control, "controlling");
    assert_eq!(pipeline.selected_display, None);
    assert_eq!(pipeline.fault, None);
    // keyframe_pending is best-effort: the serve loop consumes the flag,
    // so no stable live assertion exists. The field rides the same struct.
    assert_eq!(snapshot.connections.len(), 1);
    let connection = &snapshot.connections[0];
    assert_eq!(connection.path, "direct");
    assert!(connection.rtt_ms.is_some());
    assert!(connection.tx_bytes > 0);
    assert!(connection.rx_bytes > 0);
    assert_eq!(snapshot.resources.control_connection, Some(connection.id));
    let export = export_json(&snapshot).unwrap();
    assert!(export.contains("\"pipelines\""));
    assert!(!export.contains(&host.endpoint_id()));
    // The keyframe sent is counted with its bytes on the host; a keyframe
    // the viewer asks for is counted on the viewer.
    wait(|| {
        let pipeline = &host.diagnostic_snapshot().unwrap().pipelines[0];
        pipeline.keyframes_sent == 1 && pipeline.keyframe_bytes == 64
    });
    desktop.request_keyframe().unwrap();
    wait(|| {
        controller
            .diagnostic_snapshot()
            .unwrap()
            .pipelines
            .first()
            .is_some_and(|pipeline| pipeline.keyframes_requested >= 1)
    });
    let viewer = &controller.diagnostic_snapshot().unwrap().pipelines[0];
    assert_eq!(
        (
            viewer.lost_expired,
            viewer.lost_evicted,
            viewer.lost_gap,
            viewer.lost_stale
        ),
        (0, 0, 0, 0),
        "a clean loopback loses nothing"
    );
    desktop.close();
}
