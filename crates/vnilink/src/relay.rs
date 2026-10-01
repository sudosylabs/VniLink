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
