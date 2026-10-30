//! Nonvisual source and playback contracts. Native UI remains deferred.
use crate::{error::map_error, BackendStartup, FfiError, FfiServiceInfo, VnilinkCore};
use std::sync::{Arc, Mutex};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfiAudioKind {
    System,
    Microphone,
}
impl From<FfiAudioKind> for vnilink::audio::AudioKind {
    fn from(k: FfiAudioKind) -> Self {
        match k {
            FfiAudioKind::System => Self::System,
            FfiAudioKind::Microphone => Self::Microphone,
        }
    }
}
impl From<vnilink::audio::AudioKind> for FfiAudioKind {
    fn from(k: vnilink::audio::AudioKind) -> Self {
        match k {
            vnilink::audio::AudioKind::System => Self::System,
            vnilink::audio::AudioKind::Microphone => Self::Microphone,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAudioDevice {
    pub id: String,
    pub name: String,
    pub kind: FfiAudioKind,
}
/// A local playback destination, independent of the remote capture kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAudioOutputDevice {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAudioConfig {
    pub device_id: String,
    pub kind: FfiAudioKind,
    pub channels: u8,
    pub voice: bool,
}
impl From<FfiAudioConfig> for vnilink::audio::AudioConfig {
    fn from(c: FfiAudioConfig) -> Self {
        Self {
            device_id: c.device_id,
            kind: c.kind.into(),
            channels: c.channels,
            voice: c.voice,
        }
    }
}
impl From<vnilink::audio::AudioConfig> for FfiAudioConfig {
    fn from(c: vnilink::audio::AudioConfig) -> Self {
        Self {
            device_id: c.device_id,
            kind: c.kind.into(),
            channels: c.channels,
            voice: c.voice,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAudioPacket {
    pub sequence: u64,
    pub timestamp: u64,
    pub channels: u8,
    pub payload: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct FfiAudioBlock {
    pub timestamp: u64,
    pub samples: Vec<f32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfiAudioPlaybackStats {
    pub received: u64,
    pub late: u64,
    pub duplicate: u64,
    pub overflow: u64,
    pub discontinuities: u64,
    pub queued: u32,
}

pub trait AudioProvider: Send + Sync {
    fn supports(&self, kind: FfiAudioKind) -> bool;
    fn devices(&self) -> Result<Vec<FfiAudioDevice>, FfiError>;
    fn start_capture(
        &self,
        requested: FfiAudioConfig,
        startup: Arc<BackendStartup>,
    ) -> Result<Arc<AudioSource>, FfiError>;
}
pub struct AudioSource {
    config: vnilink::audio::AudioConfig,
    inner: Arc<vnilink::audio::AudioSource>,
}

impl AudioSource {
    pub fn new(config: FfiAudioConfig) -> Result<Arc<Self>, FfiError> {
        let config: vnilink::audio::AudioConfig = config.into();
        config.validate().map_err(map_error)?;
        Ok(Arc::new(Self {
            config,
            inner: vnilink::audio::AudioSource::new(),
        }))
    }
    pub fn publish(&self, packet: FfiAudioPacket) -> Result<(), FfiError> {
        if packet.channels != self.config.channels {
            return Err(map_error(vnilink::Error::Malformed));
        }
        self.inner
            .publish(vnilink_audio::AudioPacket {
                sequence: packet.sequence,
                timestamp: packet.timestamp,
                format: self.config.format().map_err(map_error)?,
                payload: packet.payload,
            })
            .map_err(map_error)
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn fail(&self) {
        self.inner.fail();
    }
}
struct ProviderAdapter(Arc<dyn AudioProvider>);
impl vnilink::audio::HostAudioFactory for ProviderAdapter {
    fn supports(&self, kind: vnilink::audio::AudioKind) -> bool {
        self.0.supports(kind.into())
    }
    fn devices(&self) -> Result<Vec<vnilink::audio::AudioDevice>, vnilink::Error> {
        self.0
            .devices()
            .map(|list| {
                list.into_iter()
                    .map(|d| vnilink::audio::AudioDevice {
                        id: d.id,
                        name: d.name,
                        kind: d.kind.into(),
                    })
                    .collect()
            })
            .map_err(|_| vnilink::Error::Unsupported("native audio enumeration"))
    }
    fn open(
        &self,
        requested: vnilink::audio::AudioConfig,
        startup: vnilink::BackendStartup,
    ) -> Result<vnilink::audio::AudioBackend, vnilink::Error> {
        startup.check()?;
        let source = self
            .0
            .start_capture(
                requested.clone().into(),
                Arc::new(BackendStartup {
                    inner: startup.clone(),
                }),
            )
            .map_err(|_| vnilink::Error::Unsupported("native audio capture"))?;
        if let Err(e) = startup.check() {
            source.inner.close();
            return Err(e);
        }
        if source.config != requested {
            source.inner.close();
            return Err(vnilink::Error::Malformed);
        }
        let backend =
            vnilink::audio::AudioBackend::new(source.config.clone(), source.inner.clone());
        if matches!(&backend,Err(error) if !matches!(error,vnilink::Error::Busy)) {
            source.inner.close();
        }
        backend
    }
}
pub struct AudioService {
    inner: vnilink::RemoteAudioService,
    pub(crate) core: Arc<vnilink::Core>,
    pub(crate) playback_claimed: std::sync::atomic::AtomicBool,
    playback: Mutex<vnilink_audio::AudioPlayback>,
}

impl AudioService {
    pub fn info(&self) -> FfiServiceInfo {
        self.inner.info().clone().into()
    }
    pub fn config(&self) -> FfiAudioConfig {
        self.inner.config().into()
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
    pub fn close(&self) {
        self.inner.close();
    }
    pub fn set_muted(&self, muted: bool) {
        self.playback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .set_muted(muted);
    }
    /// Called by an existing native playback worker, not a UI/render thread.
    /// The oldest 20 ms block that has arrived, decoded; the output paces
    /// playback. Mute still consumes transport and codec state.
    pub fn poll_playback(&self) -> Result<Option<FfiAudioBlock>, FfiError> {
        if self
            .playback_claimed
            .load(std::sync::atomic::Ordering::Acquire)
        {
            return Err(map_error(vnilink::Error::Busy));
        }
        self.poll_internal()
    }
    pub fn playback_stats(&self) -> FfiAudioPlaybackStats {
        let s = self
            .playback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats();
        FfiAudioPlaybackStats {
            received: s.received,
            late: s.late,
            duplicate: s.duplicate,
            overflow: s.overflow,
            discontinuities: s.discontinuities,
            queued: s.queued as u32,
        }
    }
}
impl AudioService {
    pub(crate) fn poll_internal(&self) -> Result<Option<FfiAudioBlock>, FfiError> {
        if self.inner.is_cancelled() {
            return Err(map_error(vnilink::Error::NoSession));
        }
        let mut playback = self.playback.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..8 {
            let Some(packet) = self.inner.try_recv() else {
                break;
            };
            playback
                .push(packet)
                .map_err(|_| map_error(vnilink::Error::Malformed))?;
        }
        playback
            .poll()
            .map(|block| {
                block.map(|b| FfiAudioBlock {
                    timestamp: b.timestamp,
                    samples: b.samples,
                })
            })
            .map_err(|_| map_error(vnilink::Error::Malformed))
    }
}

impl VnilinkCore {
    pub fn set_audio_provider(
        &self,
        provider: Option<Box<dyn AudioProvider>>,
    ) -> Result<(), FfiError> {
        self.inner
            .set_audio_factory(provider.map(|p| {
                Arc::new(ProviderAdapter(Arc::from(p))) as Arc<dyn vnilink::audio::HostAudioFactory>
            }))
            .map_err(map_error)
    }
    pub fn remote_audio_devices(
        &self,
        connection: u64,
        generation: u64,
        kind: FfiAudioKind,
    ) -> Result<Vec<FfiAudioDevice>, FfiError> {
        self.inner
            .remote_audio_devices(connection, generation, kind.into())
            .map(|list| {
                list.into_iter()
                    .map(|d| FfiAudioDevice {
                        id: d.id,
                        name: d.name,
                        kind: d.kind.into(),
                    })
                    .collect()
            })
            .map_err(map_error)
    }
    pub fn open_audio_service(
        &self,
        id: u64,
        generation: u64,
        config: FfiAudioConfig,
    ) -> Result<Arc<AudioService>, FfiError> {
        let inner = self
            .inner
            .open_audio_service(id, generation, config.into())
            .map_err(map_error)?;
        let playback =
            vnilink_audio::AudioPlayback::new(inner.config().format().map_err(map_error)?)
                .map_err(|_| map_error(vnilink::Error::Unsupported("Opus audio decoder")))?;
        Ok(Arc::new(AudioService {
            inner,
            core: self.inner.clone(),
            playback_claimed: std::sync::atomic::AtomicBool::new(false),
            playback: Mutex::new(playback),
        }))
    }
}
