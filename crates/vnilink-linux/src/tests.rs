use vnilink_media::Error;

use crate::encode::LinuxEncoder;
use crate::portal::{PortalPhase, PortalSession, ONE_PORTAL_SESSION_IS_UNIVERSAL};
use crate::x11::{raw_frame_from_ximage, X11Image, X11Plan};

#[test]
fn portal_phases_follow_the_screen_cast_sequence() {
    assert!(!ONE_PORTAL_SESSION_IS_UNIVERSAL);
    let mut session = PortalSession::new();
    assert_eq!(session.phase(), PortalPhase::Idle);
    assert!(matches!(
        session.select_monitor(),
        Err(Error::InvalidSetting(
            "portal sources were selected before the session existed"
        ))
    ));
    assert!(matches!(
        session.start(1),
        Err(Error::InvalidSetting(
            "portal stream started before a monitor was selected"
        ))
    ));
    assert!(matches!(
        session.close(),
        Err(Error::InvalidSetting("portal session is not open"))
    ));
    session.create_session().unwrap();
    assert!(matches!(
        session.create_session(),
        Err(Error::InvalidSetting("portal session was already created"))
    ));
    session.select_monitor().unwrap();
    session.start(7).unwrap();
    assert_eq!(session.phase(), PortalPhase::Started);
    assert_eq!(session.pipewire_node(), Some(7));
    session.close().unwrap();
    assert_eq!(session.phase(), PortalPhase::Closed);
    assert!(session.pipewire_node().is_none());
    assert!(matches!(
        session.start(7),
        Err(Error::InvalidSetting(
            "portal stream started before a monitor was selected"
        ))
    ));
}

#[test]
fn a_restore_token_is_kept_out_of_debug() {
    let mut session = PortalSession::new();
    assert!(matches!(
        session.set_restore_token(""),
        Err(Error::InvalidSetting("restore token is empty"))
    ));
    session.set_restore_token("secret-token").unwrap();
    assert_eq!(session.restore_token(), Some("secret-token"));
    let rendered = format!("{session:?}");
    assert!(rendered.contains("has_restore_token"));
    assert!(!rendered.contains("secret-token"));
}

#[test]
fn x11_images_must_be_little_endian_bgra() {
    let pixels = [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let frame = raw_frame_from_ximage(
        X11Image {
            width: 2,
            height: 2,
            stride: 8,
            depth: 24,
            bits_per_pixel: 32,
            lsb_first: true,
            bytes: &pixels,
        },
        5,
    )
    .unwrap();
    assert_eq!(frame.timestamp_us(), 5);
    assert_eq!(frame.row(1), &pixels[8..16]);

    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 16,
                bits_per_pixel: 16,
                lsb_first: true,
                bytes: &pixels,
            },
            0,
        ),
        Err(Error::Unsupported("this X11 pixel format"))
    ));
    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 24,
                bits_per_pixel: 32,
                lsb_first: false,
                bytes: &pixels,
            },
            0,
        ),
        Err(Error::Unsupported("this X11 pixel format"))
    ));
    assert!(matches!(
        raw_frame_from_ximage(
            X11Image {
                width: 2,
                height: 2,
                stride: 8,
                depth: 24,
                bits_per_pixel: 32,
                lsb_first: true,
                bytes: &[0, 1, 2, 3],
            },
            0,
        ),
        Err(Error::InvalidFrame("frame is missing pixels"))
    ));
    assert_eq!(X11Plan::current_session().screen, 0);
    assert!(X11Plan::current_session().display_name.is_none());
}

#[test]
fn the_software_encoder_is_not_linked() {
    assert!(matches!(
        LinuxEncoder::open(64, 64),
        Err(Error::Unsupported("VA-API H.264"))
    ));
}
