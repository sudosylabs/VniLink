//! Locally approved native sources and explicit receiver playback workers.
use crate::{
    audio::{AudioService, FfiAudioDevice, FfiAudioOutputDevice},
    error::map_error,
    FfiError, VnilinkCore,
};
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use vnilink::audio::{
    AudioBackend, AudioConfig, AudioDevice, AudioKind, AudioSource, HostAudioFactory,
};

#[derive(Default)]
pub(crate) struct AudioCoordinator {
    state: Mutex<AudioResources>,
}
#[derive(Default)]
struct AudioResources {
    captures: HashSet<String>,
    system: usize,
    playbacks: usize,
}
struct ResourceLease {
    coordinator: Arc<AudioCoordinator>,
    source: Option<String>,
    system: bool,
}
impl Drop for ResourceLease {
    fn drop(&mut self) {
        let mut state = self
            .coordinator
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(id) = self.source.take() {
            state.captures.remove(&id);
            if self.system {
                state.system -= 1;
            }
        } else {
            state.playbacks -= 1;
        }
    }
}
impl AudioCoordinator {
    fn capture(
        self: &Arc<Self>,
        id: &str,
        kind: AudioKind,
    ) -> Result<ResourceLease, vnilink::Error> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.captures.len() >= 8
            || state.captures.contains(id)
            || kind == AudioKind::System && state.playbacks > 0
        {
            return Err(vnilink::Error::Busy);
        }
        state.captures.insert(id.into());
        if kind == AudioKind::System {
            state.system += 1;
        }
        Ok(ResourceLease {
            coordinator: self.clone(),
            source: Some(id.into()),
            system: kind == AudioKind::System,
        })
    }
    fn playback(self: &Arc<Self>) -> Result<ResourceLease, vnilink::Error> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.system > 0 || state.playbacks >= 8 {
            return Err(vnilink::Error::Busy);
        }
        state.playbacks += 1;
        Ok(ResourceLease {
            coordinator: self.clone(),
            source: None,
            system: false,
        })
    }
}
fn microphone_permission() -> bool {
    #[cfg(target_os = "macos")]
    {
        vnilink_macos::microphone_authorized()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    } // Native WASAPI/PulseAudio still enforce OS/server device access.
}
/// Native sources are classified by the platform, never by a display name.
fn available_sources() -> Result<Vec<AudioDevice>, vnilink::Error> {
    let available = vnilink_audio::audio_devices()
        .map_err(|_| vnilink::Error::Unsupported("native audio enumeration"))?;
    let mut sources: Vec<_> = available
        .into_iter()
        .filter_map(|d| {
            let kind = match d.direction {
                vnilink_audio::DeviceDirection::Input => AudioKind::Microphone,
                vnilink_audio::DeviceDirection::Loopback => AudioKind::System,
                _ => return None,
            };
            Some(AudioDevice {
                id: d.id,
                name: d.name,
                kind,
            })
        })
        .collect();
    #[cfg(target_os = "macos")]
    sources.extend(vnilink_macos::system_audio_devices()?);
    // The mutable binding supports the Mac extension without cfg-specific API.
    sources.shrink_to_fit();
    Ok(sources)
}
enum NativeInput {
    Device(vnilink_audio::NativeCapture),
    #[cfg(target_os = "macos")]
    System(vnilink_macos::MacSystemAudio),
}
impl NativeInput {
    fn open(
        config: &AudioConfig,
        startup: &vnilink::BackendStartup,
    ) -> Result<Self, vnilink::Error> {
        #[cfg(target_os = "macos")]
        if config.kind == AudioKind::System {
            return vnilink_macos::MacSystemAudio::open(
                &config.device_id,
                config.format()?,
                startup,
            )
            .map(Self::System);
        }
        let direction = if config.kind == AudioKind::System {
            vnilink_audio::DeviceDirection::Loopback
        } else {
            vnilink_audio::DeviceDirection::Input
        };
        vnilink_audio::NativeCapture::open(&config.device_id, direction, config.format()?, false)
            .map(Self::Device)
            .map_err(|_| vnilink::Error::Unsupported("native audio capture"))
    }
    fn next_packet(&mut self) -> Result<Option<vnilink_audio::AudioPacket>, vnilink::Error> {
        match self {
            Self::Device(capture) => capture
                .next_packet()
                .map_err(|_| vnilink::Error::Unsupported("native audio capture ended")),
            #[cfg(target_os = "macos")]
            Self::System(capture) => capture.next_packet(),
        }
    }
}
struct NativeFactory {
    approved: Vec<AudioDevice>,
    coordinator: Arc<AudioCoordinator>,
}
impl HostAudioFactory for NativeFactory {
    fn supports(&self, kind: AudioKind) -> bool {
        self.approved.iter().any(|d| d.kind == kind)
    }
    fn devices(&self) -> Result<Vec<AudioDevice>, vnilink::Error> {
        let available = available_sources()?;
        Ok(self
            .approved
            .iter()
            .filter(|d| available.iter().any(|a| a.id == d.id && a.kind == d.kind))
            .cloned()
            .collect())
    }
    fn open(
        &self,
        config: AudioConfig,
        startup: vnilink::BackendStartup,
    ) -> Result<AudioBackend, vnilink::Error> {
        startup.check()?;
        if config.voice {
            return Err(vnilink::Error::Unsupported("voice audio echo cancellation"));
        }
        if !self
            .approved
            .iter()
            .any(|d| d.id == config.device_id && d.kind == config.kind)
        {
            return Err(vnilink::Error::Unauthorized);
        }
        if config.kind == AudioKind::Microphone && !microphone_permission() {
            return Err(vnilink::Error::Unsupported("microphone permission"));
        }
        // Display filters identify the same Mac system mix. Avoid duplicate
        // physical audio capture through two differently named display sources.
        #[cfg(target_os = "macos")]
        let resource_id = if config.kind == AudioKind::System {
            "sck-system-mix"
        } else {
            &config.device_id
        };
        #[cfg(not(target_os = "macos"))]
        let resource_id = &config.device_id;
        let resource = self.coordinator.capture(resource_id, config.kind)?;
        let source = AudioSource::new();
        let producer = source.clone();
        let requested = config.clone();
        let worker_startup = startup.clone();
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("vnilink-audio-capture".into())
            .spawn(move || {
                let _resource = resource;
                let run = || -> Result<(), vnilink::Error> {
                    worker_startup.check()?;
                    let mut capture = NativeInput::open(&requested, &worker_startup)?;
                    worker_startup.check()?;
                    let mut admitted = false;
                    let mut last = Instant::now();
                    while !producer.is_cancelled() {
                        if !admitted {
                            worker_startup.check()?;
                        }
                        match capture.next_packet()? {
                            Some(packet) => {
                                producer.publish(packet)?;
                                last = Instant::now();
                                if !admitted {
                                    admitted = true;
                                    let _ = ready.send(Ok(()));
                                }
                            }
                            None => {
                                if last.elapsed() > Duration::from_secs(3) {
                                    return Err(vnilink::Error::Timeout);
                                }
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    producer.fail();
                    let _ = ready.send(Err(error));
                }
            })
            .map_err(|_| vnilink::Error::Unsupported("audio worker"))?;
        loop {
            if let Err(error) = startup.check() {
                source.close();
                return Err(error);
            }
            match result.recv_timeout(Duration::from_millis(20)) {
                Ok(Ok(())) => break,
                Ok(Err(error)) => {
                    source.close();
                    return Err(error);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    source.close();
                    return Err(vnilink::Error::Unsupported("audio worker ended"));
                }
            }
        }
        if let Err(error) = startup.check() {
            source.close();
            return Err(error);
        }
        let backend = AudioBackend::new(config, source.clone());
        if backend.is_err() {
            source.close();
        }
        backend
    }
}

#[derive(uniffi::Object)]
pub struct NativeAudioPlayback {
    stop: Arc<AtomicBool>,
    service: Arc<AudioService>,
}
#[uniffi::export]
impl NativeAudioPlayback {
    pub fn close(&self) {
        self.stop.store(true, Ordering::Release);
        self.service.close();
    }
    pub fn is_cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire) || self.service.is_cancelled()
    }
}
impl Drop for NativeAudioPlayback {
    fn drop(&mut self) {
        self.close();
    }
}
#[uniffi::export]
impl VnilinkCore {
    pub fn native_audio_output_devices(&self) -> Result<Vec<FfiAudioOutputDevice>, FfiError> {
        let available = vnilink_audio::audio_devices()
            .map_err(|_| map_error(vnilink::Error::Unsupported("native audio enumeration")))?;
        Ok(available
            .into_iter()
            .filter(|d| d.direction == vnilink_audio::DeviceDirection::Output)
            .map(|d| FfiAudioOutputDevice {
                id: d.id,
                name: d.name,
            })
            .collect())
    }
    /// Registers explicit local approvals. No device activation, default-source
    /// fallback, peer grant or native permission prompt happens here.
    pub fn enable_native_audio(
        &self,
        microphone_ids: Vec<String>,
        system_source_ids: Vec<String>,
    ) -> Result<(), FfiError> {
        if microphone_ids.len() + system_source_ids.len() > 32 {
            return Err(map_error(vnilink::Error::InvalidSetting(
                "too many approved audio sources",
            )));
        }
        if !microphone_ids.is_empty() && !microphone_permission() {
            return Err(map_error(vnilink::Error::Unsupported(
                "microphone permission",
            )));
        }
        let available = available_sources().map_err(map_error)?;
        let mut approved = Vec::new();
        let mut ids = HashSet::new();
        for (kind, list) in [
            (AudioKind::Microphone, microphone_ids),
            (AudioKind::System, system_source_ids),
        ] {
            for id in list {
                if !ids.insert(id.clone()) {
                    return Err(map_error(vnilink::Error::InvalidSetting(
                        "duplicate audio source",
                    )));
                }
                let device = available
                    .iter()
                    .find(|d| d.id == id && d.kind == kind)
                    .ok_or_else(|| {
                        map_error(vnilink::Error::Unsupported("selected native audio source"))
                    })?;
                approved.push(AudioDevice {
                    id,
                    name: device.name.clone(),
                    kind,
                });
            }
        }
        let factory = if approved.is_empty() {
            None
        } else {
            Some(Arc::new(NativeFactory {
                approved,
                coordinator: self.audio_coordinator.clone(),
            }) as Arc<dyn HostAudioFactory>)
        };
        self.inner.set_audio_factory(factory).map_err(map_error)
    }
    pub fn native_audio_devices(&self) -> Result<Vec<FfiAudioDevice>, FfiError> {
        Ok(available_sources()
            .map_err(map_error)?
            .into_iter()
            .map(|d| FfiAudioDevice {
                id: d.id,
                name: d.name,
                kind: d.kind.into(),
            })
            .collect())
    }
    pub fn start_native_audio_playback(
        &self,
        service: Arc<AudioService>,
        output_id: String,
    ) -> Result<Arc<NativeAudioPlayback>, FfiError> {
        if service.is_cancelled() {
            return Err(map_error(vnilink::Error::NoSession));
        }
        if !Arc::ptr_eq(&self.inner, &service.core) {
            return Err(map_error(vnilink::Error::InvalidSetting(
                "audio belongs to another installation",
            )));
        }
        let resource = self.audio_coordinator.playback().map_err(map_error)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let player = service.clone();
        if player.playback_claimed.swap(true, Ordering::AcqRel) {
            return Err(map_error(vnilink::Error::Busy));
        }
        let (ready, result) = std::sync::mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("vnilink-audio-playback".into())
            .spawn(move || {
                let _resource = resource;
                let run = || -> Result<(), vnilink::Error> {
                    if player.is_cancelled() || worker_stop.load(Ordering::Acquire) {
                        return Err(vnilink::Error::NoSession);
                    }
                    let format = player.config().channels;
                    let mut output = vnilink_audio::NativeOutput::open(
                        &output_id,
                        if format == 1 {
                            vnilink_audio::Format::Mono
                        } else {
                            vnilink_audio::Format::Stereo
                        },
                    )
                    .map_err(|_| vnilink::Error::Unsupported("native audio playback"))?;
                    if player.is_cancelled() || worker_stop.load(Ordering::Acquire) {
                        return Err(vnilink::Error::NoSession);
                    }
                    let _ = ready.send(Ok(()));
                    while !worker_stop.load(Ordering::Acquire) && !player.is_cancelled() {
                        if let Some(block) = player
                            .poll_internal()
                            .map_err(|_| vnilink::Error::Unsupported("audio playback ended"))?
                        {
                            output.push(&block.samples).map_err(|_| {
                                vnilink::Error::Unsupported("native audio output ended")
                            })?;
                        } else {
                            std::thread::sleep(Duration::from_millis(2));
                        }
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    let _ = ready.send(Err(error));
                }
                worker_stop.store(true, Ordering::Release);
                player.close();
            });
        if spawned.is_err() {
            stop.store(true, Ordering::Release);
            service.playback_claimed.store(false, Ordering::Release);
            return Err(map_error(vnilink::Error::Unsupported("audio worker")));
        }
        match result.recv_timeout(Duration::from_secs(6)) {
            Ok(Ok(())) => Ok(Arc::new(NativeAudioPlayback { stop, service })),
            outcome => {
                stop.store(true, Ordering::Release);
                service.close();
                Err(map_error(match outcome {
                    Ok(Err(error)) => error,
                    _ => vnilink::Error::Timeout,
                }))
            }
        }
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;

    #[test]
    fn capture_limits_and_exclusivity_release_with_the_owner() {
        let coordinator = Arc::new(AudioCoordinator::default());
        let mut owners = Vec::new();
        for index in 0..8 {
            owners.push(
                coordinator
                    .capture(&format!("mic-{index}"), AudioKind::Microphone)
                    .unwrap(),
            );
        }
        assert!(matches!(
            coordinator.capture("overflow", AudioKind::Microphone),
            Err(vnilink::Error::Busy)
        ));
        assert!(matches!(
            coordinator.capture("mic-0", AudioKind::Microphone),
            Err(vnilink::Error::Busy)
        ));
        owners.remove(0);
        let replacement = coordinator.capture("mic-0", AudioKind::Microphone).unwrap();
        drop(owners);
        drop(replacement);
        let state = coordinator.state.lock().unwrap();
        assert!(state.captures.is_empty());
        assert_eq!(state.system, 0);
        assert_eq!(state.playbacks, 0);
    }

    #[test]
    fn system_capture_and_playback_prevent_feedback_in_both_orders() {
        let coordinator = Arc::new(AudioCoordinator::default());
        let microphone = coordinator.capture("mic", AudioKind::Microphone).unwrap();
        let system = coordinator.capture("monitor", AudioKind::System).unwrap();
        assert!(matches!(coordinator.playback(), Err(vnilink::Error::Busy)));
        drop(system);
        let playback = coordinator.playback().unwrap();
        assert!(matches!(
            coordinator.capture("monitor", AudioKind::System),
            Err(vnilink::Error::Busy)
        ));
        drop(playback);
        let system = coordinator.capture("monitor", AudioKind::System).unwrap();
        drop(system);
        // A microphone owner survives both transitions.
        assert!(matches!(
            coordinator.capture("mic", AudioKind::Microphone),
            Err(vnilink::Error::Busy)
        ));
        drop(microphone);
        assert!(coordinator.capture("mic", AudioKind::Microphone).is_ok());
    }

    #[test]
    fn playback_limit_returns_capacity_after_drop() {
        let coordinator = Arc::new(AudioCoordinator::default());
        let mut owners: Vec<_> = (0..8).map(|_| coordinator.playback().unwrap()).collect();
        assert!(matches!(coordinator.playback(), Err(vnilink::Error::Busy)));
        owners.pop();
        owners.push(coordinator.playback().unwrap());
        assert_eq!(coordinator.state.lock().unwrap().playbacks, 8);
        drop(owners);
        assert_eq!(coordinator.state.lock().unwrap().playbacks, 0);
    }
}
