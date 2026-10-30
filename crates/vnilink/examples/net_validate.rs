// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Real-network validation for one host and one controller.
//!
//! Boots two cores on the default public relay, pairs them through signed
//! invitations, links them, opens a Desktop service, streams synthetic video
//! on it for a soak period, and reports connect latency, the transport path
//! timeline (relay versus direct), round trips, and video delivery. The
//! frames are synthetic bytes, not a captured desktop: this exercises
//! pairing, the link, and the Desktop stream over the real network, not
//! capture or encode.
//!
//! With a restart second, the controller core is dropped mid-soak and a new
//! core reopens the same installation store, then links and opens its
//! Desktop again. The report shows identity preservation, reconnect latency,
//! the host generation step, and old-generation fencing.
//!
//! Run: `cargo run -p vnilink --example net_validate [soak_secs] [fps] [relay_url] [payload_bytes] [restart_at_secs]`
//! Defaults: 60 seconds at 10 frames per second on the public relay with
//! 64-byte frames and no restart. With a relay URL both cores use it as a
//! custom relay instead.

use std::sync::Arc;
use std::time::{Duration, Instant};

use vnilink::{
    Core, CoreConfig, CustomRelay, EncodedFrame, Grants, HostCapture, MemoryStore, OpenConfig,
    RelaySettings, RemoteDesktopService, ServiceKind, SessionInfo, SessionPath, SessionPhase,
    VideoSubscription,
};

/// The monitor the host reports. Nothing is captured: the frames are synthetic.
struct SyntheticMonitor;

impl HostCapture for SyntheticMonitor {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let soak_secs: u64 = args.get(1).and_then(|arg| arg.parse().ok()).unwrap_or(60);
    let fps: u64 = args.get(2).and_then(|arg| arg.parse().ok()).unwrap_or(10);
    let payload_bytes: usize = args.get(4).and_then(|arg| arg.parse().ok()).unwrap_or(64);
    let restart_at: u64 = args.get(5).and_then(|arg| arg.parse().ok()).unwrap_or(0);
    assert!((1..=600).contains(&soak_secs), "soak 1..=600 seconds");
    assert!((1..=30).contains(&fps), "fps 1..=30");
    assert!(
        (1..=512 * 1024).contains(&payload_bytes),
        "payload 1..=512KiB"
    );
    assert!(
        restart_at == 0 || (5..soak_secs).contains(&restart_at),
        "restart inside the soak"
    );

    let relay = args.get(3).map(|url| {
        RelaySettings::custom(vec![CustomRelay::new(url).expect("relay URL")]).expect("relay")
    });
    if let Some(url) = args.get(3) {
        println!("relay_mode=custom {url}");
    } else {
        println!("relay_mode=public");
    }

    let host = Core::start(host_config(&relay)).expect("host starts");
    // Synthetic frames bypass capture and encode, but admission still
    // requires a monitor and the negotiated codec flags.
    host.set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("host flags");
    host.set_capture(Box::new(SyntheticMonitor))
        .expect("host monitor");
    let controller_store: Arc<dyn vnilink::InstallationStore> = Arc::new(MemoryStore::empty());
    let mut controller = Core::open(
        Arc::clone(&controller_store),
        controller_open_config(&relay),
    )
    .expect("controller opens");
    controller
        .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("controller flags");

    let ttl = Duration::from_secs(15 * 60);
    let token = host.create_invitation(ttl).expect("invitation");
    controller.accept_invitation(&token).expect("paired");
    let back = controller.create_invitation(ttl).expect("invitation");
    host.accept_invitation(&back).expect("paired");
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .expect("authorized");
    host.start_hosting().expect("hosting");

    let started = Instant::now();
    let (mut session, mut desktop, mut video) = open_desktop(&controller, &host);
    let connect_ms = started.elapsed().as_millis();
    let mut hosted = wait_hosted(&host, None);

    let mut published = 0u64;
    let mut received = 0u64;
    let mut keyframes = 0u64;
    let mut keyframe_requests = 0u64;
    let mut relay_seen = false;
    let mut direct_selected = false;
    let mut rtts: Vec<u64> = Vec::new();
    let mut timeline: Vec<(u64, Vec<SessionPath>)> = Vec::new();
    let mut first_bytes: Option<(u64, u64)> = None;
    let mut last_bytes = (0u64, 0u64);
    let mut restarted = restart_at == 0;
    let mut reconnect_ms = 0u128;
    let mut generations = (0u64, 0u64);
    let mut stale_rejected = false;

    let interval = Duration::from_micros(1_000_000 / fps);
    let mut next_frame = Instant::now();
    let mut next_sample = Instant::now();
    let end = Instant::now() + Duration::from_secs(soak_secs);
    let mut index = 0u64;
    while Instant::now() < end {
        if !restarted && started.elapsed() >= Duration::from_secs(restart_at) {
            restarted = true;
            let old = hosted;
            let old_host = host
                .snapshot()
                .unwrap()
                .incoming
                .map(|s| (s.id, s.generation));
            let controller_id = controller.endpoint_id();
            drop(video);
            drop(desktop);
            drop(controller);
            std::thread::sleep(Duration::from_secs(3));
            controller = Core::open(
                Arc::clone(&controller_store),
                controller_open_config(&relay),
            )
            .expect("controller reopens");
            controller
                .set_video_codecs(vnilink::VideoCodecCaps::h264_full(4096, 4096, "test"))
                .expect("controller flags");
            let identity_preserved = controller.endpoint_id() == controller_id;
            println!("identity_preserved={identity_preserved}");
            let again = Instant::now();
            (session, desktop, video) = open_desktop(&controller, &host);
            reconnect_ms = again.elapsed().as_millis();
            hosted = wait_hosted(&host, Some(old.0));
            let new_host = host
                .snapshot()
                .unwrap()
                .incoming
                .map(|s| (s.id, s.generation));
            generations = (
                old_host.map(|host| host.1).unwrap_or(0),
                new_host.map(|host| host.1).unwrap_or(0),
            );
            // The old Desktop's pictures are refused once its viewer left.
            stale_rejected = matches!(
                host.publish_desktop_video(
                    old.0,
                    old.1,
                    EncodedFrame::h264(true, 0, vec![0x42u8; payload_bytes]).unwrap(),
                ),
                Err(vnilink::Error::NoSession | vnilink::Error::StaleSession)
            );
            next_frame = Instant::now();
            next_sample = Instant::now();
        }
        let keyframe = index.is_multiple_of(30);
        let payload = vec![0x42u8; payload_bytes];
        match host.publish_desktop_video(
            hosted.0,
            hosted.1,
            EncodedFrame::h264(keyframe, index * (1_000_000 / fps), payload).unwrap(),
        ) {
            Ok(()) => published += 1,
            Err(error) => {
                println!("publish stopped: {error}");
                break;
            }
        }
        index += 1;
        if host
            .take_desktop_keyframe_request(hosted.0, hosted.1)
            .unwrap_or(false)
        {
            keyframe_requests += 1;
        }
        while let Some(frame) = video.try_recv() {
            received += 1;
            if frame.keyframe {
                keyframes += 1;
            }
        }
        if Instant::now() >= next_sample {
            next_sample = Instant::now() + Duration::from_secs(5);
            if let Ok(paths) = controller.session_paths(session.id) {
                for path in &paths {
                    relay_seen |= path.relay;
                    direct_selected |= path.selected && !path.relay;
                    rtts.push(path.rtt_ms);
                }
                let totals = paths.iter().fold((0u64, 0u64), |(tx, rx), path| {
                    (tx + path.tx_bytes, rx + path.rx_bytes)
                });
                if first_bytes.is_none() {
                    first_bytes = Some(totals);
                }
                last_bytes = totals;
                timeline.push((started.elapsed().as_secs(), paths));
            }
        }
        next_frame += interval;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        }
    }
    while let Some(frame) = video.try_recv() {
        received += 1;
        if frame.keyframe {
            keyframes += 1;
        }
    }

    println!("connect_ms={connect_ms}");
    println!("published={published} received={received} keyframes={keyframes}");
    println!("keyframe_requests={keyframe_requests}");
    println!(
        "loss_pct={:.1}",
        100.0 * (published.saturating_sub(received)) as f64 / published.max(1) as f64
    );
    println!("relay_path_seen={relay_seen} direct_selected={direct_selected}");
    if let Some((first_tx, first_rx)) = first_bytes {
        let secs = started.elapsed().as_secs_f64().max(1.0);
        println!(
            "net_bytes_tx={} net_bytes_rx={} net_mbps={:.1}",
            last_bytes.0.saturating_sub(first_tx),
            last_bytes.1.saturating_sub(first_rx),
            8.0 * (last_bytes.0.saturating_sub(first_tx) + last_bytes.1.saturating_sub(first_rx))
                as f64
                / secs
                / 1_000_000.0
        );
    }
    if restarted && restart_at != 0 {
        println!("reconnect_ms={reconnect_ms}");
        println!(
            "host_generation={}->{} stale_rejected={stale_rejected}",
            generations.0, generations.1
        );
    }
    if !rtts.is_empty() {
        rtts.sort_unstable();
        println!(
            "rtt_ms min={} p50={} max={}",
            rtts[0],
            rtts[rtts.len() / 2],
            rtts[rtts.len() - 1]
        );
    }
    for (secs, paths) in timeline {
        println!("t={secs}s {paths:?}");
    }
    drop(video);
    drop(desktop);
}

/// Links `controller` to `host` and opens a Desktop service on the link,
/// as the application does: the link, the Desktop and its pictures.
fn open_desktop(
    controller: &Core,
    host: &Core,
) -> (SessionInfo, RemoteDesktopService, VideoSubscription) {
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .expect("linked");
    let deadline = Instant::now() + Duration::from_secs(75);
    let session = loop {
        let session = controller.controller_session(pending.id).expect("the link");
        if session.phase == SessionPhase::Active {
            break session;
        }
        assert!(
            Instant::now() < deadline,
            "the link never became active: {:?}",
            session.phase
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let owner = controller
        .attach_controller_service(session.id, session.generation, ServiceKind::Desktop)
        .expect("a Desktop owner");
    let desktop = controller
        .open_desktop_service(owner.id, owner.generation)
        .expect("the Desktop service");
    let video = desktop.subscribe_video().expect("video subscription");
    (session, desktop, video)
}

fn host_config(relay: &Option<RelaySettings>) -> CoreConfig {
    let mut config = CoreConfig::default();
    if let Some(relay) = relay {
        config.relay = relay.clone();
    }
    config
}

fn controller_open_config(relay: &Option<RelaySettings>) -> OpenConfig {
    let mut config = OpenConfig::default();
    if let Some(relay) = relay {
        config.initial_relay = relay.clone();
    }
    config
}

/// The host's Desktop service, other than `exclude_id`: its id and generation.
fn wait_hosted(host: &Core, exclude_id: Option<u64>) -> (u64, u64) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(service) = host.hosted_services().unwrap().into_iter().find(|service| {
            service.kind == ServiceKind::Desktop
                && service.phase == vnilink::ServicePhase::Active
                && Some(service.id) != exclude_id
        }) {
            return (service.id, service.generation);
        }
        if Instant::now() > deadline {
            panic!("host never reported the Desktop service");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
