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
fn locked_and_unavailable_stores_fail_without_writing() {
    assert_eq!(
        Error::StoreLocked.to_string(),
        "the credential store is locked"
    );
    assert_eq!(
        Error::StoreUnavailable.to_string(),
        "the installation store is unavailable"
    );
    assert_eq!(
        Error::StoreRecoveryRequired.to_string(),
        "the installation store must be reopened and VniLink restarted"
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

fn assert_fenced(host: &Core, store: &MemoryStore, before: &[u8], attempts_after_failure: usize) {
    assert_eq!(store.record().as_deref(), Some(before));
    assert_eq!(store.save_attempts(), attempts_after_failure);
    assert!(matches!(host.snapshot(), Err(Error::ShutDown)));
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::ShutDown)
    ));
    assert_eq!(store.save_attempts(), attempts_after_failure);
    host.shutdown();
    assert_eq!(host.endpoint_close_count(), 1);
}

#[test]
fn the_first_uncertain_save_does_not_keep_an_identity() {
    let store = MemoryStore::empty();
    store.fail_next_save(Error::StoreRecoveryRequired);
    assert!(matches!(
        Core::open_direct(Arc::new(store.clone()), RelaySettings::public_n0()),
        Err(Error::StoreRecoveryRequired)
    ));
    assert!(store.record().is_none());
    assert_eq!(store.save_attempts(), 1);
}

#[test]
fn an_unavailable_save_does_not_fence_the_engine() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreUnavailable);
    let events = host.subscribe().unwrap();
    drain(&events);
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::StoreUnavailable)
    ));
    assert_eq!(store.record().as_deref(), Some(before.as_slice()));
    assert_eq!(store.save_attempts(), attempts + 1);
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceAuthorized { .. })
    }));
    host.set_desktop_sharing(all_grants()).unwrap();
    assert_eq!(store.save_attempts(), attempts + 2);
    assert_eq!(host.sharing_policy().unwrap().grants(), all_grants());
    host.shutdown();
    assert_eq!(host.endpoint_close_count(), 1);
}

#[test]
fn an_uncertain_consent_save_fences_without_a_second_write() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreRecoveryRequired);
    let events = host.subscribe().unwrap();
    drain(&events);
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::StoreRecoveryRequired)
    ));
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DeviceAuthorized { .. })
    }));
    assert_fenced(&host, &store, &before, attempts + 1);
}

#[test]
fn an_uncertain_pairing_save_fences_without_remembering_the_peer() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreRecoveryRequired);
    let events = host.subscribe().unwrap();
    drain(&events);
    assert!(matches!(
        host.accept_invitation(&invitation),
        Err(Error::StoreRecoveryRequired)
    ));
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DevicePaired { .. })
    }));
    assert_fenced(&host, &store, &before, attempts + 1);
}

#[test]
fn an_uncertain_forget_save_fences_and_keeps_the_paired_record() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let invitation = controller
        .create_invitation(Duration::from_secs(60))
        .unwrap();
    host.accept_invitation(&invitation).unwrap();
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreRecoveryRequired);
    let events = host.subscribe().unwrap();
    drain(&events);
    assert!(matches!(
        host.revoke(&controller.endpoint_id()),
        Err(Error::StoreRecoveryRequired)
    ));
    assert!(!saw(&events, |event| {
        matches!(
            event,
            CoreEvent::DeviceUnpaired { .. } | CoreEvent::DeviceRevoked { .. }
        )
    }));
    assert_fenced(&host, &store, &before, attempts + 1);
}

#[test]
fn an_uncertain_reset_keeps_the_saved_record_and_the_bound_secret() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    let secret = host.export_identity();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreRecoveryRequired);
    assert!(matches!(
        host.reset_identity(),
        Err(Error::StoreRecoveryRequired)
    ));
    assert_eq!(host.export_identity(), secret);
    assert_fenced(&host, &store, &before, attempts + 1);
    let reopened = open_loop(&store, RelaySettings::public_n0());
    assert_eq!(reopened.export_identity(), secret);
}

#[test]
fn an_uncertain_control_approval_fences_before_the_peer_is_saved() {
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let controller = peer();
    let file = host
        .export_pairing_file(Duration::from_secs(60), true)
        .unwrap();
    let preview = controller.open_pairing_file(&file.bytes).unwrap();
    controller.confirm_pairing(&preview, &file.check).unwrap();
    host.start_hosting().unwrap();
    let pending = controller
        .begin_connect_services(&host.endpoint_id())
        .unwrap();
    let request = wait_control_request(&host);
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.fail_next_save(Error::StoreRecoveryRequired);
    let events = host.subscribe().unwrap();
    drain(&events);
    assert!(matches!(
        host.confirm_control_request(&request.check),
        Err(Error::StoreRecoveryRequired)
    ));
    assert!(!saw(&events, |event| {
        matches!(event, CoreEvent::DevicePaired { .. })
    }));
    assert_fenced(&host, &store, &before, attempts + 1);
    let _ = pending;
    let _ = controller;
}

#[test]
fn a_panicking_save_fences_the_core_and_leaves_terminal_consent_unsaved() {
    use crate::ServiceKind;
    let store = MemoryStore::empty();
    let host = open_loop(&store, RelaySettings::public_n0());
    let before = store.record().unwrap();
    let attempts = store.save_attempts();
    store.panic_next_save();
    let saved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        host.set_service_sharing(ServiceKind::Terminal, true)
    }));
    let error = saved.expect("the save panic stays inside the core");
    assert!(matches!(error, Err(Error::StoreRecoveryRequired)));
    assert_eq!(store.record().as_deref(), Some(before.as_slice()));
    assert_eq!(store.save_attempts(), attempts + 1);
    assert!(matches!(host.start_hosting(), Err(Error::ShutDown)));
    assert!(matches!(
        host.set_desktop_sharing(all_grants()),
        Err(Error::ShutDown)
    ));
    assert!(matches!(host.sharing_policy(), Err(Error::ShutDown)));
    host.shutdown();
    assert_eq!(host.endpoint_close_count(), 1);
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

fn wait_control_request(host: &Core) -> crate::ControlRequest {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(request) = host.control_request().unwrap() {
            return request;
        }
        if std::time::Instant::now() > deadline {
            panic!("the host did not report a control request");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
