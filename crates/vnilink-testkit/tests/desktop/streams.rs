// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! The pictures themselves: capture fenced to its consumers, pressure that
//! adapts one owner, twin sources, conditions the viewer learns, why a
//! picture ended, pictures reaching a Desktop's viewer, and the size a
//! host serves.

use expect_test::expect;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use vnilink::{
    AdaptationReason, Condition, DesktopControlState, DesktopEnd, DisplayInfo, EncodedFrame,
    EncoderImplementation, Error, Grants, InputEvent, ServiceInfo, ServiceKind, StreamEnd,
    StreamFailure, VideoCodecCaps,
};
use vnilink_testkit::desktop::{input_failed, keyframe, next_picture, InputLog, MonitorCapture};
use vnilink_testkit::drops::{DropPoint, SeededDrops};
use vnilink_testkit::{
    holds, traced, wait_for, Engine, Pair, TestResult, DEFAULT_WAIT, SEEN_AND_CONTROLLED,
};

/// The size of the host's screens.
pub(super) const SCREEN: (u16, u16) = (1920, 1080);

/// A display of the host, at revision `revision`.
pub(super) fn display(id: u64, x: i32, width: u16, revision: u64) -> DisplayInfo {
    DisplayInfo {
        id,
        origin_x: x,
        origin_y: 0,
        width,
        height: SCREEN.1,
        rotation_deg: 0,
        scale_percent: 100,
        revision,
    }
}

/// The host's Desktop service whose applied selection is `display` alone.
pub(super) fn hosted_showing(host: &Engine, display: u64) -> ServiceInfo {
    wait_for(
        &format!("the host to apply a selection of display {display}"),
        DEFAULT_WAIT,
        || {
            host.hosted(Some(ServiceKind::Desktop))
                .into_iter()
                .find(|service| {
                    host.core
                        .hosted_display_selection(service.id, service.generation)
                        .ok()
                        .as_deref()
                        == Some(&[display][..])
                })
        },
    )
}

/// How often a producer publishes while a test waits for the viewer to
/// learn something: a picture every 50 ms, as a 20 fps stream.
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Says when the next picture is due, so a producer stands in for a
/// stream's clock without sleeping.
struct Paced {
    last: Option<Instant>,
}

impl Paced {
    fn new() -> Self {
        Self { last: None }
    }

    fn due(&mut self) -> bool {
        let due = self
            .last
            .is_none_or(|last| last.elapsed() >= FRAME_INTERVAL);
        if due {
            self.last = Some(Instant::now());
        }
        due
    }
}

/// A physical capture that starts serves the desktops open when it began:
/// it lasts while one of them is open, is not adopted by a desktop opened
/// after they all closed, and ends with its engine.
#[test]
fn capture_startup_serves_only_the_desktops_open_when_it_began() -> TestResult {
    let trace = traced();
    let timeout = Duration::from_secs(5);
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    host.set_capture(Box::new(MonitorCapture::new(SCREEN)))?;
    let stop = Arc::new(AtomicBool::new(false));
    let alone = host.desktop_capture_startup(0, Arc::clone(&stop), timeout);
    assert!(
        matches!(alone, Err(Error::NoSession)),
        "no capture starts with no desktop to serve: {:?}",
        alone.err()
    );
    let second = pair.add_viewer("second");
    let viewers = [&pair.controller, &second];
    let mut connections = Vec::new();
    let mut desktops = Vec::new();
    for viewer in viewers {
        let connection = viewer.connect_services(&pair.host)?;
        desktops.push(viewer.open_desktop(&connection)?);
        connections.push(connection);
    }
    wait_for("the host to serve both desktops", DEFAULT_WAIT, || {
        (host.video_targets().ok()?.len() == 2).then_some(())
    });
    let startup = host.desktop_capture_startup(0, Arc::clone(&stop), timeout)?;

    desktops[0].close();
    wait_for("the host to serve one desktop", DEFAULT_WAIT, || {
        (host.video_targets().ok()?.len() == 1).then_some(())
    });
    assert!(
        startup.check().is_ok(),
        "one departing desktop leaves the capture its sibling waits for"
    );
    desktops[1].close();
    wait_for("the host to serve no desktop", DEFAULT_WAIT, || {
        host.video_targets().ok()?.is_empty().then_some(())
    });
    assert!(
        matches!(startup.check(), Err(Error::NoSession)),
        "a capture whose desktops all closed is cancelled"
    );

    let replacement = pair.controller.open_desktop(&connections[0])?;
    assert!(
        matches!(startup.check(), Err(Error::NoSession)),
        "a desktop opened later does not adopt a cancelled capture"
    );
    let fresh = host.desktop_capture_startup(0, Arc::clone(&stop), timeout)?;
    assert!(
        fresh.check().is_ok(),
        "the new desktop starts its own capture"
    );
    host.shutdown();
    assert!(
        matches!(fresh.check(), Err(Error::NoSession)),
        "a capture ends with its engine"
    );
    drop(replacement);
    // The viewers learn of the shutdown at moments of their own: whether
    // the replacement suspends before it is dropped is theirs to race.
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
          service: service phase from=active generation=2 kind=Desktop service=2 to=closed: PeerClose
          service: service phase from=active generation=1 kind=Desktop service=3 to=closed: Shutdown
    "#]],
        || trace.engine_story("host", &["desktop", "capture", "service"]),
    );
    Ok(())
}

/// A viewer whose pictures wait unread adapts its own frame rate to that
/// pressure, alone: its sibling on another computer keeps streaming at the
/// quality it had and learns the host's and its decoder's timings, which
/// stay its own; once the slow viewer closes, the sibling still streams.
#[test]
fn receiver_pressure_adapts_only_its_owner_and_the_sibling_keeps_streaming() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    InputLog::install(&pair.host, SCREEN)?;
    let second = pair.add_viewer("second");
    let slow = pair
        .controller
        .open_desktop(&pair.controller.connect_services(&pair.host)?)?;
    let slow_host = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    let fast = second.open_desktop(&second.connect_services(&pair.host)?)?;
    let fast_host = pair.host.serving(ServiceKind::Desktop, &second);
    let _slow_video = slow.subscribe_video()?;
    let fast_video = fast.subscribe_video()?;
    let initial = slow.desktop_info()?.quality;
    let host = &pair.host.core;
    // One picture the slow viewer never reads.
    host.publish_desktop_video(
        slow_host.id,
        slow_host.generation,
        keyframe(1, vec![31; 1000]),
    )?;

    let budget = Duration::from_secs(8);
    let started = Instant::now();
    let mut paced = Paced::new();
    let mut timestamp = 2;
    let mut stream_fast = |payload: u8| {
        if !paced.due() {
            return;
        }
        host.report_desktop_producer_duration(fast_host.id, fast_host.generation, 17, 29)
            .expect("the host reports its producer's timings");
        fast.report_decode_duration(1000)
            .expect("the window reports its decode time");
        host.publish_desktop_video(
            fast_host.id,
            fast_host.generation,
            keyframe(timestamp, vec![payload; 1000]),
        )
        .expect("the host publishes to the fast viewer");
        assert_eq!(
            next_picture(&fast_video, "the fast viewer's picture").payload,
            vec![payload; 1000],
            "the fast viewer receives each picture published to it"
        );
        timestamp += 1;
    };
    wait_for("the slow viewer to adapt to its pressure", budget, || {
        stream_fast(42);
        (slow.desktop_info().ok()?.quality.max_frame_rate != initial.max_frame_rate).then_some(())
    });
    let changed = slow.desktop_info()?.quality;
    assert!(
        changed.max_frame_rate < initial.max_frame_rate,
        "pressure lowers the slow viewer's frame rate: {changed:?}"
    );
    assert_eq!(
        (
            changed.max_width,
            changed.max_height,
            changed.max_bitrate_kbps
        ),
        (
            initial.max_width,
            initial.max_height,
            initial.max_bitrate_kbps
        ),
        "pressure changes the frame rate alone"
    );
    let slowed = slow.media_status()?;
    assert_eq!(
        slowed.reason,
        AdaptationReason::ReceiverPressure,
        "the slow viewer says why it adapted"
    );
    assert!(
        slowed.receiver_queue_ms >= 150,
        "the slow viewer's queue is what pressed: {} ms",
        slowed.receiver_queue_ms
    );
    // Host feedback and receiver sampling have independent one-second
    // ticks: the sibling learns the timings at its own next sample.
    let learned = wait_for(
        "the fast viewer to learn the timings",
        budget.saturating_sub(started.elapsed()),
        || {
            stream_fast(42);
            let status = fast.media_status().ok()?;
            ((status.encode_us, status.capture_us, status.decode_us) == (29, 17, 1000))
                .then_some(status)
        },
    );
    assert!(
        learned.received_fps > 0,
        "the fast viewer measures what it receives"
    );
    assert_eq!(
        fast.desktop_info()?.quality,
        initial,
        "the sibling keeps the quality it had"
    );
    assert_eq!(
        slow.media_status()?.encode_us,
        0,
        "one owner's producer timings are not another's"
    );
    assert!(
        fast.report_decode_duration(5_000_001).is_err(),
        "a decode time beyond five seconds is refused"
    );
    slow.close();
    // The handle closes at once, before the host hears of it.
    assert!(
        slow.media_status().is_err(),
        "a closed desktop has no status"
    );
    assert!(
        slow.report_decode_duration(1000).is_err(),
        "a closed desktop takes no reports"
    );
    wait_for("the host to let the slow viewer go", DEFAULT_WAIT, || {
        (pair.host.hosted(Some(ServiceKind::Desktop)).len() == 1).then_some(())
    });
    host.publish_desktop_video(
        fast_host.id,
        fast_host.generation,
        keyframe(timestamp, vec![77; 1000]),
    )?;
    assert_eq!(
        next_picture(&fast_video, "the sibling's picture after the close").payload,
        vec![77; 1000],
        "the sibling streams after the slow viewer closed"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["desktop", "service"]),
    );
    Ok(())
}

/// Two Desktop owners on one connection showing twin displays, alike in
/// all but their ids, each receive their own display's pictures with its
/// exact stamp; an unstamped picture gains the live revision and no
/// invented display. Pointers name their display exactly, closing one owner
/// spares the other, and switching the survivor's display re-sources it.
#[test]
fn twin_sources_stream_independently_with_exact_stamps() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    let applied_events = InputLog::install(&pair.host, SCREEN)?;
    // Twins: identical geometry, distinct ids.
    host.set_topology(5, &[display(1, 0, 1920, 5), display(2, 1920, 1920, 5)])?;
    let connection = pair.connect_services();
    let desktop_a = pair.controller.open_desktop(&connection)?;
    let desktop_b = pair.controller.open_desktop(&connection)?;
    wait_for("both owners to learn the twins", DEFAULT_WAIT, || {
        (desktop_a.topology().ok()?.len() == 2 && desktop_b.topology().ok()?.len() == 2)
            .then_some(())
    });
    desktop_a.select_displays(&[1])?;
    desktop_b.select_displays(&[2])?;
    let remote_a = hosted_showing(&pair.host, 1);
    let remote_b = hosted_showing(&pair.host, 2);
    let mut targets: Vec<_> = host
        .video_targets()?
        .into_iter()
        .map(|target| (target.display_id, target.revision))
        .collect();
    targets.sort_unstable();
    assert_eq!(
        targets,
        vec![(1, 5), (2, 5)],
        "each owner's stream names its display and revision"
    );
    // Contended input is discarded before acceptance: acquire explicitly.
    desktop_a.acquire_control()?;
    wait_for("the first owner to control", DEFAULT_WAIT, || {
        (desktop_a.control_state().ok()? == DesktopControlState::Controlling).then_some(())
    });

    let video_a = desktop_a.subscribe_video()?;
    let video_b = desktop_b.subscribe_video()?;
    host.publish_desktop_video(
        remote_a.id,
        remote_a.generation,
        keyframe(1, vec![31; 1000]).with_source(1, 5),
    )?;
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        keyframe(1, vec![42; 1000]).with_source(2, 5),
    )?;
    let got_a = next_picture(&video_a, "the first twin's picture");
    assert_eq!(
        (got_a.payload, got_a.display_id, got_a.revision),
        (vec![31; 1000], 1, 5),
        "the first owner receives its twin's picture, stamped exactly"
    );
    let got_b = next_picture(&video_b, "the second twin's picture");
    assert_eq!(
        (got_b.payload, got_b.display_id, got_b.revision),
        (vec![42; 1000], 2, 5),
        "the second owner receives its twin's picture, stamped exactly"
    );
    host.publish_desktop_video(remote_a.id, remote_a.generation, keyframe(2, vec![33; 100]))?;
    let unstamped = next_picture(&video_a, "the unstamped picture");
    assert_eq!(
        (unstamped.display_id, unstamped.revision),
        (0, 5),
        "an unstamped picture gains the live revision, never an invented display"
    );

    let on_first = InputEvent::pointer(100, 100, 1, 5);
    desktop_a.submit_input(on_first)?;
    wait_for("the pointer on display 1 to apply", DEFAULT_WAIT, || {
        (!applied_events.events().is_empty()).then_some(())
    });
    assert_eq!(
        applied_events.events(),
        [on_first],
        "a pointer on the owner's twin applies"
    );
    let events = host.subscribe()?;
    desktop_a.submit_input(InputEvent::pointer(2000, 100, 2, 5))?;
    assert_eq!(
        input_failed(&events).1,
        "that display is not selected",
        "a pointer on the other twin is refused for this owner"
    );
    assert_eq!(
        applied_events.events().len(),
        1,
        "the refused pointer did not apply"
    );

    desktop_a.close();
    wait_for("the host to drop the first owner", DEFAULT_WAIT, || {
        (pair.host.hosted(Some(ServiceKind::Desktop)).len() == 1).then_some(())
    });
    assert!(
        host.publish_desktop_video(remote_a.id, remote_a.generation, keyframe(3, vec![31; 100]))
            .is_err(),
        "a closed owner takes no pictures"
    );
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        keyframe(3, vec![43; 500]).with_source(2, 5),
    )?;
    let survivor = next_picture(&video_b, "the survivor's picture");
    assert_eq!(
        (survivor.payload, survivor.display_id, survivor.revision),
        (vec![43; 500], 2, 5),
        "closing one owner spares the other's stamped stream"
    );
    desktop_b.select_displays(&[1])?;
    wait_for("the survivor to switch to display 1", DEFAULT_WAIT, || {
        (host
            .hosted_display_selection(remote_b.id, remote_b.generation)
            .ok()?
            == [1])
        .then_some(())
    });
    host.publish_desktop_video(
        remote_b.id,
        remote_b.generation,
        keyframe(4, vec![44; 500]).with_source(1, 5),
    )?;
    let switched = next_picture(&video_b, "the switched picture");
    assert_eq!(
        (switched.payload, switched.display_id, switched.revision),
        (vec![44; 500], 1, 5),
        "the survivor's stream is re-sourced to display 1"
    );
    assert_eq!(
        host.capture_displays()?,
        vec![1],
        "the host captures only the display still shown"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          input: control asked for epoch=1 service=1 state=Controlling
          warn input: input refused kind=pointer reason=that display is not selected service=1
    "#]],
        || trace.story(&["desktop", "topology", "input"]),
    );
    Ok(())
}

/// What the host says of itself, and what this window's decoder reports,
/// become the viewer's conditions, with their evidence; a clean loopback
/// adds no link strain, and a decoder that restarted shows as recovering.
#[test]
fn host_and_decoder_reports_become_the_viewers_conditions() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    InputLog::install(&pair.host, SCREEN)?;
    let connection = pair.connect_services();
    let viewer = pair.controller.open_desktop(&connection)?;
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    let video = viewer.subscribe_video()?;
    host.report_desktop_encoder(
        remote.id,
        remote.generation,
        EncoderImplementation {
            name: "test",
            hardware: Some(false),
            input: None,
        },
    )?;
    host.report_secure_desktop(true)?;
    viewer.report_decoder(true, 0, 0)?;

    // Conditions arrive with feedback, which needs pictures to flow.
    let mut paced = Paced::new();
    let mut timestamp = 1;
    let mut stream_until = |what: &str, done: &dyn Fn(&[Condition]) -> bool| {
        wait_for(what, DEFAULT_WAIT, || {
            if paced.due() {
                host.publish_desktop_video(
                    remote.id,
                    remote.generation,
                    keyframe(timestamp, vec![9; 500]),
                )
                .expect("the host publishes a picture");
                next_picture(&video, "the viewer's picture");
                timestamp += 1;
            }
            let set: Vec<_> = viewer.media_status().ok()?.conditions.iter().collect();
            done(&set).then_some(())
        });
    };
    stream_until("the host's reports to reach the viewer", &|set| {
        set.contains(&Condition::EncoderSoftware) && set.contains(&Condition::HostSecureDesktop)
    });
    let set: Vec<_> = viewer.media_status()?.conditions.iter().collect();
    assert_eq!(
        set,
        [
            Condition::HostSecureDesktop,
            Condition::EncoderSoftware,
            Condition::DecoderSoftware
        ],
        "the viewer's conditions are the reports, most severe first, with no link strain"
    );
    let signals: Vec<_> = viewer
        .conditions()?
        .iter()
        .map(|entry| entry.evidence.signal)
        .collect();
    assert_eq!(
        signals,
        [
            "host_secure_desktop",
            "host_encoder_hardware",
            "decoder_hardware"
        ],
        "each condition names the signal it rests on"
    );
    host.report_secure_desktop(false)?;
    viewer.report_decoder(false, 0, 0)?;
    stream_until("the cleared reports to reach the viewer", &|set| {
        set == [Condition::EncoderSoftware]
    });
    // The window's decoder restarted twice: it shows as recovering.
    viewer.report_decoder(false, 2, 1)?;
    stream_until("the decoder's restarts to reach the viewer", &|set| {
        set.contains(&Condition::DecoderRecovering)
    });
    assert!(
        viewer.report_decoder(false, 1, 1).is_err(),
        "a restart count that goes back belongs to another decoder"
    );
    viewer.close();
    vnilink_testkit::assert_story(
        expect![[r#"
        host:
          stream: encoder opened encoder=EncoderImplementation { name: "test", hardware: Some(false), input: None } generation=1 service=1
    "#]],
        || trace.story(&["stream"]),
    );
    Ok(())
}

/// How a picture ends in [`picture_ended_by`].
#[derive(Clone, Copy, Debug)]
enum Ending {
    HostCloses,
    HostFails(StreamFailure),
    HostStopsSharing,
    ViewerCloses,
}

/// Opens a picture, ends it by `ending`, and says what end its viewer
/// learned.
fn picture_ended_by(pair: &Pair, ending: Ending) -> Result<Option<DesktopEnd>, Error> {
    let host = &pair.host.core;
    InputLog::install(&pair.host, SCREEN)?;
    let connection = pair.connect_services();
    let viewer = pair.controller.open_desktop(&connection)?;
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    assert_eq!(viewer.end(), None, "a live picture has no end");
    match ending {
        Ending::HostCloses => host.close_hosted_service(remote.id, remote.generation)?,
        Ending::HostFails(failure) => {
            host.fail_hosted_service(remote.id, remote.generation, failure)?
        }
        Ending::HostStopsSharing => host.set_service_sharing(ServiceKind::Desktop, false)?,
        Ending::ViewerCloses => viewer.close(),
    }
    wait_for("the viewer's picture to end", DEFAULT_WAIT, || {
        viewer.is_cancelled().then_some(())
    });
    match ending {
        // The host has let the service go, and an end it sent by mistake
        // has the original's 300 ms to reach the viewer.
        Ending::ViewerCloses => {
            wait_for("the host to let the picture go", DEFAULT_WAIT, || {
                pair.host
                    .hosted(Some(ServiceKind::Desktop))
                    .is_empty()
                    .then_some(())
            });
            holds(
                "the viewer's closed picture having no end",
                Duration::from_millis(300),
                || viewer.end().is_none(),
            );
        }
        _ => wait_for("the viewer to learn why", DEFAULT_WAIT, || {
            viewer.end().is_some().then_some(())
        }),
    }
    Ok(viewer.end())
}

/// A host that closes the picture tells its viewer it stopped.
#[test]
fn the_viewer_learns_the_host_stopped_its_picture() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostCloses)?,
        Some(DesktopEnd::Host(StreamEnd::Stopped)),
        "a host's close reaches the viewer as stopped"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// A host whose encoder failed tells its viewer so.
#[test]
fn the_viewer_learns_the_hosts_encoder_failed() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostFails(StreamFailure::Encoder))?,
        Some(DesktopEnd::Host(StreamEnd::Failed(StreamFailure::Encoder))),
        "an encoder failure reaches the viewer as itself"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Failed(Encoder)
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// A host refused permission to capture tells its viewer so.
#[test]
fn the_viewer_learns_the_host_lacks_permission() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostFails(StreamFailure::Permission))?,
        Some(DesktopEnd::Host(StreamEnd::Failed(
            StreamFailure::Permission
        ))),
        "a permission failure reaches the viewer as itself"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Failed(Permission)
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// A host that stops sharing its desktop tells its viewer it was revoked.
#[test]
fn the_viewer_learns_the_host_stopped_sharing() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::HostStopsSharing)?,
        Some(DesktopEnd::Host(StreamEnd::Revoked)),
        "a withdrawn share reaches the viewer as revoked"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: Revoked
    "#]],
        || trace.story(&["service"]),
    );
    Ok(())
}

/// A picture this computer closed has no end from the host.
#[test]
fn a_picture_the_viewer_closed_has_no_host_end() -> TestResult {
    let trace = traced();
    let pair = Pair::for_desktop();
    assert_eq!(
        picture_ended_by(&pair, Ending::ViewerCloses)?,
        None,
        "the viewer's own close carries no reason from the host"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          session: session phase from=connecting generation=1 role=Controller session=1 to=active
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["service", "session"]),
    );
    Ok(())
}

/// Pictures a host publishes to a viewer's Desktop reach it in order,
/// keyframe and delta as published, a burst of them too. What the host
/// advertises follows its adapters: a capture alone offers no input, a
/// desktop adds it. Empty pictures and pictures for no Desktop are refused,
/// as are pictures once the viewer left.
#[test]
fn encoded_frames_reach_the_controller_in_order() -> TestResult {
    let trace = traced();
    // The kit's engines share their disk; this host offers only its screen.
    let pair = Pair::host_keeping_its_disk(
        SEEN_AND_CONTROLLED,
        Grants {
            view: false,
            control: false,
            clipboard: false,
            transfer: false,
        },
    );
    let host = &pair.host.core;
    host.set_capture(Box::new(MonitorCapture::new((1280, 720))))?;
    host.set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "testkit"))?;
    let capture_only = host.capabilities();
    assert!(
        capture_only.desktop_capture
            && !capture_only.keyboard
            && !capture_only.pointer
            && !capture_only.clipboard
            && !capture_only.file_transfer
            && !capture_only.audio
            && capture_only.monitors == 1,
        "a capture alone offers one monitor and nothing else: {capture_only:?}"
    );
    InputLog::install(&pair.host, (100, 100))?;
    let with_desktop = host.capabilities();
    assert!(
        with_desktop.desktop_capture
            && with_desktop.keyboard
            && with_desktop.pointer
            && with_desktop.monitors == 1,
        "a desktop adds keyboard and pointer: {with_desktop:?}"
    );

    let desktop = pair.controller.open_desktop(&pair.connect_services())?;
    let video = desktop.subscribe_video()?;
    let info = desktop.desktop_info()?;
    assert_eq!(
        (info.monitor_width, info.monitor_height),
        (1280, 720),
        "the viewer learns the capture's size, not the desktop's"
    );
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    let served = host.hosted_desktop_info(remote.id, remote.generation)?;
    assert_eq!(
        (served.monitor_width, served.monitor_height),
        (1280, 720),
        "the host serves the capture's size"
    );
    assert!(
        matches!(
            host.set_capture(Box::new(MonitorCapture::new((1, 1)))),
            Err(Error::Busy)
        ),
        "the capture is not swapped during a Desktop"
    );
    let publish =
        |frame: EncodedFrame| host.publish_desktop_video(remote.id, remote.generation, frame);
    publish(EncodedFrame::h264(true, 1, vec![1])?)?;
    publish(EncodedFrame::h264(false, 2, vec![2])?)?;
    let first = next_picture(&video, "the keyframe");
    assert_eq!(
        (first.keyframe, first.timestamp_us, first.payload),
        (true, 1, vec![1]),
        "the keyframe arrives first, as published"
    );
    let second = next_picture(&video, "the delta");
    assert_eq!(
        (second.keyframe, second.timestamp_us, second.payload),
        (false, 2, vec![2]),
        "the delta follows, as published"
    );
    for index in 0..30u64 {
        publish(EncodedFrame::h264(false, 100 + index, vec![3])?)?;
    }
    publish(EncodedFrame::h264(true, 999, vec![9, 9])?)?;
    // The sender may drain the burst or drop part of it; the core's
    // bounded-queue tests own overflow. The burst's keyframe arrives last.
    let last = wait_for("the burst's keyframe", Duration::from_secs(5), || {
        video
            .recv_timeout(Duration::from_millis(20))
            .filter(|frame| frame.timestamp_us == 999)
    });
    assert_eq!(
        (last.keyframe, last.payload),
        (true, vec![9, 9]),
        "the burst's keyframe arrives as published"
    );
    assert!(
        video.try_recv().is_none(),
        "nothing follows the last picture"
    );

    let mut empty = EncodedFrame::h264(true, 1, vec![1])?;
    empty.payload.clear();
    assert!(
        matches!(
            publish(empty),
            Err(Error::InvalidSetting("encoded frame is empty"))
        ),
        "an empty picture is refused"
    );
    assert!(
        matches!(
            host.publish_desktop_video(
                remote.id + 1,
                remote.generation,
                EncodedFrame::h264(true, 1, vec![1])?
            ),
            Err(Error::NoSession)
        ),
        "a picture for no Desktop is refused"
    );
    desktop.close();
    pair.host.wait_released(Some(ServiceKind::Desktop));
    assert!(
        publish(EncodedFrame::h264(true, 1, vec![1])?).is_err(),
        "a picture after the viewer left is refused"
    );
    vnilink_testkit::assert_story(
        expect![[r#"
        controller:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: OwnerClose
        host:
          service: service phase from=active generation=1 kind=Desktop service=1 to=closed: PeerClose
    "#]],
        || trace.story(&["service", "input"]),
    );
    Ok(())
}

/// A capture that reports no size leaves the desktop's size to advertise
/// and serve.
#[test]
fn a_zero_capture_size_falls_back_to_the_desktop() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    host.set_capture(Box::new(MonitorCapture::new((0, 0))))?;
    host.set_video_codecs(VideoCodecCaps::encode_only(4096, 4096, "testkit"))?;
    InputLog::install(&pair.host, (100, 80))?;
    let capabilities = host.capabilities();
    assert!(
        capabilities.desktop_capture && capabilities.keyboard && capabilities.monitors == 1,
        "the host still offers its one monitor: {capabilities:?}"
    );
    let desktop = pair.controller.open_desktop(&pair.connect_services())?;
    let info = desktop.desktop_info()?;
    assert_eq!(
        (info.monitor_width, info.monitor_height),
        (100, 80),
        "the viewer learns the desktop's size"
    );
    Ok(())
}

/// Publishes pictures on `remote` until `asked` holds, one each frame
/// interval, each a delta after `from`; gives the timestamp after the last.
fn publish_until(
    host: &Engine,
    remote: &ServiceInfo,
    from: u64,
    what: &str,
    mut asked: impl FnMut() -> bool,
) -> u64 {
    let mut paced = Paced::new();
    let mut timestamp = from;
    wait_for(what, DEFAULT_WAIT, || {
        if paced.due() {
            timestamp += 1;
            host.core
                .publish_desktop_video(
                    remote.id,
                    remote.generation,
                    vnilink_testkit::desktop::delta(timestamp, vec![5; 200]),
                )
                .expect("the host publishes");
        }
        asked().then_some(())
    });
    timestamp + 1
}

/// One picture lost at `point` after the first keyframe arrived: the host
/// is asked for a keyframe, which then reaches the viewer. Gives what the
/// point lost.
fn a_lost_picture_is_made_good(point: DropPoint) -> Result<usize, Box<dyn std::error::Error>> {
    let pair = Pair::for_desktop();
    let host = &pair.host.core;
    InputLog::install(&pair.host, SCREEN)?;
    let desktop = pair.controller.open_desktop(&pair.connect_services())?;
    let video = desktop.subscribe_video()?;
    let remote = pair.host.serving(ServiceKind::Desktop, &pair.controller);
    host.publish_desktop_video(remote.id, remote.generation, keyframe(1, vec![1; 200]))?;
    assert_eq!(
        next_picture(&video, "the first keyframe").timestamp_us,
        1,
        "the first keyframe arrives whole"
    );
    // What opening asked for is answered.
    host.take_desktop_keyframe_request(remote.id, remote.generation)?;
    let loses = if point == DropPoint::Gate {
        &pair.host
    } else {
        &pair.controller
    };
    let drops = SeededDrops::new().nth(point, &[1]).install(loses);
    let next = publish_until(
        &pair.host,
        &remote,
        1,
        "the host to be asked for a keyframe after the loss",
        || {
            host.take_desktop_keyframe_request(remote.id, remote.generation)
                .unwrap_or(false)
        },
    );
    host.publish_desktop_video(remote.id, remote.generation, keyframe(next, vec![9; 200]))?;
    wait_for("the keyframe after the loss", DEFAULT_WAIT, || {
        video
            .recv_timeout(Duration::from_millis(20))
            .filter(|picture| picture.keyframe && picture.timestamp_us == next)
    });
    Ok(drops.tally(point).lost)
}

/// A picture the host's send gate holds back asks for a keyframe of its
/// own: the pictures after it reference it.
#[test]
fn a_picture_held_back_at_the_send_gate_is_made_good_by_a_keyframe() -> TestResult {
    let _trace = traced();
    assert_eq!(
        a_lost_picture_is_made_good(DropPoint::Gate)?,
        1,
        "the gate held back one picture"
    );
    Ok(())
}

/// A picture whose datagram the network lost leaves a gap in the numbers
/// the viewer sees, and the viewer asks for a keyframe.
#[test]
fn a_picture_lost_on_the_network_is_made_good_by_a_keyframe() -> TestResult {
    let _trace = traced();
    assert_eq!(
        a_lost_picture_is_made_good(DropPoint::Datagram)?,
        1,
        "the network lost one datagram"
    );
    Ok(())
}

/// A picture lost as it reached the viewer's queue leaves a gap too, and
/// the viewer asks for a keyframe.
#[test]
fn a_picture_lost_before_the_viewers_queue_is_made_good_by_a_keyframe() -> TestResult {
    let _trace = traced();
    assert_eq!(
        a_lost_picture_is_made_good(DropPoint::Inbox)?,
        1,
        "the viewer's queue lost one picture"
    );
    Ok(())
}
