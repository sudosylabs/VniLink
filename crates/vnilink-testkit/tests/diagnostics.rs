//! Diagnostics: what an engine's snapshot says of its live connections,
//! services and pictures, and of failures it ignored, peers only by
//! fingerprint and nothing a user owns; and that sessions opened and
//! closed again and again leave nothing behind.

#[path = "diagnostics/soak.rs"]
mod soak;

use expect_test::expect;

use vnilink::{
    export_json, DesktopControlState, EncoderImplementation, EncoderInput, ServiceKind,
    StreamFailure,
};
use vnilink_testkit::desktop::{keyframe, MonitorCapture};
use vnilink_testkit::{traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT};

/// A live services connection and its Files service are listed on the
/// controller with phases and counts, the peer by fingerprint; the export
/// names neither computer; the host lists the connection as its own.
#[test]
fn live_snapshot_lists_connections_services_and_counts() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Files]);
    let (host, controller) = (&pair.host, &pair.controller);
    let connection = pair.connect_services();
    pair.attach(&connection, ServiceKind::Files);
    let snapshot = controller.core.diagnostic_snapshot()?;
    let resources = &snapshot.resources;
    assert_eq!(resources.paired_devices, 1, "one paired computer");
    assert_eq!(resources.outgoing_connections, 1, "one outgoing connection");
    assert_eq!(resources.controller_services, 1, "one service it opened");
    assert_eq!(snapshot.connections.len(), 1, "the connection is listed");
    let listed = &snapshot.connections[0];
    assert_eq!(listed.role, "controller", "as the controller's");
    assert_eq!(listed.phase, "active", "and active");
    assert_eq!(listed.peer.len(), 8, "its peer by fingerprint");
    assert_eq!(snapshot.services.len(), 1, "the service is listed");
    assert_eq!(snapshot.services[0].kind, "files", "as Files");
    assert!(!snapshot.services[0].hosted, "opened, not hosted");
    let json = export_json(&snapshot)?;
    assert!(
        !json.contains(&host.endpoint()),
        "the export does not name the host"
    );
    assert!(!json.contains(&controller.endpoint()), "nor the controller");
    assert!(json.contains(&listed.peer), "only its fingerprint");
    let hosted = host.core.diagnostic_snapshot()?;
    assert_eq!(
        hosted.resources.incoming_connections, 1,
        "the host counts the incoming connection"
    );
    assert_eq!(
        hosted.connections[0].role, "host",
        "and lists it as the host's"
    );
    Ok(())
}

/// A live Desktop is listed on the host as a pipeline with its codec,
/// size, control and faults; the connection reports its direct path with
/// round trip, bytes and congestion state, and the input lease names it.
/// The host counts each keyframe sent with its bytes and hears the viewer
/// lose nothing; the viewer counts the keyframe it asked for and loses
/// nothing.
#[test]
fn live_snapshot_reports_pipeline_path_and_lease() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host, &pair.controller);
    host.core
        .set_capture(Box::new(MonitorCapture::new((1, 1))))?;
    let connection = pair.connect_services();
    let desktop = controller.open_desktop(&connection)?;
    wait_for("the viewer to control", DEFAULT_WAIT, || {
        (desktop.control_state().expect("the viewer's control state")
            == DesktopControlState::Controlling)
            .then_some(())
    });
    // One frame, so the produce queue has something to count.
    let remote = host
        .hosted_for(ServiceKind::Desktop, controller)
        .ok_or("the host serves the viewer's desktop")?;
    host.core
        .publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![7; 64]))?;
    let snapshot = host.core.diagnostic_snapshot()?;
    assert_eq!(snapshot.pipelines.len(), 1, "the desktop is a pipeline");
    let pipeline = &snapshot.pipelines[0];
    assert!(pipeline.hosted, "hosted");
    assert_eq!(pipeline.service_id, remote.id, "of the served desktop");
    assert!(
        pipeline.codec == "h264" || pipeline.codec == "h264-high",
        "in the agreed codec: {}",
        pipeline.codec
    );
    assert_eq!(
        (pipeline.width, pipeline.height),
        (1, 1),
        "at the display's size"
    );
    assert_eq!(pipeline.control, "controlling", "under control");
    assert_eq!(pipeline.selected_display, None, "with no display chosen");
    assert_eq!(pipeline.fault, None, "and no fault");
    // keyframe_pending is best-effort: the serve loop consumes the flag,
    // so no stable live assertion exists. The field rides the same struct.
    assert_eq!(snapshot.connections.len(), 1, "one connection");
    let listed = &snapshot.connections[0];
    assert_eq!(listed.path, "direct", "on loopback, direct");
    assert!(listed.rtt_ms.is_some(), "with a round trip");
    assert!(listed.tx_bytes > 0, "bytes sent");
    assert!(listed.rx_bytes > 0, "and received");
    // The selected path's own congestion state; loopback loses nothing.
    assert!(
        listed.cwnd.is_some_and(|cwnd| cwnd > 0),
        "a congestion window: {:?}",
        listed.cwnd
    );
    assert_eq!(listed.lost_packets, Some(0), "no packet lost");
    assert_eq!(listed.black_holes_detected, Some(0), "no black hole");
    assert_eq!(
        snapshot.resources.control_connection,
        Some(listed.id),
        "the input lease names the connection"
    );
    let export = export_json(&snapshot)?;
    assert!(export.contains("\"pipelines\""), "the export has pipelines");
    assert!(
        !export.contains(&host.endpoint()),
        "and does not name the host"
    );
    wait_for("the host to count the keyframe sent", DEFAULT_WAIT, || {
        let pipeline = &host
            .core
            .diagnostic_snapshot()
            .expect("the host's snapshot")
            .pipelines[0];
        ((pipeline.keyframes_sent, pipeline.keyframe_bytes) == (1, 64)).then_some(())
    });
    // The viewer reports what reached it; the host hears no loss. A report
    // is current for `PIPELINE_FRESH_SECS` and one frame brings only one, so
    // a test thread held off longer would wait for a report that never
    // comes. While the host has none, another keyframe brings a fresh one;
    // a keyframe never makes the viewer ask for another.
    wait_for(
        "the host to hear the viewer lose nothing",
        DEFAULT_WAIT,
        || {
            let pipeline = host
                .core
                .diagnostic_snapshot()
                .expect("the host's snapshot")
                .pipelines[0]
                .clone();
            let loss = (
                pipeline.viewer_frame_loss_permille,
                pipeline.viewer_fragment_loss_permille,
            );
            if loss == (None, None) {
                host.core
                    .publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![7; 64]))
                    .expect("another keyframe");
            }
            (loss == (Some(0), Some(0))).then_some(())
        },
    );
    desktop.request_keyframe()?;
    wait_for(
        "the viewer to count the keyframe it asked for",
        DEFAULT_WAIT,
        || {
            let requested = controller
                .core
                .diagnostic_snapshot()
                .expect("the viewer's snapshot")
                .pipelines
                .first()?
                .keyframes_requested;
            (requested >= 1).then_some(())
        },
    );
    let viewer = &controller.core.diagnostic_snapshot()?.pipelines[0];
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
    Ok(())
}

/// The snapshot names what the coders are, and why the last picture
/// ended on both computers after that pipeline left it.
#[test]
fn live_snapshot_names_the_coders_and_the_last_end() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let (host, controller) = (&pair.host, &pair.controller);
    host.core
        .set_capture(Box::new(MonitorCapture::new((1, 1))))?;
    let connection = pair.connect_services();
    let desktop = controller.open_desktop(&connection)?;
    let remote = host
        .hosted_for(ServiceKind::Desktop, controller)
        .ok_or("the host serves the viewer's desktop")?;
    assert_eq!(
        host.core.diagnostic_snapshot()?.version,
        5,
        "the snapshot's version"
    );
    host.core.report_desktop_encoder(
        remote.id,
        remote.generation,
        EncoderImplementation {
            name: "videotoolbox",
            hardware: Some(true),
            input: Some(EncoderInput::GpuTexture),
        },
    )?;
    desktop.report_decoder(true, 3, 2)?;
    let hosted = host.core.diagnostic_snapshot()?.pipelines[0].clone();
    assert_eq!(
        hosted.encoder.as_deref(),
        Some("videotoolbox"),
        "the host names its encoder"
    );
    assert_eq!(hosted.encoder_hardware, Some(true), "as hardware");
    assert_eq!(
        hosted.encoder_input.as_deref(),
        Some("gpu-texture"),
        "fed GPU textures"
    );
    assert_eq!(hosted.end, None, "a live picture has no end");
    let viewing = controller.core.diagnostic_snapshot()?.pipelines[0].clone();
    assert_eq!(
        viewing.decoder_hardware,
        Some(false),
        "the viewer's decoder is software"
    );
    assert_eq!(viewing.decoder_recoveries, 3, "with its recoveries");
    assert_eq!(viewing.stills_shown, 2, "and the stills it showed");
    assert_eq!(
        viewing.encoder, None,
        "the viewer has no encoder of its own"
    );
    host.core
        .fail_hosted_service(remote.id, remote.generation, StreamFailure::Encoder)?;
    wait_for("the viewer to learn the end", DEFAULT_WAIT, || {
        desktop.end().map(drop)
    });
    for (engine, hosted) in [(host, true), (controller, false)] {
        let end = engine
            .core
            .diagnostic_snapshot()?
            .last_end
            .ok_or("the last end is kept")?;
        assert_eq!(
            end.reason, "hostEncoderFailed",
            "the end names the host's encoder"
        );
        assert_eq!(end.hosted, hosted, "on the side that hosted it or not");
    }
    let export = export_json(&controller.core.diagnostic_snapshot()?)?;
    assert!(
        export.contains("\"last_end\""),
        "the export has the last end"
    );
    trace.wait_for_records("desktop ended", 2);
    trace.wait_for_records("service phase", 2);
    expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          session: desktop ended hosted=false reason=hostEncoderFailed service=1
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          stream: encoder opened encoder=EncoderImplementation { name: "videotoolbox", hardware: Some(true), input: Some(GpuTexture) } generation=1 service=1
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Failed(Encoder)
          session: desktop ended hosted=true reason=hostEncoderFailed service=1
    "#]].assert_eq(&trace.story(&["session", "service", "stream"]));
    Ok(())
}

/// A failure an area ignored reaches an engine's snapshot by area and
/// reason, never by what failed.
#[test]
fn ignored_failures_reach_the_snapshot_by_area_and_reason() -> TestResult {
    let trace = traced();
    let engine = Engine::start("host");
    // Ignored in the engine's work, as its areas ignore failures.
    engine.core.trace_span().in_scope(|| {
        vnilink::ignored!(
            vnilink::trace::CLIPBOARD,
            "the diagnostics test ignores it",
            Err::<(), _>(vnilink::Error::Network("192.0.2.9".into()))
        )
    });
    let snapshot = engine.core.diagnostic_snapshot()?;
    let counted = snapshot
        .ignored
        .iter()
        .find(|count| count.why == "the diagnostics test ignores it")
        .ok_or("the ignored failure is counted")?;
    assert_eq!(counted.area, vnilink::trace::CLIPBOARD, "under its area");
    assert!(counted.count >= 1, "at least once: {}", counted.count);
    assert!(
        !export_json(&snapshot)?.contains("192.0.2.9"),
        "a failure's text was exported"
    );
    trace.assert_unrecorded("a failure's text", &["192.0.2.9"]);
    Ok(())
}
