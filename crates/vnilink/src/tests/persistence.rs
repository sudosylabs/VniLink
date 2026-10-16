use std::sync::Arc;
use std::time::{Duration, SystemTime};

use iroh::RelayMode;

use crate::core::{Core, CoreConfig, Listen, OpenConfig};
use crate::error::Error;
use crate::events::{CoreEvent, EventSubscription};
use crate::installation::{self, MemoryStore};
use crate::invitation::{self};
use crate::relay::{CustomRelay, RelaySettings};
use crate::session::{Grants, SessionPhase};

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

fn drain(events: &EventSubscription) {
    while events.try_recv().is_some() {}
}

fn saw(events: &EventSubscription, kind: fn(&CoreEvent) -> bool) -> bool {
    let mut found = false;
    while let Some(event) = events.try_recv() {
        if kind(&event) {
            found = true;
        }
    }
    found
}

#[test]
fn restart_keeps_the_endpoint_grants_and_addresses() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    host.start_hosting().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    host.set_desktop_sharing(all_grants()).unwrap();
    let id = host.endpoint_id();
    let secret = host.export_identity();
    let devices = host.snapshot().unwrap().devices;
    assert!(host.snapshot().unwrap().hosting);
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].grants, all_grants());
    assert!(!devices[0].direct.is_empty());
    drop(host);

    let host = open_loop(&store, credentialed_relay());
    assert_eq!(host.endpoint_id(), id);
    assert!(
        host.export_identity() == secret,
        "restart changed the installation secret"
    );
    let snapshot = host.snapshot().unwrap();
    assert!(!snapshot.hosting);
    assert!(snapshot.incoming.is_none());
    assert!(snapshot.outgoing.is_none());
    assert!(snapshot.public_relay);
    assert_eq!(snapshot.devices, devices);
    assert_eq!(format!("{store:?}"), "MemoryStore");
}

#[test]
fn a_used_invitation_cannot_be_replayed_after_restart() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    drop(host);
    let host = open_loop(&store, RelaySettings::public_n0());
    assert!(matches!(
        host.accept_invitation(&invitation),
        Err(Error::InvitationReplayed)
    ));
}

#[test]
fn reaccepting_an_invitation_keeps_saved_grants() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let first = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&first).unwrap();
    host.set_desktop_sharing(all_grants()).unwrap();
    let second = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    let paired = host.accept_invitation(&second).unwrap();
    assert_eq!(paired.grants, all_grants());
    drop(host);

    let host = open_loop(&store, RelaySettings::public_n0());
    let device = &host.snapshot().unwrap().devices[0];
    assert_eq!(device.endpoint_id, controller.endpoint_id());
    assert_eq!(device.grants, all_grants());
}

#[test]
fn locked_and_unavailable_stores_fail_without_writing() {
    assert_eq!(
        Error::StoreLocked.to_string(),
        "the credential store is locked"
    );
    assert_eq!(
        Error::StoreUnavailable.to_string(),
        "the credential store is unavailable"
    );
    assert_eq!(
        Error::CorruptRecord.to_string(),
        "the installation record is not valid"
    );

    let locked = MemoryStore::locked();
    assert!(matches!(
        Core::open_direct(Arc::new(locked.clone()), RelaySettings::public_n0()),
        Err(Error::StoreLocked)
    ));
    assert!(locked.record().is_none());

    let unavailable = MemoryStore::unavailable();
    assert!(matches!(
        Core::open_direct(Arc::new(unavailable.clone()), RelaySettings::public_n0()),
        Err(Error::StoreUnavailable)
    ));
    assert!(unavailable.record().is_none());
}

#[test]
fn a_corrupt_record_is_left_unchanged() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let id = host.endpoint_id();
    drop(host);
    let good = store.record().unwrap();
    let mut corrupt = good.clone();
    corrupt[10] ^= 0x5a;
    let broken = MemoryStore::containing(corrupt.clone());
    assert!(matches!(
        Core::open_direct(Arc::new(broken.clone()), RelaySettings::public_n0()),
        Err(Error::CorruptRecord)
    ));
    assert_eq!(broken.record().as_deref(), Some(corrupt.as_slice()));

    let host = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(host.endpoint_id(), id);
    assert_eq!(store.record().as_deref(), Some(good.as_slice()));
}

#[test]
fn the_first_save_failure_does_not_keep_an_identity() {
    let store = MemoryStore::empty();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()),
        Err(Error::StoreUnavailable)
    ));
    assert!(store.record().is_none());

    let host = open_loop(&store, RelaySettings::public_n0());
    let id = host.endpoint_id();
    drop(host);
    assert_eq!(
        open_loop(&store, RelaySettings::public_n0()).endpoint_id(),
        id
    );
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
fn a_failed_save_keeps_the_previous_record() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_relay(credentialed_relay()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(store.record().as_deref(), Some(before.as_slice()));
    assert!(host.snapshot().unwrap().public_relay);

    let controller = peer();
    host.start_hosting().unwrap();
    let events = host.subscribe().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.accept_invitation(&invitation),
        Err(Error::StoreLocked)
    ));
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DevicePaired { .. })
    }));
    host.accept_invitation(&invitation).unwrap();

    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(
        host.snapshot().unwrap().devices[0].grants,
        Grants::default()
    );
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceAuthorized { .. })
    }));
    host.set_desktop_sharing(all_grants()).unwrap();

    let again = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        host.accept_invitation(&again),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(host.snapshot().unwrap().devices[0].grants, all_grants());
    host.accept_invitation(&again).unwrap();
    assert_eq!(host.snapshot().unwrap().devices[0].grants, all_grants());
    host.set_desktop_sharing(Grants {
        view: true,
        control: true,
        clipboard: false,
        transfer: false,
    })
    .unwrap();

    let host_invitation = host.create_invitation(Duration::from_secs(60)).unwrap();
    controller.accept_invitation(&host_invitation).unwrap();
    let session = controller.connect(&host.endpoint_id()).unwrap();
    assert_eq!(session.phase, SessionPhase::Active);
    let generation = host.snapshot().unwrap().incoming.unwrap().generation;
    assert_eq!(
        host.snapshot().unwrap().incoming.unwrap().phase,
        SessionPhase::Active
    );
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.revoke(&controller.endpoint_id()),
        Err(Error::StoreLocked)
    ));
    let incoming = host.snapshot().unwrap().incoming.unwrap();
    assert_eq!(incoming.phase, SessionPhase::Active);
    assert_eq!(incoming.generation, generation);
    assert!(host.snapshot().unwrap().devices[0].grants.control);
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceRevoked { .. })
    }));

    host.revoke(&controller.endpoint_id()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let phase = host
            .snapshot()
            .unwrap()
            .incoming
            .as_ref()
            .map(|session| session.phase.clone());
        if phase.as_ref() == Some(&SessionPhase::Closed) {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("revoke did not stop the session: {phase:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_core_without_a_store_does_not_save() {
    let core = peer();
    let id = core.endpoint_id();
    assert!(matches!(
        core.set_relay(RelaySettings::public_n0()),
        Err(Error::InvalidSetting(_))
    ));
    assert!(matches!(
        core.reset_identity(),
        Err(Error::InvalidSetting(_))
    ));
    assert_eq!(core.endpoint_id(), id);
    assert!(core.snapshot().is_ok());
    assert!(core.snapshot().unwrap().devices.is_empty());
}

#[test]
fn unpair_removal_and_rollback_persist() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let events = host.subscribe().unwrap();
    host.start_hosting().unwrap();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);

    host.set_peer_pet_name(&controller.endpoint_id(), Some("Office computer"))
        .unwrap();
    let before = host.snapshot().unwrap().devices;
    assert_eq!(before[0].os, Some(crate::OperatingSystem::local()));
    // A failed save restores the entire identity and metadata, and emits nothing.
    store.fail_next_save(Error::StoreLocked);
    drain(&events);
    assert!(matches!(
        host.unpair(&controller.endpoint_id()),
        Err(Error::StoreLocked)
    ));
    assert_eq!(host.snapshot().unwrap().devices, before);
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceUnpaired { .. })
    }));

    // Removal survives a reopen.
    host.unpair(&controller.endpoint_id()).unwrap();
    assert!(saw(&events, |event| {
        matches!(event, CoreEvent::DeviceUnpaired { .. })
    }));
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert!(reopened.snapshot().unwrap().devices.is_empty());
    assert!(matches!(
        reopened.unpair(&controller.endpoint_id()),
        Err(Error::NotPaired)
    ));
}

#[test]
fn service_permissions_are_independent_persisted_and_rollback_before_revocation() {
    use crate::{ServiceKind as Kind, ServicePermissions};
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    let id = controller.endpoint_id();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions::default()
    );
    host.set_service_sharing(Kind::Terminal, true).unwrap();
    host.set_service_sharing(Kind::Camera, true).unwrap();
    host.set_service_sharing(Kind::SystemAudio, true).unwrap();
    host.set_service_sharing(Kind::Microphone, true).unwrap();
    host.set_recording_sharing(true).unwrap();
    host.set_desktop_sharing(all_grants()).unwrap();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: true,
            terminal: true,
            camera: true,
            system_audio: true,
            microphone: true,
            recording: true,
            system_actions: false
        }
    );
    host.set_service_sharing(Kind::Files, false).unwrap();
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true,
            system_audio: true,
            microphone: true,
            recording: true,
            system_actions: false
        }
    );
    assert!(host.snapshot().unwrap().devices[0].grants.clipboard);
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_service_sharing(Kind::Terminal, false),
        Err(Error::StoreLocked)
    ));
    assert!(host.service_permissions(&id).unwrap().terminal);
    assert_eq!(store.record().unwrap(), saved);
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_service_sharing(Kind::SystemAudio, false),
        Err(Error::StoreLocked)
    ));
    assert!(host.service_permissions(&id).unwrap().system_audio);
    store.fail_next_save(Error::StoreLocked);
    assert!(matches!(
        host.set_recording_sharing(false),
        Err(Error::StoreLocked)
    ));
    assert!(host.service_permissions(&id).unwrap().recording);
    assert_eq!(store.record().unwrap(), saved);
    assert_eq!(store.record().unwrap(), saved);
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(host.revoke(&id), Err(Error::StoreUnavailable)));
    assert_eq!(
        host.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true,
            system_audio: true,
            microphone: true,
            recording: true,
            system_actions: false
        }
    );
    assert_eq!(store.record().unwrap(), saved);
    // Invitation refresh preserves the explicit grants, rather than treating
    // a peer metadata refresh as permission removal.
    host.accept_invitation(
        &controller
            .create_invitation(Duration::from_secs(60))
            .unwrap(),
    )
    .unwrap();
    assert!(host.service_permissions(&id).unwrap().camera);
    host.shutdown();
    drop(host);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(
        reopened.service_permissions(&id).unwrap(),
        ServicePermissions {
            desktop: true,
            files: false,
            terminal: true,
            camera: true,
            system_audio: true,
            microphone: true,
            recording: true,
            system_actions: false
        }
    );
    reopened.unpair(&id).unwrap();
    assert!(matches!(
        reopened.service_permissions(&id),
        Err(Error::NotPaired)
    ));
    reopened.set_service_sharing(Kind::Camera, true).unwrap();
    assert!(reopened.sharing_policy().unwrap().camera);
}

#[test]
fn terminal_policy_is_durable_and_save_failure_rolls_back() {
    use crate::terminal::TerminalPolicy;
    let store = MemoryStore::empty();
    let core = open_loop(&store, RelaySettings::public_n0());
    let policy = TerminalPolicy {
        allow_persistence: false,
        max_sessions: 3,
        max_sessions_per_peer: 1,
        history_bytes: 32 * 1024,
        replay_bytes: 1024,
        detached_timeout_secs: 17,
    };
    core.set_terminal_policy(policy).unwrap();
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        core.set_terminal_policy(TerminalPolicy::default()),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(core.terminal_policy().unwrap(), policy);
    assert_eq!(store.record().unwrap(), saved);
    core.shutdown();
    drop(core);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(reopened.terminal_policy().unwrap(), policy);
    let mut invalid = policy;
    invalid.replay_bytes = invalid.history_bytes + 1;
    assert!(matches!(
        reopened.set_terminal_policy(invalid),
        Err(Error::InvalidSetting(_))
    ));
    assert_eq!(reopened.terminal_policy().unwrap(), policy);
    assert_eq!(store.record().unwrap(), saved);
}

#[test]
fn file_retention_policy_is_durable_and_save_failure_rolls_back() {
    use crate::job::FileRetentionPolicy;
    let store = MemoryStore::empty();
    let core = open_loop(&store, RelaySettings::public_n0());
    let policy = FileRetentionPolicy {
        resume_cache_bytes: 12_345,
        resume_cache_age_secs: 456,
        recent_fetch_bytes: 789,
        recent_fetch_age_secs: 10,
    };
    core.set_file_retention_policy(policy).unwrap();
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        core.set_file_retention_policy(FileRetentionPolicy {
            resume_cache_bytes: 0,
            resume_cache_age_secs: 0,
            recent_fetch_bytes: 0,
            recent_fetch_age_secs: 0
        }),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(core.file_retention_policy().unwrap(), policy);
    assert_eq!(store.record().unwrap(), saved);
    let mut invalid = policy;
    invalid.resume_cache_bytes = u64::MAX;
    assert!(matches!(
        core.set_file_retention_policy(invalid),
        Err(Error::InvalidSetting(_))
    ));
    core.shutdown();
    drop(core);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(reopened.file_retention_policy().unwrap(), policy);
}

#[test]
fn power_policy_is_durable_and_save_failure_rolls_back() {
    use crate::power::PowerPolicy;
    let store = MemoryStore::empty();
    let core = open_loop(&store, RelaySettings::public_n0());
    let policy = PowerPolicy {
        incoming_display: true,
        outgoing_system: true,
        incoming_idle_secs: 73,
        outgoing_idle_secs: 94,
        ..Default::default()
    };
    core.set_power_policy(policy).unwrap();
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        core.set_power_policy(PowerPolicy::default()),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(core.power_policy().unwrap(), policy);
    assert_eq!(store.record().unwrap(), saved);
    core.shutdown();
    drop(core);
    assert_eq!(
        open_loop(&store, RelaySettings::public_n0())
            .power_policy()
            .unwrap(),
        policy
    );
    for bits in 0..=31 {
        assert_eq!(PowerPolicy::from_bits(bits).unwrap().bits(), bits);
    }
    for bits in 32..=255 {
        assert!(matches!(
            PowerPolicy::from_bits(bits),
            Err(Error::CorruptRecord)
        ));
    }
}

#[test]
fn file_resource_policy_roundtrips_and_save_failure_preserves_durable_state() {
    use crate::job::FileResourcePolicy;
    let store = MemoryStore::empty();
    let core = open_loop(&store, RelaySettings::public_n0());
    let policy = FileResourcePolicy {
        max_concurrent_receivers: 2,
        max_reserved_bytes: 123456,
        minimum_free_bytes: 7890,
    };
    core.set_file_resource_policy(policy).unwrap();
    let saved = store.record().unwrap();
    store.fail_next_save(Error::StoreUnavailable);
    assert!(matches!(
        core.set_file_resource_policy(Default::default()),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(core.file_resource_policy().unwrap(), policy);
    assert_eq!(store.record().unwrap(), saved);
    for invalid in [
        FileResourcePolicy {
            max_concurrent_receivers: 0,
            ..policy
        },
        FileResourcePolicy {
            max_concurrent_receivers: 17,
            ..policy
        },
        FileResourcePolicy {
            max_reserved_bytes: 0,
            ..policy
        },
        FileResourcePolicy {
            minimum_free_bytes: u64::MAX,
            ..policy
        },
    ] {
        assert!(core.set_file_resource_policy(invalid).is_err());
        assert_eq!(core.file_resource_policy().unwrap(), policy);
        assert_eq!(store.record().unwrap(), saved);
    }
    core.shutdown();
    drop(core);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(reopened.file_resource_policy().unwrap(), policy);
}

#[test]
fn relay_restart_required_includes_credentials_and_clears_when_reverted() {
    let store = MemoryStore::empty();
    let original = credentialed_relay();
    let core = open_loop(&store, original.clone());
    assert!(!core.relay_restart_required().unwrap());
    let changed = CustomRelay::new("https://relay.example.com./")
        .unwrap()
        .with_auth_token("relay-credential-beta")
        .unwrap();
    core.set_relay(RelaySettings::custom(vec![changed]).unwrap())
        .unwrap();
    assert!(core.relay_restart_required().unwrap());
    core.set_relay(original).unwrap();
    assert!(!core.relay_restart_required().unwrap());
}
