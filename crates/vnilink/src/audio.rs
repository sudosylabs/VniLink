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
    /// Packets dropped because nobody took them in time.
    dropped: std::sync::atomic::AtomicU64,
}
impl AudioSource {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            claimed: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            queue: Mutex::new(VecDeque::with_capacity(8)),
            notify: tokio::sync::Notify::new(),
            dropped: std::sync::atomic::AtomicU64::new(0),
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
            // Loss is also observable through sequence gaps.
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        queue.push_back(packet);
        drop(queue);
        self.notify.notify_one();
        Ok(())
    }
    /// Packets published and not yet taken to send.
    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    /// Packets dropped so far because nobody took them in time.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
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
        // A microphone is admitted on encoded output, not just a device handle,
        // and its first packet proves the declared channel format. System
        // loopback stays silent until something plays, so it is admitted on
        // a started capture; the stream checks each packet's format as it sends.
        let first = source
            .queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .front()
            .map(|p| p.format);
        let admitted = match first {
            Some(format) => Some(format) == config.format().ok(),
            None => config.kind == AudioKind::System,
        };
        if !admitted {
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn packet(encoder: &mut vnilink_audio::AudioEncoder) -> AudioPacket {
        encoder.encode(&[0.125; 1920]).unwrap()
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
    fn quiet_system_sound_is_admitted_and_a_quiet_microphone_is_not() {
        assert!(
            AudioBackend::new(config(AudioKind::System), AudioSource::new()).is_ok(),
            "system loopback delivers nothing until something plays"
        );
        assert!(
            matches!(
                AudioBackend::new(config(AudioKind::Microphone), AudioSource::new()),
                Err(Error::InvalidSetting(_))
            ),
            "a microphone is admitted on its first packet"
        );
        let mono = AudioSource::new();
        let mut encoder = vnilink_audio::AudioEncoder::new(AudioFormat::Mono, false).unwrap();
        mono.publish(encoder.encode(&[0.125; 960]).unwrap())
            .unwrap();
        assert!(
            matches!(
                AudioBackend::new(config(AudioKind::System), mono),
                Err(Error::InvalidSetting(_))
            ),
            "a first packet in another format than declared is refused"
        );
    }
}
