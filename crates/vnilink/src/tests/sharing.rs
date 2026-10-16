use std::sync::Arc;
use std::time::Duration;

use crate::installation::MemoryStore;
use crate::tests::support::start_direct;
use crate::{Core, Error, RelaySettings, ServiceKind, SharingPolicy, SharingPolicyUpdate};

#[test]
fn global_consent_is_durable_covers_future_peers_and_failed_save_changes_nothing() {
    let store = MemoryStore::empty();
    let host = Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()).unwrap();
    assert_eq!(host.sharing_policy().unwrap(), SharingPolicy::default());
    host.change_sharing_policy(SharingPolicyUpdate {
        terminal: Some(true),
        files: Some(true),
        ..Default::default()
    })
    .unwrap();
    let a = start_direct();
    let b = start_direct();
    for peer in [&a, &b] {
        host.accept_invitation(&peer.create_invitation(Duration::from_secs(60)).unwrap())
            .unwrap();
        assert!(
            host.service_permissions(&peer.endpoint_id())
                .unwrap()
                .terminal
        );
        assert!(host.service_permissions(&peer.endpoint_id()).unwrap().files);
    }
    let saved = store.record();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        host.set_service_sharing(ServiceKind::Terminal, false),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(store.record(), saved);
    for peer in [&a, &b] {
        assert!(
            host.service_permissions(&peer.endpoint_id())
                .unwrap()
                .terminal
        );
    }
    host.set_service_sharing(ServiceKind::Terminal, false)
        .unwrap();
    for peer in [&a, &b] {
        let permissions = host.service_permissions(&peer.endpoint_id()).unwrap();
        assert!(!permissions.terminal);
        assert!(permissions.files);
    }
    host.shutdown();
    drop(host);
    let reopened = Core::open_direct(Arc::new(store), RelaySettings::public_n0()).unwrap();
    assert!(!reopened.sharing_policy().unwrap().terminal);
    assert!(reopened.sharing_policy().unwrap().files);
    for device in reopened.snapshot().unwrap().devices {
        assert_eq!(device.os, Some(crate::OperatingSystem::local()));
        assert_eq!(device.presence, crate::DevicePresence::Disconnected);
    }
}

#[test]
fn concurrent_partial_updates_preserve_unrelated_global_switches() {
    let host = start_direct();
    std::thread::scope(|scope| {
        let terminal = scope.spawn(|| {
            host.change_sharing_policy(SharingPolicyUpdate {
                terminal: Some(true),
                ..Default::default()
            })
        });
        let camera = scope.spawn(|| {
            host.change_sharing_policy(SharingPolicyUpdate {
                camera: Some(true),
                ..Default::default()
            })
        });
        terminal.join().unwrap().unwrap();
        camera.join().unwrap().unwrap();
    });
    assert_eq!(
        host.sharing_policy().unwrap(),
        SharingPolicy {
            terminal: true,
            camera: true,
            ..Default::default()
        }
    );
}
