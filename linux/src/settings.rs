//! Settings model. Owns what the settings surface shows, not the widgets.

use vnilink::Snapshot;

/// Settings state read off a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub hosting: bool,
    pub endpoint_id: String,
    pub public_relay: bool,
    pub relay_urls: Vec<String>,
}

impl Settings {
    pub fn of(snapshot: &Snapshot) -> Self {
        Self {
            hosting: snapshot.hosting,
            endpoint_id: snapshot.endpoint_id.clone(),
            public_relay: snapshot.public_relay,
            relay_urls: snapshot.relay_urls.clone(),
        }
    }

    pub fn relay_summary(&self) -> String {
        crate::presentation::relay_summary(self.public_relay, &self.relay_urls)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vnilink::{Capabilities, Grants, SessionInfo, SessionPhase, SessionRole};

    #[test]
    fn settings_mirror_the_snapshot() {
        let snapshot = Snapshot {
            endpoint_id: "this".to_string(),
            public_relay: false,
            relay_urls: vec!["relay.example".to_string()],
            hosting: true,
            devices: Vec::new(),
            incoming: None,
            outgoing: None,
            capabilities: Capabilities::UNSUPPORTED,
        };
        let settings = Settings::of(&snapshot);
        assert!(settings.hosting);
        assert_eq!(settings.relay_summary(), "relay.example");
        let _ = (Grants::default(), SessionPhase::Closed, SessionRole::Host);
        let _ = SessionInfo {
            id: 0,
            generation: 0,
            role: SessionRole::Host,
            phase: SessionPhase::Closed,
            peer_id: String::new(),
            quality: None,
            monitor_width: 0,
            monitor_height: 0,
        };
    }
}

/// Preserve opaque credentials when a saved URL is unchanged. Disabling
/// authentication deliberately removes its credential; a new authenticated
/// relay must have a token.
pub fn relay_configuration(
    rows: &[(String, String, bool)],
    saved: &vnilink::RelaySettings,
) -> Result<vnilink::RelaySettings, vnilink::Error> {
    let mut relays = Vec::new();
    for (url, token, authenticated) in rows {
        if url.trim().is_empty() && token.is_empty() && !authenticated {
            continue;
        }
        let relay = vnilink::CustomRelay::new(url.trim())?;
        let relay = if *authenticated {
            if !token.is_empty() {
                relay.with_auth_token(token)?
            } else {
                let canonical =
                    vnilink::RelaySettings::custom(vec![relay.clone()])?.relay_urls()?;
                let previous = match saved {
                    vnilink::RelaySettings::Custom(previous) => previous,
                    _ => {
                        return Err(vnilink::Error::InvalidRelay(
                            "enter an authentication token",
                        ))
                    }
                };
                previous
                    .iter()
                    .find(|old| {
                        old.has_auth_token()
                            && vnilink::RelaySettings::custom(vec![(*old).clone()])
                                .and_then(|r| r.relay_urls())
                                .is_ok_and(|urls| urls == canonical)
                    })
                    .cloned()
                    .ok_or(vnilink::Error::InvalidRelay(
                        "enter an authentication token for this URL",
                    ))?
            }
        } else {
            relay
        };
        relays.push(relay);
    }
    vnilink::RelaySettings::custom(relays)
}

#[cfg(test)]
mod relay_tests {
    use super::relay_configuration;
    #[test]
    fn relay_form_validates_and_preserves_credentials_without_exposing_them() {
        let public = vnilink::RelaySettings::Public;
        let saved = relay_configuration(
            &[(
                " https://relay.example/ ".into(),
                "fixture-token".into(),
                true,
            )],
            &public,
        )
        .unwrap();
        assert_eq!(saved.relay_urls().unwrap(), vec!["https://relay.example/"]);
        assert!(!format!("{saved:?}").contains("fixture-token"));
        assert_eq!(
            relay_configuration(
                &[("https://relay.example/".into(), "".into(), true)],
                &saved
            )
            .unwrap(),
            saved
        );
        assert!(relay_configuration(
            &[("https://different.example/".into(), "".into(), true)],
            &saved
        )
        .is_err());
        let removed = relay_configuration(
            &[("https://relay.example/".into(), "".into(), false)],
            &saved,
        )
        .unwrap();
        if let vnilink::RelaySettings::Custom(relays) = removed {
            assert!(!relays[0].has_auth_token());
        } else {
            panic!("custom expected");
        }
        assert!(relay_configuration(&[], &public).is_err());
        assert!(relay_configuration(
            &[("http://relay.example/".into(), "".into(), false)],
            &public
        )
        .is_err());
        assert!(relay_configuration(
            &[(
                "https://relay.example/".into(),
                "invalid token".into(),
                true
            )],
            &public
        )
        .is_err());
        assert!(relay_configuration(
            &[
                ("https://relay.example/".into(), "".into(), false),
                ("https://relay.example/".into(), "".into(), false)
            ],
            &public
        )
        .is_err());
        assert!(
            relay_configuration(&[("".into(), "token-without-url".into(), true)], &public).is_err()
        );
    }
}
