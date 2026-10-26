use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen, OpenConfig};
use crate::error::Error;
use crate::installation::{self, MemoryStore};
use crate::invitation::{self};
use crate::relay::{CustomRelay, RelaySettings};
use crate::session::Grants;

const TOKEN: &str = "relay-credential-alpha";

fn peer() -> Core {
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

fn open_loop(store: &MemoryStore, relay: RelaySettings) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), relay).expect("installation opens");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

fn credentialed_relay() -> RelaySettings {
    let relay = CustomRelay::new("https://relay.example.com./")
        .unwrap()
        .with_auth_token(TOKEN)
        .unwrap();
    RelaySettings::custom(vec![relay]).unwrap()
}

fn all_grants() -> Grants {
    Grants {
        view: true,
        control: true,
        clipboard: true,
        transfer: true,
    }
}

#[test]
fn reset_identity_forgets_trust_and_keeps_the_saved_relay() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(120))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    host.set_desktop_sharing(all_grants()).unwrap();
    let settings = credentialed_relay();
    host.set_relay(settings.clone()).unwrap();
    assert_eq!(host.relay_configuration().unwrap(), settings);
    assert!(!format!("{:?}", host.relay_configuration().unwrap()).contains(TOKEN));
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
    assert!(host.snapshot().unwrap().public_relay);
    let issued = host.create_invitation(Duration::from_secs(60)).unwrap();
    let (opened, _) = invitation::open(&issued, SystemTime::now()).unwrap();
    assert!(opened.relays.is_empty());
    assert!(!issued.contains(TOKEN));
    let old_id = host.endpoint_id();
    let old_secret = host.export_identity();
    host.reset_identity().unwrap();
    assert!(matches!(host.snapshot(), Err(Error::ShutDown)));
    assert_eq!(host.endpoint_id(), old_id);
    assert!(
        host.export_identity() == old_secret,
        "the stopped core dropped the key it bound"
    );
    drop(host);

    let host = open_loop(&store, RelaySettings::public_n0());
    assert_ne!(host.endpoint_id(), old_id);
    assert!(
        host.export_identity() != old_secret,
        "reset kept the previous identity"
    );
    let snapshot = host.snapshot().unwrap();
    assert!(snapshot.devices.is_empty());
    assert!(!snapshot.public_relay);
    assert_eq!(snapshot.relay_urls, settings.relay_urls().unwrap());
    assert!(!format!("{snapshot:?}").contains(TOKEN));
    assert_eq!(format!("{store:?}"), "MemoryStore");
    let saved = installation::decode(store.record().as_ref().unwrap()).unwrap();
    assert_eq!(saved.relay, settings);
    let rendered = format!("{saved:?}");
    assert!(rendered.contains("[redacted]"));
    assert!(!rendered.contains(TOKEN));
    let config = OpenConfig {
        initial_relay: settings,
        ..OpenConfig::default()
    };
    assert!(!format!("{config:?}").contains(TOKEN));

    let paired = host.accept_invitation(&invitation).unwrap();
    assert_eq!(paired.grants, Grants::default());
    assert_eq!(paired.endpoint_id, controller.endpoint_id());
}

#[test]
fn a_timed_out_endpoint_close_can_be_retried() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    host.arm_blocked_teardown(Duration::from_millis(40));
    store.fail_next_save(Error::StoreRecoveryRequired);
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::StoreRecoveryRequired)
    ));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while host.teardown_phase() == 1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host.teardown_phase(), 0);
    assert!(!host.endpoint_is_closed());
    assert!(matches!(host.start_hosting(), Err(Error::ShutDown)));
    host.shutdown();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !host.endpoint_is_closed() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(host.endpoint_is_closed());
    assert_eq!(host.teardown_phase(), 2);
    assert_eq!(host.endpoint_close_count(), 2);
}
