use iroh::RelayMode;

use crate::error::Error;
use crate::relay::{CustomRelay, RelaySettings};

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
