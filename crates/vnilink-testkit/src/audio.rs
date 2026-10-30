// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the audio suites share: the host's sound sources standing in for
//! its system sound and microphone, sending the Opus packets a test
//! encodes, and audio opened from the controller as the application opens
//! it.

use std::sync::{Arc, Mutex};

use vnilink::audio::{
    AudioBackend, AudioConfig, AudioDevice, AudioKind, AudioSource, HostAudioFactory,
};
use vnilink::{BackendStartup, Error, RemoteAudioService, ServiceKind};
use vnilink_audio::{AudioEncoder, AudioPacket};

use crate::{wait_for, Pair, DEFAULT_WAIT};

/// Samples in one packet: 20 ms of stereo at 48 kHz.
pub const PACKET_SAMPLES: usize = 1920;

pub fn config(kind: AudioKind) -> AudioConfig {
    AudioConfig {
        device_id: match kind {
            AudioKind::System => "testkit-system",
            AudioKind::Microphone => "testkit-microphone",
        }
        .into(),
        kind,
        channels: 2,
        voice: false,
    }
}

/// One packet of a quiet tone.
pub fn packet(encoder: &mut AudioEncoder) -> AudioPacket {
    encoder
        .encode(&[0.125; PACKET_SAMPLES])
        .expect("an Opus packet")
}

pub fn encoder() -> AudioEncoder {
    AudioEncoder::new(vnilink::audio::AudioFormat::Stereo, false).expect("an Opus encoder")
}

/// The host's sound sources: system sound and a microphone, whose sound
/// is what a test publishes into the [`AudioSource`] each opened.
#[derive(Default)]
pub struct StandInAudio {
    pub sources: Mutex<Vec<(AudioKind, Arc<AudioSource>)>>,
}

impl StandInAudio {
    /// The source of `kind` opened last.
    pub fn latest(&self, kind: AudioKind) -> Arc<AudioSource> {
        wait_for("the host to open its sound source", DEFAULT_WAIT, || {
            self.sources
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .iter()
                .rev()
                .find(|(opened, _)| *opened == kind)
                .map(|(_, source)| Arc::clone(source))
        })
    }
}

impl HostAudioFactory for StandInAudio {
    fn supports(&self, _: AudioKind) -> bool {
        true
    }

    fn devices(&self) -> Result<Vec<AudioDevice>, Error> {
        Ok([AudioKind::System, AudioKind::Microphone]
            .into_iter()
            .map(|kind| AudioDevice {
                id: config(kind).device_id,
                name: "Test kit sound".into(),
                kind,
            })
            .collect())
    }

    fn open(&self, config: AudioConfig, startup: BackendStartup) -> Result<AudioBackend, Error> {
        startup.check()?;
        let source = AudioSource::new();
        // As native capture does, a microphone opens with its first packet
        // ready and system sound opens silent until something plays.
        if config.kind == AudioKind::Microphone {
            let mut first =
                AudioEncoder::new(config.format()?, config.voice).map_err(|_| Error::Malformed)?;
            source.publish(packet(&mut first))?;
        }
        self.sources
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push((config.kind, Arc::clone(&source)));
        AudioBackend::new(config, source)
    }
}

/// A host sharing `kinds` of sound, and a controller connected to it.
pub struct AudioHost {
    pub pair: Pair,
    pub audio: Arc<StandInAudio>,
    pub connection: vnilink::SessionInfo,
}

impl AudioHost {
    pub fn start(kinds: &[ServiceKind]) -> Self {
        let pair = Pair::for_services(kinds);
        let audio = Arc::new(StandInAudio::default());
        pair.host
            .core
            .set_audio_factory(Some(Arc::clone(&audio) as Arc<dyn HostAudioFactory>))
            .expect("an audio backend");
        let connection = pair.connect_services();
        Self {
            pair,
            audio,
            connection,
        }
    }

    /// Opens the host's sound of `kind`, once a microphone's first packet
    /// arrived, so what the viewer takes next is what the test sends.
    /// System sound opens silent and has none to take.
    pub fn open(&self, kind: AudioKind) -> Result<RemoteAudioService, Error> {
        let owner = self.pair.attach(&self.connection, kind.service());
        let audio = self.pair.controller.core.open_audio_service(
            owner.id,
            owner.generation,
            config(kind),
        )?;
        if kind == AudioKind::Microphone {
            wait_for("the microphone's first packet", DEFAULT_WAIT, || {
                audio.try_recv()
            });
        }
        Ok(audio)
    }
}
