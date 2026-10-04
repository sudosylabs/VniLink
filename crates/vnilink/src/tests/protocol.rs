use crate::error::Error;
use crate::input::InputEvent;
use crate::input::MAX_KEY_TEXT;
use crate::protocol::{
    error_code, ControlMessage, KeyText, Quality, RejectCode, SessionStamp, MAX_CONTROL_FRAME,
    PROTOCOL_VERSION,
};

#[test]
fn control_messages_round_trip() {
    let messages = [
        ControlMessage::Hello {
            version: PROTOCOL_VERSION,
        },
        ControlMessage::Accept {
            stamp: SessionStamp::new(7, 3),
            monitor_width: 1920,
            monitor_height: 1080,
        },
        ControlMessage::Reject(RejectCode::Unauthorized),
        ControlMessage::Input {
            stamp: SessionStamp::new(7, 3),
            event: InputEvent::key_down(crate::input::usage::A),
        },
        ControlMessage::Input {
            stamp: SessionStamp::new(7, 3),
            event: InputEvent::pointer(15, 20),
        },
        ControlMessage::Input {
            stamp: SessionStamp::new(7, 3),
            event: InputEvent::wheel(0, -1),
        },
        ControlMessage::Keyframe {
            stamp: SessionStamp::new(7, 3),
        },
        ControlMessage::Text {
            stamp: SessionStamp::new(7, 3),
            text: KeyText("héllo".to_owned()),
        },
        ControlMessage::Chord {
            stamp: SessionStamp::new(7, 3),
            text: KeyText("@".to_owned()),
        },
    ];
    for message in messages {
        assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    }
}

#[test]
fn oversized_and_unknown_frames_are_malformed() {
    assert!(matches!(ControlMessage::decode(&[]), Err(Error::Malformed)));
    assert!(matches!(
        ControlMessage::decode(&[9, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[10, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[0u8; MAX_CONTROL_FRAME + 1]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[0u8; MAX_CONTROL_FRAME]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[1, 0, 1, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[2, 0, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[3, 0, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[3, 0, 8]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[5, 0]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[8, 0]),
        Err(Error::Malformed)
    ));
}

#[test]
fn finished_text_hides_the_characters_and_rejects_bad_bytes() {
    let message = ControlMessage::Text {
        stamp: SessionStamp::new(4, 9),
        text: KeyText("vnl-secret-héllo".to_owned()),
    };
    assert!(!format!("{message:?}").contains("vnl-secret"));
    assert!(!format!("{message:?}").contains("héllo"));
    assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    let chord = ControlMessage::Chord {
        stamp: SessionStamp::new(4, 9),
        text: KeyText("@".to_owned()),
    };
    assert!(!format!("{chord:?}").contains('@'));
    assert_eq!(ControlMessage::decode(&chord.encode()).unwrap(), chord);

    let mut empty = vec![8];
    empty.extend_from_slice(&1u64.to_be_bytes());
    empty.extend_from_slice(&2u64.to_be_bytes());
    assert_eq!(
        ControlMessage::decode(&empty).unwrap(),
        ControlMessage::Text {
            stamp: SessionStamp::new(1, 2),
            text: KeyText(String::new()),
        }
    );

    let mut bad = empty.clone();
    bad.push(0xFF);
    assert!(matches!(
        ControlMessage::decode(&bad),
        Err(Error::Malformed)
    ));

    let mut long = empty;
    long.extend(std::iter::repeat_n(b'a', MAX_KEY_TEXT + 1));
    assert!(long.len() <= MAX_CONTROL_FRAME);
    match ControlMessage::decode(&long).unwrap() {
        ControlMessage::Text { text, .. } => {
            assert_eq!(text.0.len(), MAX_KEY_TEXT + 1);
            assert!(!format!("{text:?}").contains('a'));
        }
        other => panic!("decoded {other:?}"),
    }
}

#[test]
fn every_reject_code_and_input_shape_round_trips() {
    for code in [
        RejectCode::Unauthorized,
        RejectCode::NotHosting,
        RejectCode::Busy,
        RejectCode::IncompatibleVersion,
        RejectCode::StaleSession,
        RejectCode::Unsupported,
        RejectCode::Malformed,
    ] {
        let message = ControlMessage::Reject(code);
        assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
        let error = code.to_error();
        assert_eq!(error_code(&error), code);
    }
    assert!(matches!(
        RejectCode::Unsupported.to_error(),
        Error::Unsupported("no desktop adapter is installed")
    ));

    let events = [
        InputEvent::key_down(crate::input::usage::A),
        InputEvent::key_up(crate::input::usage::ENTER),
        InputEvent::button_down(crate::input::PointerButton::Left),
        InputEvent::button_up(crate::input::PointerButton::Left),
        InputEvent::button_down(crate::input::PointerButton::Right),
        InputEvent::button_up(crate::input::PointerButton::Right),
        InputEvent::button_down(crate::input::PointerButton::Middle),
        InputEvent::button_up(crate::input::PointerButton::Middle),
        InputEvent::pointer(0, 0),
        InputEvent::pointer(u16::MAX, u16::MAX),
        InputEvent::wheel(i16::MIN, i16::MAX),
    ];
    for event in events {
        let message = ControlMessage::Input {
            stamp: SessionStamp::new(u64::MAX, 0),
            event,
        };
        assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    }

    let mut bad_direction = vec![4];
    bad_direction.extend_from_slice(&1u64.to_be_bytes());
    bad_direction.extend_from_slice(&1u64.to_be_bytes());
    bad_direction.extend_from_slice(&[1, 0, 0x04, 2]);
    assert!(matches!(
        ControlMessage::decode(&bad_direction),
        Err(Error::Malformed)
    ));
}

#[test]
fn quality_limits_are_enforced() {
    assert!(Quality::new(1, 1, 1, 1).is_ok());
    assert!(Quality::new(50_000, 60, 7680, 7680).is_ok());
    assert!(matches!(
        Quality::new(0, 1, 1, 1),
        Err(Error::InvalidSetting(
            "bitrate must be between 1 and 50000 kbps"
        ))
    ));
    assert!(matches!(
        Quality::new(50_001, 1, 1, 1),
        Err(Error::InvalidSetting(
            "bitrate must be between 1 and 50000 kbps"
        ))
    ));
    assert!(matches!(
        Quality::new(1, 0, 1, 1),
        Err(Error::InvalidSetting("frame rate must be between 1 and 60"))
    ));
    assert!(matches!(
        Quality::new(1, 61, 1, 1),
        Err(Error::InvalidSetting("frame rate must be between 1 and 60"))
    ));
    assert!(matches!(
        Quality::new(1, 1, 0, 1),
        Err(Error::InvalidSetting(
            "width and height must be between 1 and 7680"
        ))
    ));
    assert!(matches!(
        Quality::new(1, 1, 1, 7681),
        Err(Error::InvalidSetting(
            "width and height must be between 1 and 7680"
        ))
    ));
}

#[test]
fn unmapped_errors_become_a_malformed_reject() {
    assert_eq!(error_code(&Error::ShutDown), RejectCode::Malformed);
    assert_eq!(error_code(&Error::Timeout), RejectCode::Malformed);
    assert_eq!(
        error_code(&Error::Network("down".to_owned())),
        RejectCode::Malformed
    );
    assert_eq!(
        error_code(&Error::InputFailed(
            "the operating system rejected the event"
        )),
        RejectCode::Malformed
    );
    assert_eq!(error_code(&Error::NoSession), RejectCode::Malformed);
    assert_eq!(error_code(&Error::ClipboardDenied), RejectCode::Malformed);
    assert_eq!(error_code(&Error::TransferDenied), RejectCode::Malformed);
    assert_eq!(error_code(&Error::FileExists), RejectCode::Malformed);
}
