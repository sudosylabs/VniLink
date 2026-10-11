use crate::error::Error;
use crate::session::{Grant, Grants};
use crate::trust::{Device, Trust};

fn device(byte: u8) -> Device {
    let mut endpoint_id = [0u8; 32];
    endpoint_id[0] = byte;
    Device {
        terminal: false,
        camera: false,
        system_audio: false,
        microphone: false,
        recording: false,
        system_actions: false,
        endpoint_id,
        display_id: format!("{byte}"),
        relays: Vec::new(),
        direct: Vec::new(),
        grants: Grants::default(),
    }
}

#[test]
fn grants_are_directional_records_on_the_host_that_stores_them() {
    let mut trust = Trust::default();
    let paired = device(1);
    trust.pair(paired).unwrap();
    let mut id = [0u8; 32];
    id[0] = 1;
    assert!(!trust.granted(&id, Grant::Control));
    trust
        .authorize(
            &id,
            Grants {
                view: true,
                control: true,
                clipboard: false,
                transfer: false,
            },
        )
        .unwrap();
    assert!(trust.granted(&id, Grant::Control));
    trust.authorize(&id, Grants::default()).unwrap();
    assert!(!trust.granted(&id, Grant::Control));
    let mut other = [0u8; 32];
    other[0] = 2;
    assert!(matches!(
        trust.authorize(&other, Grants::default()),
        Err(Error::NotPaired)
    ));
    assert!(matches!(
        trust.authorize(&other, Grants::default()),
        Err(Error::NotPaired)
    ));
    assert!(trust.get(&id).is_some());
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

    trust
        .authorize(
            &id,
            Grants {
                view: false,
                control: false,
                clipboard: true,
                transfer: true,
            },
        )
        .unwrap();
    assert!(!trust.granted(&id, Grant::Control));
    trust.authorize(&id, Grants::default()).unwrap();
    assert!(trust.get(&id).is_some());
    assert!(!trust.granted(&id, Grant::Control));
    trust
        .authorize(
            &id,
            Grants {
                view: true,
                control: true,
                clipboard: false,
                transfer: false,
            },
        )
        .unwrap();
    assert!(trust.granted(&id, Grant::Control));
}
