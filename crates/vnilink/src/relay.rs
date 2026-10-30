// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

use iroh::{RelayConfig, RelayMap, RelayMode, RelayUrl};

use crate::error::Error;

pub(crate) const MAX_CUSTOM_RELAYS: usize = 8;
const MAX_RELAY_URL_BYTES: usize = 512;
const MAX_AUTH_TOKEN_BYTES: usize = 256;

/// Where this installation sends relay traffic.
///
/// The default is the public n0 set (`RelayMode::Default`). A custom list
/// replaces that set for this installation only.
#[derive(Clone, PartialEq, Eq)]
pub enum RelaySettings {
    Public,
    Custom(Vec<CustomRelay>),
}

/// One relay this installation will use instead of the public n0 set.
#[derive(Clone, PartialEq, Eq)]
pub struct CustomRelay {
    url: String,
    auth_token: Option<String>,
}

impl fmt::Debug for RelaySettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Public => formatter.write_str("Public"),
            Self::Custom(relays) => formatter.debug_tuple("Custom").field(relays).finish(),
        }
    }
}

impl fmt::Debug for CustomRelay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CustomRelay")
            .field("url", &self.url)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl Default for RelaySettings {
    fn default() -> Self {
        Self::Public
    }
}

impl RelaySettings {
    pub fn public_n0() -> Self {
        Self::Public
    }

    pub fn custom(relays: Vec<CustomRelay>) -> Result<Self, Error> {
        if relays.is_empty() {
            return Err(Error::InvalidRelay(
                "custom relay mode requires at least one relay URL",
            ));
        }
        if relays.len() > MAX_CUSTOM_RELAYS {
            return Err(Error::InvalidRelay(
                "custom relay mode supports at most 8 relay URLs",
            ));
        }
        let _validated = canonical_relays(&relays)?;
        Ok(Self::Custom(relays))
    }

    pub fn is_public(&self) -> bool {
        matches!(self, Self::Public)
    }

    /// Relay URLs a peer must be able to dial. Empty for the public default.
    pub fn relay_urls(&self) -> Result<Vec<String>, Error> {
        match self {
            Self::Public => Ok(Vec::new()),
            Self::Custom(relays) => Ok(canonical_relays(relays)?
                .into_iter()
                .map(|url| url.to_string())
                .collect()),
        }
    }

    pub(crate) fn to_relay_mode(&self) -> Result<RelayMode, Error> {
        match self {
            Self::Public => Ok(RelayMode::Default),
            Self::Custom(relays) => {
                let configs = canonical_relays(relays)?;
                let map = RelayMap::from_iter(configs);
                Ok(RelayMode::Custom(map))
            }
        }
    }
}

impl CustomRelay {
    pub fn new(url: impl Into<String>) -> Result<Self, Error> {
        let relay = Self {
            url: url.into(),
            auth_token: None,
        };
        canonical_relay(&relay, 0)?;
        Ok(relay)
    }

    pub fn with_auth_token(mut self, token: impl Into<String>) -> Result<Self, Error> {
        let token = token.into();
        validate_token(&token)?;
        self.auth_token = Some(token);
        Ok(self)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn has_auth_token(&self) -> bool {
        self.auth_token.is_some()
    }

    pub(crate) fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }
}

fn canonical_relays(relays: &[CustomRelay]) -> Result<Vec<RelayConfig>, Error> {
    let mut configs = Vec::with_capacity(relays.len());
    let mut seen = Vec::with_capacity(relays.len());
    for (index, relay) in relays.iter().enumerate() {
        let url = canonical_relay(relay, index)?;
        if seen.iter().any(|existing: &RelayUrl| existing == &url) {
            return Err(Error::InvalidRelayUrl {
                index,
                reason: "duplicate relay URL".to_owned(),
            });
        }
        seen.push(url.clone());
        let mut config = if url.scheme() == "http" {
            RelayConfig::new(url, None)
        } else {
            RelayConfig::from(url)
        };
        if let Some(token) = &relay.auth_token {
            validate_token(token)?;
            config = config.with_auth_token(token.clone());
        }
        configs.push(config);
    }
    Ok(configs)
}

pub(crate) fn canonical_relay(relay: &CustomRelay, index: usize) -> Result<RelayUrl, Error> {
    validate_url(&relay.url, index)?;
    let url = RelayUrl::from_str(&relay.url).map_err(|err| Error::InvalidRelayUrl {
        index,
        reason: err.to_string(),
    })?;
    validate_parsed(&url, index)?;
    Ok(url)
}

pub(crate) fn parse_relay_url(value: &str) -> Result<RelayUrl, Error> {
    let relay = CustomRelay {
        url: value.to_owned(),
        auth_token: None,
    };
    canonical_relay(&relay, 0)
}

fn validate_url(value: &str, index: usize) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > MAX_RELAY_URL_BYTES
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL must be non-empty and contain no whitespace or control characters"
                .to_owned(),
        });
    }
    Ok(())
}

fn validate_parsed(url: &RelayUrl, index: usize) -> Result<(), Error> {
    let secure = url.scheme() == "https";
    let loopback_http = url.scheme() == "http" && host_is_loopback(url);
    if !secure && !loopback_http {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL must use HTTPS; HTTP is allowed only for a loopback relay".to_owned(),
        });
    }
    if url.host_str().is_none() {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL must include a host".to_owned(),
        });
    }
    if url.port() == Some(0) {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL port must be between 1 and 65535".to_owned(),
        });
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL must not contain credentials".to_owned(),
        });
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(Error::InvalidRelayUrl {
            index,
            reason: "URL must not contain a query or fragment".to_owned(),
        });
    }
    Ok(())
}

fn host_is_loopback(url: &RelayUrl) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.parse::<IpAddr>()
        .is_ok_and(|address| address.is_loopback())
}

fn validate_token(token: &str) -> Result<(), Error> {
    if token.is_empty()
        || token.len() > MAX_AUTH_TOKEN_BYTES
        || token
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(Error::InvalidRelay(
            "relay credential must be non-empty and contain no whitespace",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_mode_uses_the_n0_default() {
        let mode = RelaySettings::public_n0().to_relay_mode().unwrap();
        assert!(matches!(mode, RelayMode::Default));
        assert!(RelaySettings::public_n0().relay_urls().unwrap().is_empty());
    }

    #[test]
    fn custom_mode_rejects_an_empty_list() {
        let error = RelaySettings::custom(Vec::new()).unwrap_err();
        assert!(matches!(error, Error::InvalidRelay(_)));
    }

    #[test]
    fn custom_mode_builds_a_relay_map_and_hides_the_token() {
        let relay = CustomRelay::new("https://relay.example.com./")
            .unwrap()
            .with_auth_token("secret-token")
            .unwrap();
        let settings = RelaySettings::custom(vec![relay]).unwrap();
        let mode = settings.to_relay_mode().unwrap();
        assert!(matches!(mode, RelayMode::Custom(_)));
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains("secret-token"));
        assert!(rendered.contains("[redacted]"));
    }

    #[test]
    fn http_is_limited_to_loopback() {
        assert!(CustomRelay::new("http://127.0.0.1:3340").is_ok());
        assert!(CustomRelay::new("http://127.0.0.1").is_ok());
        assert!(CustomRelay::new("http://localhost:80").is_ok());
        assert!(CustomRelay::new("http://LOCALHOST:80").is_ok());
        assert!(CustomRelay::new("http://[::1]:3340").is_ok());
        assert!(CustomRelay::new("http://relay.example.com").is_err());
        assert!(CustomRelay::new("https://user:pass@relay.example.com").is_err());
    }

    #[test]
    fn custom_mode_rejects_too_many_and_duplicate_urls() {
        let eight: Vec<_> = (0..8)
            .map(|index| CustomRelay::new(format!("https://relay{index}.example.com")).unwrap())
            .collect();
        let settings = RelaySettings::custom(eight).unwrap();
        assert_eq!(settings.relay_urls().unwrap().len(), 8);
        assert!(!settings.is_public());

        let nine: Vec<_> = (0..9)
            .map(|index| CustomRelay::new(format!("https://relay{index}.example.com")).unwrap())
            .collect();
        assert!(matches!(
            RelaySettings::custom(nine),
            Err(Error::InvalidRelay(_))
        ));

        let duplicate = vec![
            CustomRelay::new("https://relay.example.com").unwrap(),
            CustomRelay::new("https://relay.example.com").unwrap(),
        ];
        match RelaySettings::custom(duplicate) {
            Err(Error::InvalidRelayUrl { index: 1, reason }) => {
                assert!(reason.contains("duplicate"));
            }
            other => panic!("duplicate relays returned {other:?}"),
        }
    }

    #[test]
    fn relay_urls_reject_blank_insecure_and_decorated_forms() {
        let rejected = [
            "",
            " ",
            "https://relay.example.com/ ",
            "https://relay. example.com",
            "https://relay.example.com/\n",
            "https://relay.example.com/\u{0001}",
            "ftp://relay.example.com",
            "https://",
            "https://relay.example.com:0",
            "https://relay.example.com/?token=1",
            "https://relay.example.com/#frag",
            "https://user@relay.example.com",
        ];
        for url in rejected {
            assert!(
                CustomRelay::new(url).is_err(),
                "accepted invalid relay URL {url:?}"
            );
        }
        let too_long = format!("https://{}.example.com", "a".repeat(500));
        assert!(too_long.len() > 512);
        assert!(matches!(
            CustomRelay::new(too_long),
            Err(Error::InvalidRelayUrl { index: 0, .. })
        ));
    }

    #[test]
    fn relay_credentials_reject_empty_whitespace_and_overlong_tokens() {
        let relay = CustomRelay::new("https://relay.example.com").unwrap();
        assert!(!relay.has_auth_token());
        assert!(relay.clone().with_auth_token("").is_err());
        assert!(relay.clone().with_auth_token("secret token").is_err());
        assert!(relay.clone().with_auth_token("secret\ntoken").is_err());
        assert!(relay.clone().with_auth_token("a".repeat(257)).is_err());

        let token = "b".repeat(256);
        let relay = relay.with_auth_token(token.clone()).unwrap();
        assert!(relay.has_auth_token());
        let settings = RelaySettings::custom(vec![relay]).unwrap();
        let rendered = format!("{settings:?}");
        assert!(!rendered.contains(&token));
        assert!(rendered.contains("[redacted]"));
        let urls = settings.relay_urls().unwrap();
        assert_eq!(urls.len(), 1);
        assert!(!urls[0].contains(&token));
    }
}
