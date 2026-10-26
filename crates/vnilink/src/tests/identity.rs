use std::str::FromStr;
use std::time::{Duration, SystemTime};

use iroh::{PublicKey, RelayMode};

use crate::core::{Core, CoreConfig, Listen};
use crate::invitation;
use crate::relay::RelaySettings;

use super::support::*;

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
