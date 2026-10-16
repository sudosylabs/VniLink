use super::*;
use crate::{
    AudioProvider, AudioSource, BackendStartup, FfiAudioDevice, FfiAudioPacket, FfiSessionPhase,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until, "desktop audio did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn core() -> Arc<VnilinkCore> {
    Arc::new(VnilinkCore {
        inner: Arc::new(
            vnilink::Core::start(vnilink::CoreConfig {
                listen: vnilink::Listen::Loopback,
                relay: vnilink::RelaySettings::custom(vec![vnilink::CustomRelay::new(
                    "http://127.0.0.1:9",
                )
                .unwrap()])
                .unwrap(),
                ..Default::default()
            })
            .unwrap(),
        ),
        previews: Mutex::new(Default::default()),
        next_preview: AtomicU64::new(1),
        audio_coordinator: Arc::new(crate::audio_native::AudioCoordinator::default()),
    })
}
struct Capture;
impl vnilink::HostCapture for Capture {
    fn monitor(&self) -> (u16, u16) {
        (1920, 1080)
    }
}
struct Provider(Arc<AtomicU64>);
impl AudioProvider for Provider {
    fn supports(&self, kind: FfiAudioKind) -> bool {
        kind == FfiAudioKind::System
    }
    fn devices(&self) -> Result<Vec<FfiAudioDevice>, FfiError> {
        Ok(vec![FfiAudioDevice {
            id: "vnilink:default-system-audio".into(),
            name: "System mix".into(),
            kind: FfiAudioKind::System,
        }])
    }
    fn start_capture(
        &self,
        config: FfiAudioConfig,
        startup: Arc<BackendStartup>,
    ) -> Result<Arc<AudioSource>, FfiError> {
        startup.check()?;
        assert_eq!(config.kind, FfiAudioKind::System);
        self.0.fetch_add(1, Ordering::AcqRel);
        let source = AudioSource::new(config)?;
        let packet = vnilink_audio::AudioEncoder::new(vnilink_audio::Format::Stereo, false)
            .unwrap()
            .encode(&[0.125; 1920])
            .unwrap();
        source.publish(FfiAudioPacket {
            sequence: packet.sequence,
            timestamp: packet.timestamp,
            channels: 2,
            payload: packet.payload,
        })?;
        Ok(source)
    }
}
struct Player(Arc<AudioService>);
impl Playback for Player {
    fn close(&self) {
        self.0.close();
    }
    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    fn error(&self) -> Option<FfiError> {
        None
    }
}
struct Fixture {
    host: Arc<VnilinkCore>,
    controller: Arc<VnilinkCore>,
    desktop: Arc<DesktopService>,
    captures: Arc<AtomicU64>,
}
impl Fixture {
    fn new() -> Self {
        let host = core();
        let controller = core();
        let captures = Arc::new(AtomicU64::new(0));
        host.set_audio_provider(Some(Box::new(Provider(captures.clone()))))
            .unwrap();
        host.set_service_sharing(FfiServiceKind::SystemAudio, true)
            .unwrap();
        let caps = vnilink::VideoCodecCaps::h264_full(4096, 4096, "test");
        host.set_video_codecs(caps.clone().into()).unwrap();
        controller.set_video_codecs(caps.into()).unwrap();
        host.inner.set_capture(Box::new(Capture)).unwrap();
        host.set_desktop_sharing(true, false, false, false).unwrap();
        controller
            .accept_invitation(host.create_invitation(60).unwrap())
            .unwrap();
        host.accept_invitation(controller.create_invitation(60).unwrap())
            .unwrap();
        host.start_hosting().unwrap();
        let parent = controller
            .begin_connect_services(host.endpoint_id())
            .unwrap();
        wait(|| controller.controller_session(parent.id).unwrap().phase == FfiSessionPhase::Active);
        let parent = controller.controller_session(parent.id).unwrap();
        let owner = controller
            .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Desktop)
            .unwrap();
        let desktop = controller
            .open_desktop_service(owner.id, owner.generation)
            .unwrap();
        Self {
            host,
            controller,
            desktop,
            captures,
        }
    }
    fn start(&self, opened: Arc<Mutex<Vec<Arc<AudioService>>>>) -> Arc<DesktopAudio> {
        DesktopAudio::start_with_playback(
            self.controller.clone(),
            &self.desktop,
            Arc::new(move |_, audio| {
                lock(&opened).push(audio.clone());
                Ok(Arc::new(Player(audio)))
            }),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.desktop.close();
        self.controller.shutdown();
        self.host.shutdown();
    }
}
#[test]
fn desktop_system_audio_mute_disable_retry_and_revocation_are_independent_of_video() {
    let f = Fixture::new();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let companion = f.start(opened.clone());
    wait(|| companion.status().phase == DesktopAudioPhase::Playing);
    assert_eq!(f.captures.load(Ordering::Acquire), 1);
    let first = lock(&opened)[0].clone();
    companion.set_muted(true);
    assert_eq!(companion.status().phase, DesktopAudioPhase::Muted);
    // Exercise the actual Opus transport and jitter decoder while muted.
    let start = Instant::now();
    wait(|| {
        assert!(first.poll_internal_at(start).unwrap().is_none());
        first.playback_stats().received == 1
    });
    let block = first
        .poll_internal_at(start + Duration::from_millis(60))
        .unwrap()
        .unwrap();
    assert!(block.samples.iter().all(|sample| *sample == 0.0));
    companion.set_enabled(false);
    assert!(first.is_cancelled());
    assert!(!f.desktop.is_cancelled());
    companion.set_enabled(true);
    wait(|| companion.status().phase == DesktopAudioPhase::Muted);
    assert_eq!(f.captures.load(Ordering::Acquire), 2);
    let second = lock(&opened)[1].clone();
    assert_ne!(first.info().id, second.info().id);
    companion.set_muted(false);
    assert_eq!(companion.status().phase, DesktopAudioPhase::Playing);
    f.host
        .set_service_sharing(FfiServiceKind::SystemAudio, false)
        .unwrap();
    wait(|| companion.status().phase == DesktopAudioPhase::Unavailable);
    assert!(second.is_cancelled());
    assert!(!f.desktop.is_cancelled());
    f.host
        .set_service_sharing(FfiServiceKind::SystemAudio, true)
        .unwrap();
    assert_eq!(companion.status().phase, DesktopAudioPhase::Unavailable);
    companion.set_enabled(true);
    wait(|| companion.status().phase == DesktopAudioPhase::Playing);
    let third = lock(&opened)[2].clone();
    f.desktop.close();
    wait(|| companion.status().phase == DesktopAudioPhase::Stopped);
    assert!(third.is_cancelled());
    let info = f.desktop.info();
    assert_eq!(
        f.controller
            .controller_session(info.connection_id)
            .unwrap()
            .phase,
        FfiSessionPhase::Active
    );
}
#[test]
fn desktop_audio_permission_and_output_failures_leave_desktop_running() {
    let f = Fixture::new();
    f.host
        .set_service_sharing(FfiServiceKind::SystemAudio, false)
        .unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let companion = f.start(opened.clone());
    wait(|| companion.status().phase == DesktopAudioPhase::Unavailable);
    assert!(lock(&opened).is_empty());
    assert_eq!(f.captures.load(Ordering::Acquire), 0);
    assert!(!f.desktop.is_cancelled());
    companion.close();
    f.host
        .set_service_sharing(FfiServiceKind::SystemAudio, true)
        .unwrap();
    let failed = Arc::new(Mutex::new(None));
    let output = failed.clone();
    let companion = DesktopAudio::start_with_playback(
        f.controller.clone(),
        &f.desktop,
        Arc::new(move |_, audio| {
            *lock(&output) = Some(audio);
            Err(FfiError::Unsupported {
                message: "fixture output unavailable".into(),
            })
        }),
    );
    wait(|| companion.status().phase == DesktopAudioPhase::Unavailable);
    assert!(lock(&failed).as_ref().unwrap().is_cancelled());
    assert!(!f.desktop.is_cancelled());
    assert!(matches!(
        companion.status().error,
        Some(FfiError::Unsupported { .. })
    ));
}
#[test]
fn closing_during_playback_startup_retires_the_late_result() {
    for close_desktop in [false, true] {
        let f = Fixture::new();
        let (ready, entered) = std::sync::mpsc::sync_channel(1);
        let (release, gate) = std::sync::mpsc::sync_channel(1);
        let gate = Mutex::new(gate);
        let late = Arc::new(Mutex::new(None));
        let output = late.clone();
        let companion = DesktopAudio::start_with_playback(
            f.controller.clone(),
            &f.desktop,
            Arc::new(move |_, audio| {
                *lock(&output) = Some(audio.clone());
                ready.send(()).unwrap();
                lock(&gate).recv_timeout(Duration::from_secs(8)).unwrap();
                Ok(Arc::new(Player(audio)))
            }),
        );
        entered.recv_timeout(Duration::from_secs(8)).unwrap();
        if close_desktop {
            f.desktop.close();
        } else {
            companion.close();
            assert!(lock(&late).as_ref().unwrap().is_cancelled());
        }
        release.send(()).unwrap();
        wait(|| Arc::strong_count(lock(&late).as_ref().unwrap()) == 1);
        wait(|| companion.status().phase == DesktopAudioPhase::Stopped);
        assert!(lock(&late).as_ref().unwrap().is_cancelled());
        assert_eq!(f.desktop.is_cancelled(), close_desktop);
    }
}

#[test]
fn repeated_audio_restarts_release_private_service_records() {
    let f = Fixture::new();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let companion = f.start(opened);
    for _ in 0..=vnilink::MAX_SERVICE_RECORDS {
        wait(|| companion.status().phase == DesktopAudioPhase::Playing);
        companion.set_enabled(false);
        assert_eq!(
            f.controller.controller_services().unwrap().len(),
            1,
            "only the desktop record should remain"
        );
        companion.set_enabled(true);
    }
    companion.close();
    assert_eq!(f.controller.controller_services().unwrap().len(), 1);
    assert!(!f.desktop.is_cancelled());
}

#[test]
fn silent_system_audio_stays_connected_without_fabricating_packets() {
    let f = Fixture::new();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let companion = f.start(opened.clone());
    wait(|| companion.status().phase == DesktopAudioPhase::Playing);
    let audio = lock(&opened)[0].clone();
    let start = Instant::now();
    wait(|| {
        let _ = audio.poll_internal_at(start).unwrap();
        audio.playback_stats().received == 1
    });
    // Exceeds the transport's three-second packet timeout. Idle keepalives
    // preserve liveness, but never become invented encoded/decoded frames.
    std::thread::sleep(Duration::from_millis(3300));
    assert!(!audio.is_cancelled());
    assert_eq!(companion.status().phase, DesktopAudioPhase::Playing);
    assert_eq!(audio.playback_stats().received, 1);
    companion.close();
}
