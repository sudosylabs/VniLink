use super::support::{pair_for_control, start_direct};
use crate::{audio::*, Core, Error, ServiceKind, SessionPhase};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(8);
    while !condition() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn config(kind: AudioKind) -> AudioConfig {
    AudioConfig {
        device_id: if kind == AudioKind::System {
            "approved-system"
        } else {
            "approved-mic"
        }
        .into(),
        kind,
        channels: 2,
        voice: false,
    }
}
#[derive(Default)]
struct Factory {
    sources: Mutex<Vec<(AudioKind, Arc<AudioSource>)>>,
}
impl HostAudioFactory for Factory {
    fn supports(&self, _: AudioKind) -> bool {
        true
    }
    fn devices(&self) -> Result<Vec<AudioDevice>, Error> {
        Ok([AudioKind::System, AudioKind::Microphone]
            .into_iter()
            .map(|kind| AudioDevice {
                id: config(kind).device_id,
                name: "Test source".into(),
                kind,
            })
            .collect())
    }
    fn open(
        &self,
        config: AudioConfig,
        startup: crate::BackendStartup,
    ) -> Result<AudioBackend, Error> {
        startup.check()?;
        let source = AudioSource::new();
        let mut encoder = vnilink_audio::AudioEncoder::new(config.format()?, false).unwrap();
        source.publish(encoder.encode(&[0.125; 1920]).unwrap())?;
        self.sources
            .lock()
            .unwrap()
            .push((config.kind, source.clone()));
        AudioBackend::new(config, source)
    }
}
fn connect(controller: &Core, host: &Core) -> crate::SessionInfo {
    let parent = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    wait(|| controller.controller_session(parent.id).unwrap().phase == SessionPhase::Active);
    controller.controller_session(parent.id).unwrap()
}
fn packet(encoder: &mut vnilink_audio::AudioEncoder) -> AudioPacket {
    encoder.encode(&[0.125; 1920]).unwrap()
}
#[test]
fn independent_audio_grants_transport_real_opus_and_revoke_only_matching_source() {
    let controller = start_direct();
    let host = start_direct();
    pair_for_control(&host, &controller);
    let factory = Arc::new(Factory::default());
    host.set_audio_factory(Some(factory.clone())).unwrap();
    let parent = connect(&controller, &host);
    let support = controller
        .remote_service_support(parent.id, parent.generation)
        .unwrap();
    assert!(support.system_audio && support.microphone);
    assert!(matches!(
        controller.remote_audio_devices(parent.id, parent.generation, AudioKind::System),
        Err(Error::Unauthorized)
    ));
    host.set_service_sharing(ServiceKind::SystemAudio, true)
        .unwrap();
    assert!(
        !host
            .service_permissions(&controller.endpoint_id())
            .unwrap()
            .microphone
    );
    assert!(matches!(
        controller.remote_audio_devices(parent.id, parent.generation, AudioKind::Microphone),
        Err(Error::Unauthorized)
    ));
    let devices = controller
        .remote_audio_devices(parent.id, parent.generation, AudioKind::System)
        .unwrap();
    assert_eq!(devices, vec![factory.devices().unwrap()[0].clone()]);
    host.set_service_sharing(ServiceKind::Microphone, true)
        .unwrap();
    let files = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::SystemAudio)
        .unwrap();
    assert!(matches!(
        controller.open_audio_service(owner.id, owner.generation, config(AudioKind::Microphone)),
        Err(Error::InvalidSetting(_))
    ));
    let system = controller
        .open_audio_service(owner.id, owner.generation, config(AudioKind::System))
        .unwrap();
    assert!(matches!(
        controller.open_audio_service(owner.id, owner.generation, config(AudioKind::System)),
        Err(Error::Busy)
    ));
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Microphone)
        .unwrap();
    let mic = controller
        .open_audio_service(owner.id, owner.generation, config(AudioKind::Microphone))
        .unwrap();
    wait(|| system.try_recv().is_some());
    wait(|| mic.try_recv().is_some());
    let sources = factory.sources.lock().unwrap().clone();
    let mut encoder = vnilink_audio::AudioEncoder::new(AudioFormat::Stereo, false).unwrap();
    let expected = packet(&mut encoder);
    sources[0].1.publish(expected.clone()).unwrap();
    let mut received = None;
    wait(|| {
        received = system.try_recv();
        received.is_some()
    });
    assert_eq!(received.take().unwrap(), expected);
    host.set_service_sharing(ServiceKind::Microphone, false)
        .unwrap();
    wait(|| sources[1].1.is_cancelled() && mic.is_cancelled());
    assert!(!sources[0].1.is_cancelled());
    assert!(!files.is_cancelled());
    let expected = packet(&mut encoder);
    sources[0].1.publish(expected.clone()).unwrap();
    wait(|| {
        received = system.try_recv();
        received.is_some()
    });
    assert_eq!(received.take().unwrap(), expected);
    system.close();
    wait(|| sources[0].1.is_cancelled());
    assert!(!files.is_cancelled());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    controller.shutdown();
    host.shutdown();
}

#[test]
fn source_queue_is_bounded_and_closed_backend_cannot_be_reclaimed() {
    let source = AudioSource::new();
    let mut initial = vnilink_audio::AudioEncoder::new(AudioFormat::Stereo, false).unwrap();
    source.publish(packet(&mut initial)).unwrap();
    let backend = AudioBackend::new(config(AudioKind::System), source.clone()).unwrap();
    assert!(matches!(
        AudioBackend::new(config(AudioKind::System), source.clone()),
        Err(Error::Busy)
    ));
    let mut encoder = vnilink_audio::AudioEncoder::new(AudioFormat::Stereo, false).unwrap();
    for _ in 0..40 {
        source.publish(packet(&mut encoder)).unwrap();
    }
    for sequence in 32..40 {
        assert_eq!(source.pop().unwrap().sequence, sequence);
    }
    assert!(source.pop().is_none());
    drop(backend);
    assert!(source.is_cancelled());
    assert!(matches!(
        source.publish(packet(&mut encoder)),
        Err(Error::NoSession)
    ));
    assert!(matches!(
        AudioBackend::new(config(AudioKind::System), source),
        Err(Error::NoSession)
    ));
}

#[test]
fn cancelled_audio_startup_releases_late_backend_without_ending_files() {
    struct Delayed {
        entered: std::sync::mpsc::SyncSender<crate::BackendStartup>,
        release: Mutex<std::sync::mpsc::Receiver<()>>,
        source: Arc<AudioSource>,
    }
    impl HostAudioFactory for Delayed {
        fn supports(&self, kind: AudioKind) -> bool {
            kind == AudioKind::Microphone
        }
        fn devices(&self) -> Result<Vec<AudioDevice>, Error> {
            Ok(vec![AudioDevice {
                id: config(AudioKind::Microphone).device_id,
                name: "Delayed source".into(),
                kind: AudioKind::Microphone,
            }])
        }
        fn open(
            &self,
            config: AudioConfig,
            startup: crate::BackendStartup,
        ) -> Result<AudioBackend, Error> {
            self.entered.send(startup).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(8))
                .unwrap();
            // Simulate an OS call that succeeds after its owner was revoked.
            // The outer admission path must reject and Drop this late resource.
            AudioBackend::new(config, self.source.clone())
        }
    }
    let controller = Arc::new(start_direct());
    let host = start_direct();
    pair_for_control(&host, &controller);
    let (entered, entering) = std::sync::mpsc::sync_channel(1);
    let (release, released) = std::sync::mpsc::sync_channel(1);
    let source = AudioSource::new();
    let mut encoder = vnilink_audio::AudioEncoder::new(AudioFormat::Stereo, false).unwrap();
    source.publish(packet(&mut encoder)).unwrap();
    host.set_audio_factory(Some(Arc::new(Delayed {
        entered,
        release: Mutex::new(released),
        source: source.clone(),
    })))
    .unwrap();
    host.set_service_sharing(ServiceKind::Microphone, true)
        .unwrap();
    let parent = connect(&controller, &host);
    let files = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Files)
        .unwrap();
    let files = controller
        .service_lease(files.id, files.generation)
        .unwrap();
    let owner = controller
        .attach_controller_service(parent.id, parent.generation, ServiceKind::Microphone)
        .unwrap();
    let opening_core = controller.clone();
    let opening = std::thread::spawn(move || {
        opening_core.open_audio_service(owner.id, owner.generation, config(AudioKind::Microphone))
    });
    let startup = entering.recv_timeout(Duration::from_secs(8)).unwrap();
    host.set_service_sharing(ServiceKind::Microphone, false)
        .unwrap();
    wait(|| matches!(startup.check(), Err(Error::NoSession)));
    release.send(()).unwrap();
    assert!(opening.join().unwrap().is_err());
    wait(|| source.is_cancelled());
    assert!(!files.is_cancelled());
    assert_eq!(
        controller.controller_session(parent.id).unwrap().phase,
        SessionPhase::Active
    );
    controller.shutdown();
    host.shutdown();
}
