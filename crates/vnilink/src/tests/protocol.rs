use crate::error::Error;
use crate::input::InputEvent;
use crate::input::{DisplayInfo, MAX_KEY_TEXT};
use crate::protocol::{
    error_code, ControlMessage, KeyText, Quality, RejectCode, SessionStamp, MAX_CONTROL_FRAME,
    MAX_TOPOLOGY_FRAME, PROTOCOL_VERSION,
};

#[test]
fn control_messages_round_trip() {
    let quality = Quality::new(2_000, 30, 1920, 1080).unwrap();
    let messages = [
        ControlMessage::Hello {
            version: PROTOCOL_VERSION,
            codec: crate::protocol::CODEC_H264,
            quality,
            decode: true,
            files_only: false,
            services_only: false,

            separate_cursor: true,
            hostname: Some("mac-mini".to_string()),
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Hello {
            version: PROTOCOL_VERSION,
            codec: crate::protocol::CODEC_H264,
            quality,
            decode: false,
            files_only: false,
            services_only: false,

            separate_cursor: false,
            hostname: None,
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Hello {
            version: PROTOCOL_VERSION,
            codec: crate::protocol::CODEC_H264_HIGH,
            quality,
            decode: true,
            files_only: false,
            services_only: false,

            separate_cursor: false,
            hostname: Some("mac-mini".to_string()),
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Accept {
            stamp: SessionStamp::new(7, 3),
            monitor_width: 1920,
            monitor_height: 1080,
            codec: crate::protocol::CODEC_H264_HIGH,
            quality,
            hostname: None,
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Accept {
            stamp: SessionStamp::new(7, 3),
            monitor_width: 1920,
            monitor_height: 1080,
            codec: crate::protocol::CODEC_H264,
            quality,
            hostname: Some("linux-laptop".to_string()),
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Accept {
            stamp: SessionStamp::new(7, 3),
            monitor_width: 1920,
            monitor_height: 1080,
            codec: crate::protocol::CODEC_H264,
            quality,
            hostname: None,
            os: crate::OperatingSystem::local(),
        },
        ControlMessage::Reject(RejectCode::Unauthorized),
        ControlMessage::Input {
            stamp: SessionStamp::new(7, 3),
            event: InputEvent::key_down(crate::input::usage::A),
        },
        ControlMessage::Input {
            stamp: SessionStamp::new(7, 3),
            event: InputEvent::pointer(15, 20, 1, 1),
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
        ControlMessage::Quality {
            stamp: SessionStamp::new(7, 3),
            quality: Quality::new(2_000, 30, 1920, 1080).unwrap(),
        },
    ];
    for message in messages {
        assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    }
}

#[test]
fn hello_accept_and_quality_shapes_are_validated() {
    // New Hello/Accept lengths with out-of-range quality are malformed.
    let mut bad_hello = ControlMessage::Hello {
        version: PROTOCOL_VERSION,
        codec: crate::protocol::CODEC_H264,
        quality: Quality::new(2_000, 30, 1920, 1080).unwrap(),
        decode: true,
        files_only: false,
        services_only: false,

        separate_cursor: false,
        hostname: None,
        os: crate::OperatingSystem::local(),
    }
    .encode();
    // Corrupt the frame rate to 0.
    bad_hello[8] = 0;
    assert!(matches!(
        ControlMessage::decode(&bad_hello),
        Err(Error::Malformed)
    ));
    let mut bad_decode = ControlMessage::Hello {
        version: PROTOCOL_VERSION,
        codec: crate::protocol::CODEC_H264,
        quality: Quality::new(2_000, 30, 1920, 1080).unwrap(),
        decode: true,
        files_only: false,
        services_only: false,

        separate_cursor: false,
        hostname: None,
        os: crate::OperatingSystem::local(),
    }
    .encode();
    bad_decode[13] = 2;
    assert!(matches!(
        ControlMessage::decode(&bad_decode),
        Err(Error::Malformed)
    ));
    let mut bad_quality = ControlMessage::Quality {
        stamp: SessionStamp::new(1, 1),
        quality: Quality::new(2_000, 30, 1920, 1080).unwrap(),
    }
    .encode();
    bad_quality[21] = 0;
    assert!(matches!(
        ControlMessage::decode(&bad_quality),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[10, 0]),
        Err(Error::Malformed)
    ));
}

#[test]
fn hostname_shapes_are_validated() {
    let quality = Quality::new(2_000, 30, 1920, 1080).unwrap();
    let hello = |hostname: Option<String>| ControlMessage::Hello {
        version: PROTOCOL_VERSION,
        codec: crate::protocol::CODEC_H264,
        quality,
        decode: true,
        files_only: false,
        services_only: false,

        separate_cursor: false,
        hostname,
        os: crate::OperatingSystem::local(),
    };
    // The head is 17 bytes: tag, version, codec, quality, then the decode,
    // files-only, services-only and cursor flags. An absent name is one
    // length byte, and the OS byte ends the frame.
    // An invalid name encodes as absent; it never breaks the session.
    let encoded = hello(Some("  ".to_string())).encode();
    assert_eq!(encoded.len(), 19);
    assert_eq!(ControlMessage::decode(&encoded).unwrap(), hello(None));
    // Files-only round-trips its flag.
    let mut files_hello = hello(Some("mac-mini".to_string()));
    if let ControlMessage::Hello {
        ref mut files_only, ..
    } = files_hello
    {
        *files_only = true;
    }
    let files_encoded = files_hello.encode();
    let back_files = ControlMessage::decode(&files_encoded).unwrap();
    assert!(matches!(
        back_files,
        ControlMessage::Hello {
            version: PROTOCOL_VERSION,
            files_only: true,
            services_only: false,
            ..
        }
    ));
    // Truncated, overlong, and non-canonical names are malformed.
    let mut truncated = hello(Some("mac-mini".to_string())).encode();
    truncated.pop();
    assert!(matches!(
        ControlMessage::decode(&truncated),
        Err(Error::Malformed)
    ));
    let mut trailing = hello(Some("mac-mini".to_string())).encode();
    trailing.push(0);
    assert!(matches!(
        ControlMessage::decode(&trailing),
        Err(Error::Malformed)
    ));
    let mut padded = hello(Some("mac-mini".to_string())).encode();
    // Rewrite the name bytes with a trailing space: not canonical.
    let end = padded.len();
    padded[end - 1] = b' ';
    assert!(matches!(
        ControlMessage::decode(&padded),
        Err(Error::Malformed)
    ));
    let mut bad_utf8 = hello(Some("mac-mini".to_string())).encode();
    let end = bad_utf8.len();
    bad_utf8[end - 1] = 0xFF;
    assert!(matches!(
        ControlMessage::decode(&bad_utf8),
        Err(Error::Malformed)
    ));
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
        ControlMessage::decode(&[1, 0, 1]),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&[2, 0]),
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
        Error::Unsupported("the host cannot share its desktop")
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
        InputEvent::pointer(0, 0, 1, 1),
        InputEvent::pointer(-32768, 32768, 2, 9),
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

#[test]
fn hello_mode_and_cursor_flags_are_strict() {
    let hello = ControlMessage::Hello {
        version: PROTOCOL_VERSION,
        codec: 0x40 | 1,
        quality: Quality::new(2000, 30, 1920, 1080).unwrap(),
        decode: false,
        files_only: true,
        services_only: true,
        separate_cursor: true,
        hostname: Some("service-host".into()),
        os: crate::OperatingSystem::local(),
    };
    let bytes = hello.encode();
    assert_eq!((bytes[14], bytes[15], bytes[16]), (1, 1, 1));
    assert_eq!(ControlMessage::decode(&bytes).unwrap(), hello);
    for flag in [13, 14, 15, 16] {
        let mut malformed = bytes.clone();
        malformed[flag] = 2;
        assert!(matches!(
            ControlMessage::decode(&malformed),
            Err(Error::Malformed)
        ));
    }
    // A service-only connection must also be files-only.
    let mut desktop_services = bytes.clone();
    desktop_services[14] = 0;
    assert!(matches!(
        ControlMessage::decode(&desktop_services),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(&bytes[..18]),
        Err(Error::Malformed)
    ));
}

fn display(id: u64, origin_x: i32, width: u16, height: u16, rotation: u16) -> DisplayInfo {
    DisplayInfo {
        id,
        origin_x,
        origin_y: 0,
        width,
        height,
        rotation_deg: rotation,
        scale_percent: 100,
        revision: 4,
    }
}

#[test]
fn topology_and_one_display_selection_round_trip() {
    let displays = vec![
        display(1, -1920, 1920, 1080, 0),
        display(2, 0, 1280, 800, 90),
    ];
    let message = ControlMessage::Topology {
        stamp: SessionStamp::new(9, 2),
        revision: 4,
        displays: displays.clone(),
    };
    assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    assert!(message.encode().len() <= MAX_CONTROL_FRAME);

    let wide = (0..10)
        .map(|index| DisplayInfo {
            id: index + 1,
            origin_x: i32::try_from(index).unwrap() * 200,
            origin_y: 0,
            width: 200,
            height: 100,
            rotation_deg: 0,
            scale_percent: 100,
            revision: 4,
        })
        .collect::<Vec<_>>();
    let wide_message = ControlMessage::Topology {
        stamp: SessionStamp::new(9, 2),
        revision: 4,
        displays: wide,
    };
    let encoded = wide_message.encode();
    assert!(encoded.len() > MAX_CONTROL_FRAME);
    assert!(encoded.len() <= MAX_TOPOLOGY_FRAME);
    assert_eq!(ControlMessage::decode(&encoded).unwrap(), wide_message);

    let empty = ControlMessage::Topology {
        stamp: SessionStamp::new(9, 2),
        revision: 4,
        displays: Vec::new(),
    };
    assert_eq!(ControlMessage::decode(&empty.encode()).unwrap(), empty);

    let report = ControlMessage::VideoReport {
        stamp: SessionStamp::new(9, 2),
        report: crate::video::VideoReport {
            frames_completed: 28,
            frames_lost: 2,
            fragments_received: 900,
            fragments_missing: 11,
            fragments_repaired: 0,
        },
    };
    let encoded = report.encode();
    assert_eq!(ControlMessage::decode(&encoded).unwrap(), report);
    assert!(matches!(
        ControlMessage::decode(&encoded[..encoded.len() - 1]),
        Err(Error::Malformed)
    ));

    let selection = ControlMessage::SelectDisplays {
        stamp: SessionStamp::new(9, 2),
        displays: vec![2],
    };
    assert_eq!(
        ControlMessage::decode(&selection.encode()).unwrap(),
        selection
    );
    let mut duplicate = selection.encode();
    duplicate[17] = 2;
    duplicate.extend_from_slice(&2u64.to_be_bytes());
    assert!(matches!(
        ControlMessage::decode(&duplicate),
        Err(Error::Malformed)
    ));

    let mut old_pointer = vec![4];
    old_pointer.extend_from_slice(&9u64.to_be_bytes());
    old_pointer.extend_from_slice(&2u64.to_be_bytes());
    old_pointer.extend_from_slice(&[3, 0, 1, 0, 2]);
    assert!(matches!(
        ControlMessage::decode(&old_pointer),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(
            &ControlMessage::Input {
                stamp: SessionStamp::new(9, 2),
                event: InputEvent::pointer(1, 2, 0, 4),
            }
            .encode()
        ),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        ControlMessage::decode(
            &ControlMessage::Input {
                stamp: SessionStamp::new(9, 2),
                event: InputEvent::pointer(1, 2, 1, 0),
            }
            .encode()
        ),
        Err(Error::Malformed)
    ));
}

#[test]
fn handshake_os_is_bounded_and_every_truncation_is_rejected_without_panicking() {
    for os in [
        crate::OperatingSystem::Macos,
        crate::OperatingSystem::Windows,
        crate::OperatingSystem::Linux,
        crate::OperatingSystem::Other,
    ] {
        let quality = Quality::new(2_000, 30, 1920, 1080).unwrap();
        for message in [
            ControlMessage::Hello {
                version: PROTOCOL_VERSION,
                codec: crate::protocol::CODEC_H264,
                quality,
                decode: true,
                files_only: true,
                services_only: true,

                separate_cursor: false,
                hostname: Some("workstation".into()),
                os,
            },
            ControlMessage::Accept {
                stamp: SessionStamp::new(1, 1),
                monitor_width: 0,
                monitor_height: 0,
                codec: 0,
                quality,
                hostname: Some("workstation".into()),
                os,
            },
        ] {
            let mut bytes = message.encode();
            assert_eq!(ControlMessage::decode(&bytes).unwrap(), message);
            for length in 0..bytes.len() {
                assert!(
                    ControlMessage::decode(&bytes[..length]).is_err(),
                    "accepted truncated handshake {length}"
                );
            }
            *bytes.last_mut().unwrap() = 4;
            assert!(matches!(
                ControlMessage::decode(&bytes),
                Err(Error::Malformed)
            ));
        }
    }
}
