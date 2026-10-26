use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iroh::{RelayMode, SecretKey};

use crate::address::{self, Announcement};
use crate::core::{Core, CoreConfig, Listen};
use crate::error::Error;
use crate::installation::MemoryStore;
use crate::invitation::{self, MAX_DIRECT_ADDRS};
use crate::protocol::ControlMessage;
use crate::relay::RelaySettings;
use crate::session::{Grants, SessionPhase};
use crate::trust::Device;

fn start_direct() -> Core {
    let core = Core::boot(
        CoreConfig {
            listen: Listen::Loopback,
            ..CoreConfig::default()
        },
        RelayMode::Disabled,
    )
    .expect("loopback core");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

fn open_stored(store: &MemoryStore) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0())
        .expect("store opens");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

fn stale() -> SocketAddr {
    "203.0.113.9:9".parse().unwrap()
}

fn invitation_with(core: &Core, extra: Option<SocketAddr>) -> (String, Vec<SocketAddr>) {
    let token = core.create_invitation(Duration::from_secs(60)).unwrap();
    let (opened, _) = invitation::open(&token, SystemTime::now()).unwrap();
    let real = opened.direct.clone();
    let mut direct = real.clone();
    if let Some(extra) = extra {
        direct.push(extra);
    }
    let secret = SecretKey::from_bytes(&core.export_identity());
    let (token, _) = invitation::issue(
        &secret,
        &opened.relays,
        &direct,
        Duration::from_secs(60),
        SystemTime::now(),
        opened.hostname,
    )
    .unwrap();
    (token, real)
}

fn pair(host: &Core, controller: &Core, extra: Option<SocketAddr>) -> Vec<SocketAddr> {
    host.start_hosting().unwrap();
    let (host_token, host_direct) = invitation_with(host, extra);
    controller.accept_invitation(&host_token).unwrap();
    let (controller_token, _) = invitation_with(controller, extra);
    host.accept_invitation(&controller_token).unwrap();
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();
    host_direct
}

fn device<'a>(core: &'a Core, endpoint_id: &str) -> crate::DeviceInfo {
    core.snapshot()
        .unwrap()
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
        .expect("paired device")
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if ready() {
            return;
        }
        if std::time::Instant::now() > deadline {
            panic!("condition was not met");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn an_announcement_round_trips_and_keeps_grants() {
    let relays = vec!["https://relay.example.com".to_string()];
    let direct = vec!["127.0.0.1:9".parse().unwrap(), "[::1]:9".parse().unwrap()];
    let announcement = address::canonical(&relays, &direct).unwrap();
    let bytes = address::encode(&announcement.relays, &announcement.direct);
    assert_eq!(address::decode(&bytes).unwrap(), announcement);
    let message = ControlMessage::Addresses {
        relays: announcement.relays.clone(),
        direct: announcement.direct.clone(),
    };
    assert_eq!(ControlMessage::decode(&message.encode()).unwrap(), message);
    assert_eq!(
        ControlMessage::decode(&ControlMessage::Close.encode()).unwrap(),
        ControlMessage::Close
    );

    let mut device = Device {
        endpoint_id: [4; 32],
        display_id: "peer".to_string(),
        relays: vec!["https://old.example.com".to_string()],
        direct: vec!["198.51.100.8:8".parse().unwrap()],
    };
    address::install(&mut device, &announcement).unwrap();
    assert_eq!(device.endpoint_id, [4; 32]);
    assert_eq!(device.relays, announcement.relays);
    assert!(!device.direct.contains(&"198.51.100.8:8".parse().unwrap()));

    let mut too_many = vec!["203.0.113.1:1".parse().unwrap()];
    for port in 2..=MAX_DIRECT_ADDRS as u16 + 1 {
        too_many.push(SocketAddr::from(([203, 0, 113, 2], port)));
    }
    assert!(matches!(
        address::canonical(&[], &too_many),
        Err(Error::Malformed)
    ));
    assert!(matches!(
        address::decode(b"\x06\x01\x00\x05http:extra"),
        Err(Error::Malformed)
    ));
    let _ = Announcement {
        relays: Vec::new(),
        direct: Vec::new(),
    };
}

#[test]
fn a_session_replaces_stale_addresses_and_keeps_the_grant() {
    let host = start_direct();
    let controller = start_direct();
    let host_direct = pair(&host, &controller, Some(stale()));
    assert!(device(&controller, &host.endpoint_id())
        .direct
        .contains(&stale()));
    controller.connect(&host.endpoint_id()).unwrap();
    wait_until(|| {
        let saved = device(&controller, &host.endpoint_id());
        !saved.direct.contains(&stale()) && saved.direct == host_direct
    });
    wait_until(|| {
        !device(&host, &controller.endpoint_id())
            .direct
            .contains(&stale())
    });
    // The controller never granted the host. The host's control bit must
    // survive the address replacement, and the announcement must not add one.
    let saved = device(&controller, &host.endpoint_id());
    assert_eq!(saved.grants, Grants::default());
    assert!(saved.relays.is_empty());
    let granted = device(&host, &controller.endpoint_id());
    assert!(granted.grants.control);
    assert!(!granted.grants.clipboard);
    assert!(!granted.grants.transfer);
}

#[test]
fn a_failed_save_keeps_the_previous_addresses() {
    let host_store = MemoryStore::empty();
    let controller_store = MemoryStore::empty();
    let host = open_stored(&host_store);
    let controller = open_stored(&controller_store);
    pair(&host, &controller, Some(stale()));
    controller_store.fail_next_save(Error::StoreLocked);
    controller.connect(&host.endpoint_id()).unwrap();
    wait_until(|| {
        !device(&host, &controller.endpoint_id())
            .direct
            .contains(&stale())
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(device(&controller, &host.endpoint_id())
        .direct
        .contains(&stale()));
    assert_eq!(
        controller.snapshot().unwrap().outgoing.unwrap().phase,
        SessionPhase::Active
    );
}

#[test]
fn refreshed_addresses_survive_a_restart() {
    let host_store = MemoryStore::empty();
    let controller_store = MemoryStore::empty();
    let host = open_stored(&host_store);
    let controller = open_stored(&controller_store);
    let host_direct = pair(&host, &controller, Some(stale()));
    controller.connect(&host.endpoint_id()).unwrap();
    wait_until(|| {
        let saved = device(&controller, &host.endpoint_id());
        !saved.direct.contains(&stale()) && saved.direct == host_direct
    });
    wait_until(|| {
        !device(&host, &controller.endpoint_id())
            .direct
            .contains(&stale())
    });
    let saved = device(&controller, &host.endpoint_id());
    let granted = device(&host, &controller.endpoint_id());
    assert!(granted.grants.control);
    drop(controller);
    drop(host);
    let controller = open_stored(&controller_store);
    let restored = device(&controller, &saved.endpoint_id);
    assert_eq!(restored.direct, saved.direct);
    assert_eq!(restored.relays, saved.relays);
    assert_eq!(restored.grants, Grants::default());
    let host = open_stored(&host_store);
    let restored_grant = device(&host, &granted.endpoint_id);
    assert_eq!(restored_grant.direct, granted.direct);
    assert_eq!(restored_grant.relays, granted.relays);
    assert!(restored_grant.grants.control);
    assert!(!restored_grant.grants.clipboard);
    assert!(!restored_grant.grants.transfer);
}
