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
