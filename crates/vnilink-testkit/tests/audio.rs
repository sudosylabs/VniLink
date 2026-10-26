//! The audio services alone: the host's system sound and microphone heard
//! on the controller, each under its own grant.

use expect_test::expect;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use vnilink::audio::{
    AudioBackend, AudioConfig, AudioDevice, AudioKind, AudioSource, HostAudioFactory,
};
use vnilink::{BackendStartup, Error, ServiceKind, SessionPhase};
use vnilink_testkit::audio::{config, encoder, packet, AudioHost};
use vnilink_testkit::{traced, wait_for, Pair, TestResult, DEFAULT_WAIT};

fn next(audio: &vnilink::RemoteAudioService) -> vnilink::audio::AudioPacket {
    wait_for("a packet", DEFAULT_WAIT, || audio.try_recv())
}

/// System sound and the microphone each arrive exactly as sent, and
/// closing one leaves the other playing.
#[test]
fn each_source_arrives_as_sent_and_closes_alone() {
    let trace = traced();
    let host = AudioHost::start(&[ServiceKind::SystemAudio, ServiceKind::Microphone]);
    let system = host.open(AudioKind::System).unwrap();
    let microphone = host.open(AudioKind::Microphone).unwrap();
    let mut encoding = encoder();
    for kind in [AudioKind::System, AudioKind::Microphone] {
        let sent = packet(&mut encoding);
        host.audio.latest(kind).publish(sent.clone()).unwrap();
        let audio = if kind == AudioKind::System {
            &system
        } else {
            &microphone
        };
        assert_eq!(next(audio), sent);
    }
    microphone.close();
    wait_for("the microphone to stop", DEFAULT_WAIT, || {
        host.audio
            .latest(AudioKind::Microphone)
            .is_cancelled()
            .then_some(())
    });
    let sent = packet(&mut encoding);
    host.audio
        .latest(AudioKind::System)
        .publish(sent.clone())
        .unwrap();
    assert_eq!(next(&system), sent);
    expect![[r#"
        controller:
          audio: audio ended closed_here=true dropped=0 kind=Microphone service=2
        host:
          audio: audio opened channels=2 kind=System service=1 voice=false
          audio: audio opened channels=2 kind=Microphone service=2 voice=false
          audio: audio stream ended ended=viewer closed kind=Microphone service=2
    "#]]
    .assert_eq(&trace.story(&["audio"]));
}

/// The host offers both sounds before it shares either, lists the devices
/// of a sound only once it shares it, and the microphone needs its own
/// grant: system sound alone neither lists nor opens it.
#[test]
fn the_microphone_needs_its_own_grant() -> TestResult {
    let _trace = traced();
    let host = AudioHost::start(&[]);
    let (hosting, controller) = (&host.pair.host.core, &host.pair.controller.core);
    let connection = &host.connection;
    let support = controller.remote_service_support(connection.id, connection.generation)?;
    assert!(
        support.system_audio && support.microphone,
        "a host with an audio backend offers both sounds: {support:?}"
    );
    assert!(
        matches!(
            controller.remote_audio_devices(
                connection.id,
                connection.generation,
                AudioKind::System
            ),
            Err(Error::Unauthorized)
        ),
        "system sound the host does not share lists no devices"
    );
    hosting.set_service_sharing(ServiceKind::SystemAudio, true)?;
    assert!(
        !hosting
            .service_permissions(&host.pair.controller.endpoint())?
            .microphone,
        "sharing system sound does not grant the microphone"
    );
    assert!(
        matches!(
            controller.remote_audio_devices(
                connection.id,
                connection.generation,
                AudioKind::Microphone
            ),
            Err(Error::Unauthorized)
        ),
        "a microphone the host does not share lists no devices"
    );
    assert_eq!(
        controller.remote_audio_devices(connection.id, connection.generation, AudioKind::System)?,
        vec![host.audio.devices()?[0].clone()],
        "shared system sound lists the host's system sound alone"
    );
    assert!(
        host.open(AudioKind::System).is_ok(),
        "shared system sound opens"
    );
    assert!(
        matches!(host.open(AudioKind::Microphone), Err(Error::Unauthorized)),
        "a microphone the host does not share does not open"
    );
    Ok(())
}

/// An audio service opens the sound of its own kind, once.
#[test]
fn an_audio_service_opens_its_own_kind_once() -> TestResult {
    let trace = traced();
    let host = AudioHost::start(&[ServiceKind::SystemAudio, ServiceKind::Microphone]);
    let controller = &host.pair.controller.core;
    let owner = host.pair.attach(&host.connection, ServiceKind::SystemAudio);
    assert!(
        matches!(
            controller.open_audio_service(
                owner.id,
                owner.generation,
                config(AudioKind::Microphone)
            ),
            Err(Error::InvalidSetting(_))
        ),
        "a system sound service does not open the microphone"
    );
    let _system =
        controller.open_audio_service(owner.id, owner.generation, config(AudioKind::System))?;
    assert!(
        matches!(
            controller.open_audio_service(owner.id, owner.generation, config(AudioKind::System)),
            Err(Error::Busy)
        ),
        "a service opens its sound once"
    );
    trace.wait_for_records("audio opened", 1);
    expect![[r#"
        host:
          audio: audio opened channels=2 kind=System service=1 voice=false
    "#]]
    .assert_eq(&trace.story(&["audio"]));
    Ok(())
}

/// A host that stops sharing its microphone stops the microphone alone:
/// system sound keeps arriving as sent, and Files and the connection stay;
/// closing system sound then stops it and still leaves them.
#[test]
fn withdrawing_the_microphone_stops_it_alone() -> TestResult {
    let trace = traced();
    let host = AudioHost::start(&[ServiceKind::SystemAudio, ServiceKind::Microphone]);
    let files = host.pair.attach(&host.connection, ServiceKind::Files);
    let files = host
        .pair
        .controller
        .core
        .service_lease(files.id, files.generation)?;
    let system = host.open(AudioKind::System)?;
    let microphone = host.open(AudioKind::Microphone)?;
    let (system_source, microphone_source) = (
        host.audio.latest(AudioKind::System),
        host.audio.latest(AudioKind::Microphone),
    );
    let mut encoding = encoder();
    let sent = packet(&mut encoding);
    system_source.publish(sent.clone())?;
    assert_eq!(next(&system), sent, "system sound arrives as sent");

    host.pair
        .host
        .core
        .set_service_sharing(ServiceKind::Microphone, false)?;
    wait_for("the microphone to stop on both sides", DEFAULT_WAIT, || {
        (microphone_source.is_cancelled() && microphone.is_cancelled()).then_some(())
    });
    assert!(
        !system_source.is_cancelled(),
        "withdrawing the microphone leaves system sound"
    );
    assert!(
        !files.is_cancelled(),
        "withdrawing the microphone leaves Files"
    );
    let sent = packet(&mut encoding);
    system_source.publish(sent.clone())?;
    assert_eq!(
        next(&system),
        sent,
        "system sound still arrives as sent once the microphone is withdrawn"
    );

    system.close();
    wait_for("the host to stop system sound", DEFAULT_WAIT, || {
        system_source.is_cancelled().then_some(())
    });
    assert!(!files.is_cancelled(), "closing system sound leaves Files");
    assert!(
        host.pair
            .controller
            .core
            .controller_session(host.connection.id)
            .is_ok_and(|session| session.phase == SessionPhase::Active),
        "closing system sound keeps the connection"
    );
    trace.wait_for_records("audio ended", 2);
    expect![[r#"
        controller:
          audio: audio ended closed_here=false dropped=0 ended=withdrawn kind=Microphone service=3
          audio: audio ended closed_here=true dropped=0 kind=System service=2
        host:
          audio: audio opened channels=2 kind=System service=1 voice=false
          audio: audio opened channels=2 kind=Microphone service=2 voice=false
          audio: audio stream ended ended=service closed kind=Microphone service=2
          audio: audio stream ended ended=viewer closed kind=System service=1
    "#]]
    .assert_eq(&trace.story(&["audio"]));
    Ok(())
}

/// A microphone that opens on the host only once its grant is withdrawn,
/// as an operating system call that finishes late.
struct OpensLate {
    entered: SyncSender<BackendStartup>,
    release: Mutex<Receiver<()>>,
    source: Arc<AudioSource>,
}

impl HostAudioFactory for OpensLate {
    fn supports(&self, kind: AudioKind) -> bool {
        kind == AudioKind::Microphone
    }

    fn devices(&self) -> Result<Vec<AudioDevice>, Error> {
        Ok(vec![AudioDevice {
            id: config(AudioKind::Microphone).device_id,
            name: "Late microphone".into(),
            kind: AudioKind::Microphone,
        }])
    }

    fn open(&self, config: AudioConfig, startup: BackendStartup) -> Result<AudioBackend, Error> {
        self.entered
            .send(startup)
            .expect("the test waits for the open");
        self.release
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .recv_timeout(Duration::from_secs(8))
            .expect("the test releases the open");
        AudioBackend::new(config, Arc::clone(&self.source))
    }
}

/// A microphone whose grant is withdrawn while it starts learns it from its
/// startup, and once it opens anyway it is released unused: the viewer's
/// open fails, and Files and the connection stay.
#[test]
fn a_microphone_withdrawn_while_it_starts_is_released_unused() -> TestResult {
    let _trace = traced();
    let pair = Pair::for_services(&[ServiceKind::Microphone]);
    let (entered, entering) = sync_channel(1);
    let (release, released) = sync_channel(1);
    let source = AudioSource::new();
    source.publish(packet(&mut encoder()))?;
    pair.host.core.set_audio_factory(Some(Arc::new(OpensLate {
        entered,
        release: Mutex::new(released),
        source: Arc::clone(&source),
    })))?;
    let connection = pair.connect_services();
    let files = pair.attach(&connection, ServiceKind::Files);
    let files = pair
        .controller
        .core
        .service_lease(files.id, files.generation)?;
    let owner = pair.attach(&connection, ServiceKind::Microphone);
    std::thread::scope(|scope| -> TestResult {
        let opening = scope.spawn(|| {
            pair.controller.core.open_audio_service(
                owner.id,
                owner.generation,
                config(AudioKind::Microphone),
            )
        });
        let startup = entering.recv_timeout(Duration::from_secs(8))?;
        pair.host
            .core
            .set_service_sharing(ServiceKind::Microphone, false)?;
        wait_for(
            "the startup to learn its grant is withdrawn",
            DEFAULT_WAIT,
            || matches!(startup.check(), Err(Error::NoSession)).then_some(()),
        );
        release.send(())?;
        assert!(
            opening.join().expect("the open returns").is_err(),
            "a microphone that opened after its grant was withdrawn is not admitted"
        );
        Ok(())
    })?;
    wait_for("the late microphone to be released", DEFAULT_WAIT, || {
        source.is_cancelled().then_some(())
    });
    assert!(!files.is_cancelled(), "a released microphone leaves Files");
    assert!(
        pair.controller
            .core
            .controller_session(connection.id)
            .is_ok_and(|session| session.phase == SessionPhase::Active),
        "a released microphone keeps the connection"
    );
    Ok(())
}

/// A viewer that does not keep up loses the oldest packets, and the loss
/// is counted where it happened rather than hidden.
#[test]
fn packets_a_slow_viewer_misses_are_counted() {
    let trace = traced();
    let host = AudioHost::start(&[ServiceKind::SystemAudio]);
    let system = host.open(AudioKind::System).unwrap();
    let mut encoding = encoder();
    let source = host.audio.latest(AudioKind::System);
    // Far more than any queue on the way holds, sent at once and never read.
    for _ in 0..200 {
        source.publish(packet(&mut encoding)).unwrap();
    }
    // Every packet that was kept has gone to the viewer, who reads none.
    wait_for("the host to send what it kept", DEFAULT_WAIT, || {
        (source.queued() == 0).then_some(())
    });
    system.close();
    let ended = wait_for("the audio to end", DEFAULT_WAIT, || {
        let ended = trace.named("audio ended");
        (!ended.is_empty()).then_some(ended)
    });
    let dropped = ended[0]["dropped"].as_u64().unwrap_or(0) + source.dropped();
    assert!(
        dropped > 0,
        "200 unread packets were dropped without a count: {ended:?}"
    );
}
