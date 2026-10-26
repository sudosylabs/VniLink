use std::sync::Arc;

use iroh::SecretKey;

use crate::core::Core;
use crate::installation::{self, MemoryStore};
use crate::relay::RelaySettings;

use super::support::*;

fn open_loop(store: &MemoryStore, relay: RelaySettings) -> Core {
    let core = Core::open_direct(Arc::new(store.clone()), relay).expect("installation opens");
    core.set_video_codecs(crate::VideoCodecCaps::h264_full(4096, 4096, "test"))
        .expect("flags");
    core
}

#[test]
fn names_survive_a_restart_in_the_current_record() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = start_direct();
    pair_for_control(&host, &controller);
    controller.connect(&host.endpoint_id()).unwrap();
    let own = crate::hostname::local_hostname();
    assert_eq!(controller.peer_hostname(&host.endpoint_id()).unwrap(), own);

    // Reopening the store keeps the names.
    drop(host);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(
        reopened.peer_hostname(&controller.endpoint_id()).unwrap(),
        own
    );

    let secret = SecretKey::generate();
    let peer_key = SecretKey::generate();
    let peer_id = *peer_key.public().as_bytes();
    let mut current_trust = crate::trust::Trust::default();
    current_trust
        .pair(crate::trust::Device {
            endpoint_id: peer_id,
            display_id: peer_key.public().to_string(),
            relays: vec![],
            direct: vec![],
        })
        .unwrap();
    let record_trust = current_trust;
    // The encoder writes format version 1, which round-trips names.
    let bytes = installation::encode(
        &secret.to_bytes(),
        &RelaySettings::public_n0(),
        &record_trust,
        &crate::invitation::ReplayCache::new(),
        &crate::terminal::TerminalPolicy::default(),
        &crate::job::FileRetentionPolicy::default(),
        &crate::power::PowerPolicy::default(),
        &crate::job::FileResourcePolicy::default(),
        &crate::system::SystemPolicy::default(),
    )
    .unwrap();
    assert_eq!(bytes[4], 1);
    let again = installation::decode(&bytes).unwrap();
    assert!(again.trust.get(&peer_id).is_some());
    // Any other format version is refused, even with a valid checksum.
    let mut other = bytes[..bytes.len() - 4].to_vec();
    other[4] = 4;
    let checksum = installation::checksum(&other);
    other.extend_from_slice(&checksum.to_be_bytes());
    assert!(matches!(
        installation::decode(&other),
        Err(crate::error::Error::CorruptRecord)
    ));
}
