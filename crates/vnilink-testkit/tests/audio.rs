//! The audio services alone: the host's system sound and microphone heard
//! on the controller, each under its own grant.

use expect_test::expect;
use std::time::Duration;

use vnilink::audio::AudioKind;
use vnilink::{Error, ServiceKind};
use vnilink_testkit::audio::{encoder, packet, AudioHost};
use vnilink_testkit::{traced, wait_for, DEFAULT_WAIT};

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
          audio: audio ended closed_here=true dropped=0 ended=no session kind=Microphone service=2
        host:
          audio: audio opened channels=2 kind=System service=1 voice=false
          audio: audio opened channels=2 kind=Microphone service=2 voice=false
    "#]]
    .assert_eq(&trace.story(&["audio"]));
}

/// The microphone needs its own grant: system sound alone does not open it.
#[test]
fn the_microphone_needs_its_own_grant() {
    let _trace = traced();
    let host = AudioHost::start(&[ServiceKind::SystemAudio]);
    assert!(host.open(AudioKind::System).is_ok());
    assert!(matches!(
        host.open(AudioKind::Microphone),
        Err(Error::Unauthorized)
    ));
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
    std::thread::sleep(Duration::from_millis(300));
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
