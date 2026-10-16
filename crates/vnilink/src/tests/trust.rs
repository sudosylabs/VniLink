use crate::error::Error;
use crate::session::Grant;
use crate::trust::{Device, Trust};

fn device(byte: u8) -> Device {
    let mut endpoint_id = [0u8; 32];
    endpoint_id[0] = byte;
    Device {
        endpoint_id,
        display_id: format!("{byte}"),
        relays: Vec::new(),
        direct: Vec::new(),
    }
}

#[test]
fn global_consent_applies_to_all_known_peers_and_never_unknown_peers() {
    let mut trust = Trust::default();
    let a = device(1);
    let aid = a.endpoint_id;
    let b = device(2);
    let bid = b.endpoint_id;
    trust.pair(a).unwrap();
    trust.pair(b).unwrap();
    assert!(!trust.granted(&aid, Grant::Control));
    trust.sharing = crate::SharingPolicy {
        view: true,
        control: true,
        terminal: true,
        ..Default::default()
    };
    assert!(trust.granted(&aid, Grant::Control) && trust.granted(&bid, Grant::Control));
    assert_eq!(
        trust.service_permissions(&aid).unwrap(),
        trust.service_permissions(&bid).unwrap()
    );
    assert!(!trust.granted(&device(3).endpoint_id, Grant::Control));
    trust.remove(&aid);
    assert!(!trust.granted(&aid, Grant::Control));
    assert!(trust.granted(&bid, Grant::Control));
    trust.sharing.control = false;
    assert!(!trust.granted(&bid, Grant::Control));
}

#[test]
fn the_directory_holds_128_devices_and_replacing_one_does_not_grow_it() {
    let mut trust = Trust::default();
    for byte in 0u8..=127 {
        trust.pair(device(byte)).unwrap();
    }
    let mut extra = device(0);
    extra.endpoint_id[1] = 1;
    assert!(matches!(trust.pair(extra), Err(Error::TooManyDevices(128))));
    let mut replaced = device(0);
    replaced.relays = vec!["https://relay.example.com/".to_owned()];
    trust.pair(replaced).unwrap();
    let id = [0u8; 32];
    assert_eq!(
        trust.get(&id).unwrap().relays,
        vec!["https://relay.example.com/"]
    );
}
