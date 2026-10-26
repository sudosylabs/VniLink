use crate::{audio::*, Error};
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
