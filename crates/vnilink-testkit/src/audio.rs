// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! What the audio suites share: the host's sound sources standing in for
//! its system sound and microphone, sending the Opus packets a test
//! encodes, audio opened from the controller as the application opens it,
//! and [`check`], which stops those sounds one at a time.

use std::sync::{Arc, Mutex};

use expect_test::Expect;
use vnilink::audio::{
    AudioBackend, AudioConfig, AudioDevice, AudioKind, AudioSource, HostAudioFactory,
};
use vnilink::{BackendStartup, Error, RemoteAudioService, ServiceKind, SessionInfo, SessionPhase};
use vnilink_audio::{AudioEncoder, AudioPacket};

use crate::{assert_story, wait_for, Pair, TestResult, Traced, DEFAULT_WAIT};

/// Samples in one packet: 20 ms of stereo at 48 kHz.
pub const PACKET_SAMPLES: usize = 1920;

/// The name the host gives each of its stand-in sounds.
pub const SOUND_NAME: &str = "Test kit sound";

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
                name: SOUND_NAME.into(),
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

/// A host offering `factory` as its sound sources and sharing `kinds` of
/// sound, and the controller's services connection to it.
pub fn connect(
    kinds: &[ServiceKind],
    factory: Arc<dyn HostAudioFactory>,
) -> Result<(Pair, SessionInfo), Error> {
    let pair = Pair::for_services(kinds);
    pair.host.core.set_audio_factory(Some(factory))?;
    let connection = pair.controller.connect_services(&pair.host)?;
    Ok((pair, connection))
}

/// A host sharing `kinds` of sound, and a controller connected to it.
pub struct AudioHost {
    pub pair: Pair,
    pub audio: Arc<StandInAudio>,
    pub connection: SessionInfo,
}

impl AudioHost {
    pub fn start(kinds: &[ServiceKind]) -> Result<Self, Error> {
        let audio = Arc::new(StandInAudio::default());
        let (pair, connection) = connect(kinds, Arc::clone(&audio) as _)?;
        Ok(Self {
            pair,
            audio,
            connection,
        })
    }

    /// Sends one packet from the host's sound of `kind` and asserts `audio`
    /// receives it exactly as sent.
    #[track_caller]
    pub fn assert_arrives(
        &self,
        kind: AudioKind,
        audio: &RemoteAudioService,
        encoder: &mut AudioEncoder,
    ) -> Result<(), Error> {
        let sent = packet(encoder);
        self.audio.latest(kind).publish(sent.clone())?;
        let received = wait_for("a packet", DEFAULT_WAIT, || audio.try_recv());
        assert_eq!(received, sent, "{kind:?} sound arrives exactly as sent");
        Ok(())
    }

    /// Whether the controller's connection is still up.
    pub fn connection_active(&self) -> bool {
        self.pair
            .controller
            .core
            .controller_session(self.connection.id)
            .is_ok_and(|session| session.phase == SessionPhase::Active)
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

/// How [`check`] stops one of the host's sounds.
#[derive(Clone, Copy, Debug)]
pub enum Stop {
    /// The viewer closes it.
    ViewerCloses,
    /// The host stops sharing it.
    HostWithdraws,
}

/// Opens the system sound and the microphone of a host sharing both, beside
/// Files, then stops each sound of `stopping` in turn as its [`Stop`] says.
/// Asserts that each sound arrives exactly as sent before any stops; that a
/// stopped sound stops on both sides while each sound still open keeps
/// arriving as sent; that Files and the connection stay throughout; that no
/// record holds the host's sound devices' names or ids; and that the
/// engines told the story `expect` of their audio.
#[track_caller]
pub fn check(trace: &Traced, stopping: &[(AudioKind, Stop)], expect: Expect) -> TestResult {
    let host = AudioHost::start(&[ServiceKind::SystemAudio, ServiceKind::Microphone])?;
    let files = host.pair.attach(&host.connection, ServiceKind::Files);
    let files = host
        .pair
        .controller
        .core
        .service_lease(files.id, files.generation)?;
    let mut open = Vec::new();
    for kind in [AudioKind::System, AudioKind::Microphone] {
        let audio = host.open(kind)?;
        open.push((kind, audio, host.audio.latest(kind)));
    }
    let mut encoding = encoder();
    for (kind, audio, _) in &open {
        host.assert_arrives(*kind, audio, &mut encoding)?;
    }
    for &(kind, stop) in stopping {
        let at = open
            .iter()
            .position(|(opened, _, _)| *opened == kind)
            .ok_or("each sound stops once")?;
        let (_, audio, source) = open.remove(at);
        match stop {
            Stop::ViewerCloses => audio.close(),
            Stop::HostWithdraws => host
                .pair
                .host
                .core
                .set_service_sharing(kind.service(), false)?,
        }
        wait_for(
            &format!("{kind:?} sound to stop on both sides"),
            DEFAULT_WAIT,
            || (source.is_cancelled() && audio.is_cancelled()).then_some(()),
        );
        for (other, audio, source) in &open {
            assert!(
                !source.is_cancelled(),
                "stopping {kind:?} sound leaves {other:?} sound"
            );
            host.assert_arrives(*other, audio, &mut encoding)?;
        }
        assert!(
            !files.is_cancelled(),
            "stopping {kind:?} sound leaves Files"
        );
        assert!(
            host.connection_active(),
            "stopping {kind:?} sound keeps the connection"
        );
    }
    trace.wait_for_records("audio ended", stopping.len());
    assert_story(expect, || trace.story(&["audio"]));
    let system = config(AudioKind::System).device_id;
    let microphone = config(AudioKind::Microphone).device_id;
    trace.assert_unrecorded(
        "the host's sound devices",
        &[&system, &microphone, SOUND_NAME],
    );
    Ok(())
}
