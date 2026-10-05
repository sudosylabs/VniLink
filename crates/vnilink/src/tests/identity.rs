use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use iroh::{PublicKey, RelayMode};

use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::events::CoreEvent;
use crate::input::InputEvent;
use crate::invitation;
use crate::protocol::Quality;
use crate::relay::RelaySettings;
use crate::session::{Grants, SessionRole};
use crate::video::EncodedFrame;

use super::support::*;

#[test]
fn a_fresh_loopback_core_starts_idle() {
    let core = start_direct();
    let snapshot = core.snapshot().unwrap();
    assert_eq!(snapshot.endpoint_id, core.endpoint_id());
    assert!(snapshot.public_relay);
    assert!(snapshot.relay_urls.is_empty());
    assert!(!snapshot.hosting);
    assert!(snapshot.devices.is_empty());
    assert!(snapshot.incoming.is_none());
    assert!(snapshot.outgoing.is_none());
    assert!(!snapshot.capabilities.keyboard);
    assert!(!snapshot.capabilities.pointer);
    assert!(!snapshot.capabilities.desktop_capture);
    assert_eq!(snapshot.capabilities.monitors, 0);
}

#[test]
fn shutdown_is_idempotent_and_later_calls_report_it() {
    let core = start_direct();
    let other = start_direct();
    let token = other.create_invitation(Duration::from_secs(60)).unwrap();
    let id = core.endpoint_id();
    let secret = core.export_identity();
    core.shutdown();
    core.shutdown();
    assert_eq!(core.endpoint_id(), id);
    assert_eq!(core.export_identity(), secret);
    assert!(matches!(core.snapshot(), Err(Error::ShutDown)));
    assert!(matches!(core.subscribe(), Err(Error::ShutDown)));
    assert!(matches!(core.start_hosting(), Err(Error::ShutDown)));
    assert!(matches!(core.stop_hosting(), Err(Error::ShutDown)));
    assert!(matches!(core.stop_hosted_session(), Err(Error::ShutDown)));
    assert!(matches!(
        core.create_invitation(Duration::from_secs(60)),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.accept_invitation(&token),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.connect(&other.endpoint_id()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(core.disconnect(), Err(Error::ShutDown)));
    assert!(matches!(
        core.submit_input(1, 1, InputEvent::pointer(0, 0)),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.set_desktop(Box::new(FixedDesktop {
            width: 0,
            height: 0,
            events: Arc::new(Mutex::new(Vec::new())),
            fail_key_down: None,
            fail_release: false,
        })),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.authorize(&other.endpoint_id(), Grants::default()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.revoke(&other.endpoint_id()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.update_session_quality(1, SessionRole::Host, Quality::new(1, 1, 1, 1).unwrap()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(
        core.publish_video(1, EncodedFrame::h264(true, 0, vec![1]).unwrap()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(core.subscribe_video(), Err(Error::ShutDown)));
    assert!(matches!(
        core.set_capture(Box::new(ShutdownCapture)),
        Err(Error::ShutDown)
    ));
}

#[test]
fn event_capacity_must_be_between_1_and_1024() {
    assert!(matches!(
        boot_capacity(0),
        Err(Error::InvalidSetting("event capacity must be at least 1"))
    ));
    assert!(matches!(
        boot_capacity(1025),
        Err(Error::InvalidSetting("event capacity must be at most 1024"))
    ));
    let core = boot_capacity(1).unwrap();
    let events = core.subscribe().unwrap();
    core.start_hosting().unwrap();
    core.stop_hosting().unwrap();
    assert!(!core.snapshot().unwrap().hosting);
    assert_eq!(events.lagged(), 1);
    assert_eq!(
        events.try_recv(),
        Some(CoreEvent::HostingChanged { hosting: true })
    );
    assert!(boot_capacity(1024).is_ok());
}

#[test]
fn restored_identity_keeps_the_same_endpoint_id() {
    let original = start_direct();
    let bytes = original.export_identity();
    let id = original.endpoint_id();
    drop(original);
    let restored = Core::boot(
        CoreConfig {
            identity: Some(bytes),
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    assert_eq!(restored.export_identity(), bytes);
    assert_eq!(restored.endpoint_id(), id);
    assert_ne!(start_direct().endpoint_id(), id);
    let rendered = format!(
        "{:?}",
        CoreConfig {
            identity: Some([0x11; 32]),
            ..CoreConfig::default()
        }
    );
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("17"));
}

#[test]
fn an_installation_cannot_pair_with_itself() {
    let core = start_direct();
    let token = core.create_invitation(Duration::from_secs(60)).unwrap();
    assert!(matches!(
        core.accept_invitation(&token),
        Err(Error::InvalidInvitation(
            "an installation cannot pair with itself"
        ))
    ));
}

#[test]
fn unknown_devices_and_garbage_ids_are_rejected() {
    let host = start_direct();
    let stranger = start_direct();
    let control = Grants {
        control: true,
        clipboard: false,
        transfer: false,
    };
    assert!(matches!(
        host.authorize(&stranger.endpoint_id(), control),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        host.revoke(&stranger.endpoint_id()),
        Err(Error::NotPaired)
    ));
    for id in ["", "not-a-key", "zzzz"] {
        assert!(
            matches!(
                host.authorize(id, Grants::default()),
                Err(Error::InvalidSetting(_))
            ),
            "{id}"
        );
        assert!(
            matches!(host.connect(id), Err(Error::InvalidSetting(_))),
            "{id}"
        );
        assert!(
            matches!(host.revoke(id), Err(Error::InvalidSetting(_))),
            "{id}"
        );
    }
}

#[test]
fn a_loopback_invitation_carries_a_loopback_address() {
    let host = start_direct();
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
    assert!(opened.relays.is_empty());
    assert_eq!(opened.direct.len(), 1);
    assert_eq!(
        opened.direct[0].ip(),
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
    );
    assert_ne!(opened.direct[0].port(), 0);
    let key = PublicKey::from_str(&host.endpoint_id()).unwrap();
    assert_eq!(opened.endpoint_id, *key.as_bytes());
}

#[test]
fn a_custom_relay_url_is_advertised_without_contacting_it() {
    let relay = crate::relay::CustomRelay::new("https://relay.example.test")
        .unwrap()
        .with_auth_token("super-secret-token")
        .unwrap();
    let settings = RelaySettings::custom(vec![relay]).unwrap();
    let urls = settings.relay_urls().unwrap();
    let rendered = format!("{settings:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains("super-secret-token"));
    let host = Core::boot(
        CoreConfig {
            relay: settings,
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .unwrap();
    let snapshot = host.snapshot().unwrap();
    assert!(!snapshot.public_relay);
    assert_eq!(snapshot.relay_urls, urls);
    let token = host.create_invitation(Duration::from_secs(60)).unwrap();
    assert!(!token.contains("super-secret-token"));
    let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
    assert_eq!(opened.relays, urls);
}
