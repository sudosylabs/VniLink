//! Host-approved independent system/microphone audio sources. Core owns no
//! native devices. The native producer emits bounded validated Opus packets.
use crate::{BackendStartup, Error, ServiceKind};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
pub use vnilink_audio::{AudioPacket, Format as AudioFormat};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AudioKind {
    System,
    Microphone,
}
impl AudioKind {
    pub fn service(self) -> ServiceKind {
        match self {
            Self::System => ServiceKind::SystemAudio,
            Self::Microphone => ServiceKind::Microphone,
        }
    }
    pub(crate) fn wire(self) -> u8 {
        match self {
            Self::System => 5,
            Self::Microphone => 6,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub kind: AudioKind,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AudioConfig {
    pub device_id: String,
    pub kind: AudioKind,
    pub channels: u8,
    pub voice: bool,
}
impl AudioConfig {
    pub fn format(&self) -> Result<AudioFormat, Error> {
        match self.channels {
            1 => Ok(AudioFormat::Mono),
            2 => Ok(AudioFormat::Stereo),
            _ => Err(Error::InvalidSetting(
                "audio channels must be mono or stereo",
            )),
        }
    }
    pub fn validate(&self) -> Result<(), Error> {
        self.format()?;
        if self.device_id.is_empty()
            || self.device_id.len() > 512
            || self.voice && self.kind != AudioKind::Microphone
        {
            return Err(Error::InvalidSetting(
                "audio source configuration is invalid",
            ));
        }
        Ok(())
    }
}
pub trait HostAudioFactory: Send + Sync {
    fn supports(&self, kind: AudioKind) -> bool;
    /// Only locally approved sources. Never open devices while enumerating.
    fn devices(&self) -> Result<Vec<AudioDevice>, Error>;
    /// Return after native capture starts successfully (and, for microphones,
    /// the first packet). System loopback may be silent without producing buffers.
    /// Check startup cancellation and OS authorization before activating a source.
    fn open(&self, config: AudioConfig, startup: BackendStartup) -> Result<AudioBackend, Error>;
}
pub struct AudioSource {
    claimed: AtomicBool,
    stopped: AtomicBool,
    failed: AtomicBool,
    queue: Mutex<VecDeque<AudioPacket>>,
    notify: tokio::sync::Notify,
}
impl AudioSource {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            claimed: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::with_capacity(8)),
            notify: tokio::sync::Notify::new(),
        })
    }
    pub fn publish(&self, packet: AudioPacket) -> Result<(), Error> {
        packet.validate().map_err(|_| Error::Malformed)?;
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if self.is_cancelled() {
            return Err(Error::NoSession);
        }
        if queue.len() == 8 {
            queue.pop_front();
        } // loss is observable through sequence gaps.
        queue.push_back(packet);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }
    pub fn is_cancelled(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn fail(&self) {
        self.failed.store(true, Ordering::Release);
        self.close();
    }
    pub fn close(&self) {
        self.stopped.store(true, Ordering::Release);
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.notify.notify_waiters();
    }
    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub(crate) fn pop(&self) -> Option<AudioPacket> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop_front()
    }
    pub(crate) async fn notified(&self) {
        self.notify.notified().await
    }
}
pub struct AudioBackend {
    pub config: AudioConfig,
    pub source: Arc<AudioSource>,
}
impl AudioBackend {
    pub fn new(config: AudioConfig, source: Arc<AudioSource>) -> Result<Self, Error> {
        config.validate()?;
        if source.is_cancelled() {
            return Err(Error::NoSession);
        }
        // Backend admission requires actual encoded output, not just a device
        // handle. The first packet also proves the declared channel format.
        if !source
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .front()
            .is_some_and(|p| Some(p.format) == config.format().ok())
        {
            return Err(Error::InvalidSetting("audio source has no initial packet"));
        }
        if source
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::Busy);
        }
        Ok(Self { config, source })
    }
}
impl Drop for AudioBackend {
    fn drop(&mut self) {
        self.source.close();
    }
}
