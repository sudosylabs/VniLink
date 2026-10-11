use crate::{audio::*, *};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn core() -> VnilinkCore {
    VnilinkCore {
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
        audio_coordinator: Arc::new(audio_native::AudioCoordinator::default()),
    }
}
fn config() -> FfiAudioConfig {
    FfiAudioConfig {
        device_id: "approved-mic".into(),
        kind: FfiAudioKind::Microphone,
        channels: 2,
        voice: false,
    }
}
fn encoded(encoder: &mut vnilink_audio::AudioEncoder) -> FfiAudioPacket {
    let p = encoder.encode(&[0.125; 1920]).unwrap();
    FfiAudioPacket {
        sequence: p.sequence,
        timestamp: p.timestamp,
        channels: 2,
        payload: p.payload,
    }
}
struct Provider {
    source: Arc<Mutex<Option<Arc<AudioSource>>>>,
    encoder: Arc<Mutex<vnilink_audio::AudioEncoder>>,
}
impl AudioProvider for Provider {
    fn supports(&self, kind: FfiAudioKind) -> bool {
        kind == FfiAudioKind::Microphone
    }
    fn devices(&self) -> Result<Vec<FfiAudioDevice>, FfiError> {
        Ok(vec![FfiAudioDevice {
            id: config().device_id,
            name: "Approved fixture microphone".into(),
            kind: FfiAudioKind::Microphone,
        }])
    }
    fn start_capture(
        &self,
        requested: FfiAudioConfig,
        startup: Arc<BackendStartup>,
    ) -> Result<Arc<AudioSource>, FfiError> {
        startup.check()?;
        let source = AudioSource::new(requested)?;
        source.publish(encoded(&mut self.encoder.lock().unwrap()))?;
        *self.source.lock().unwrap() = Some(source.clone());
        Ok(source)
    }
}
#[test]
fn ffi_audio_callback_format_mute_owner_and_capability_contract() {
    let host = core();
    let controller = core();
    host.accept_invitation(controller.create_invitation(60).unwrap())
        .unwrap();
    controller
        .accept_invitation(host.create_invitation(60).unwrap())
        .unwrap();
    host.start_hosting().unwrap();
    host.authorize_service(controller.endpoint_id(), FfiServiceKind::Microphone, true)
        .unwrap();
    let source = Arc::new(Mutex::new(None));
    let encoder = Arc::new(Mutex::new(
        vnilink_audio::AudioEncoder::new(vnilink_audio::Format::Stereo, false).unwrap(),
    ));
    host.set_audio_provider(Some(Box::new(Provider {
        source: source.clone(),
        encoder: encoder.clone(),
    })))
    .unwrap();
    let parent = controller
        .begin_connect_services(host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(parent.id).unwrap().phase == FfiSessionPhase::Active);
    let parent = controller.controller_session(parent.id).unwrap();
    let support = controller
        .remote_service_support(parent.id, parent.generation)
        .unwrap();
    assert!(support.microphone);
    assert!(!support.system_audio);
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, FfiServiceKind::Microphone)
        .unwrap();
    let audio = controller
        .open_audio_service(owner.id, owner.generation, config())
        .unwrap();
    assert_eq!(audio.info().id, owner.id);
    assert_eq!(audio.config(), config());
    // Cross-installation playback is rejected before any native device opens.
    assert!(host
        .start_native_audio_playback(audio.clone(), "invalid-device".into())
        .is_err());
    audio.set_muted(true);
    // Transport arrival uses a fixed playback clock. Scheduler delays in the
    // full suite must not exercise the deliberately supported stale-audio skip.
    let playback_start = Instant::now();
    wait(|| {
        assert!(audio.poll_internal_at(playback_start).unwrap().is_none());
        audio.playback_stats().received == 1
    });
    let muted_block = audio
        .poll_internal_at(playback_start + Duration::from_millis(60))
        .unwrap()
        .unwrap();
    assert_eq!(muted_block.timestamp, 0);
    assert_eq!(muted_block.samples.len(), 1920);
    assert!(muted_block.samples.iter().all(|&s| s == 0.0));
    let source = source.lock().unwrap().clone().unwrap();
    let next = encoded(&mut encoder.lock().unwrap());
    let mut wrong = next.clone();
    wrong.channels = 1;
    assert!(source.publish(wrong).is_err());
    audio.set_muted(false);
    source.publish(next).unwrap();
    wait(|| {
        assert!(audio
            .poll_internal_at(playback_start + Duration::from_millis(60))
            .unwrap()
            .is_none());
        audio.playback_stats().received == 2
    });
    let audible = audio
        .poll_internal_at(playback_start + Duration::from_millis(80))
        .unwrap()
        .unwrap();
    assert_eq!(audible.timestamp, 960);
    assert!(!audible.concealed);
    assert!(audible.samples.iter().any(|v| v.abs() > 0.01));
    assert_eq!(audio.playback_stats().received, 2);
    audio.close();
    wait(|| source.is_cancelled());
    assert!(audio.poll_playback().is_err());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        FfiSessionPhase::Active
    );
    host.shutdown();
    controller.shutdown();
}
