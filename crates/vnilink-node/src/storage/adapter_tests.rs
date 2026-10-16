//! Core behavior through the encrypted store, not only [`vnilink::MemoryStore`].
//! Fixtures use a private directory and an in-memory key. They do not read the
//! production keychain entry or the production database.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vnilink_app::{
    CredentialStore, EventPoller, FfiError, FfiEvent, FfiRelayInput, FfiSharingPolicyUpdate,
    FfiVideoCodecCaps, VnilinkCore,
};

use super::key::{KeyVault, MemoryVault};
#[cfg(target_os = "linux")]
use super::paths::linux_app_dir;
use super::{EncryptedInstallationStore, Failure};

fn unique() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

struct Fixture {
    root: PathBuf,
    engine: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("vnilink-adapter-{}", unique()));
        fs::create_dir(&root).unwrap();
        let engine = root.join("engine");
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&engine).unwrap();
        }
        #[cfg(not(unix))]
        fs::create_dir(&engine).unwrap();
        Self { root, engine }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct SharedStore(Arc<EncryptedInstallationStore>);

impl CredentialStore for SharedStore {
    fn load(&self) -> Result<Option<Vec<u8>>, FfiError> {
        self.0.load()
    }

    fn save(&self, record: Vec<u8>) -> Result<(), FfiError> {
        self.0.save(record)
    }
}

fn memory() -> Arc<MemoryVault> {
    Arc::new(MemoryVault::new())
}

fn open_store(engine: &Path, vault: &Arc<MemoryVault>) -> Arc<EncryptedInstallationStore> {
    Arc::new(
        EncryptedInstallationStore::open_fixture(engine, vault.clone() as Arc<dyn KeyVault>)
            .expect("fixture store"),
    )
}

fn open_core(store: &Arc<EncryptedInstallationStore>) -> VnilinkCore {
    let core = VnilinkCore::open_persistent_direct(Box::new(SharedStore(store.clone())))
        .expect("persistent core");
    core.set_video_codecs(FfiVideoCodecCaps {
        encode_baseline: true,
        encode_high: true,
        decode_baseline: true,
        decode_high: true,
        max_width: 4096,
        max_height: 4096,
        device: "test".into(),
        ..FfiVideoCodecCaps::default()
    })
    .expect("codecs");
    core
}

fn controller() -> VnilinkCore {
    let core = VnilinkCore::create_loopback_for_test().expect("controller");
    core.set_video_codecs(FfiVideoCodecCaps {
        encode_baseline: true,
        encode_high: true,
        decode_baseline: true,
        decode_high: true,
        max_width: 4096,
        max_height: 4096,
        device: "test".into(),
        ..FfiVideoCodecCaps::default()
    })
    .expect("codecs");
    core
}

fn drain(events: &EventPoller) {
    while !events.poll(64).is_empty() {}
}

fn saw(events: &EventPoller, kind: fn(&FfiEvent) -> bool) -> bool {
    events.poll(64).into_iter().any(|event| kind(&event))
}

fn assert_fenced(core: &VnilinkCore, store: &EncryptedInstallationStore, database: &Path) {
    let unchanged = fs::read(database).unwrap();
    assert_eq!(store.load().unwrap_err(), FfiError::StoreRecoveryRequired);
    assert_eq!(
        store.save(b"no-further-write".to_vec()).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert_eq!(fs::read(database).unwrap(), unchanged);
    assert_eq!(core.snapshot().unwrap_err(), FfiError::ShutDown);
    assert_eq!(
        core.set_desktop_sharing(true, false, false, false)
            .unwrap_err(),
        FfiError::ShutDown
    );
    assert_eq!(core.start_hosting().unwrap_err(), FfiError::ShutDown);
    core.shutdown();
    assert_eq!(core.endpoint_close_count(), 1);
}

fn production_database() -> PathBuf {
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_DATA_HOME");
        return linux_app_dir(home.as_deref(), xdg.as_deref())
            .unwrap_or_else(|_| PathBuf::from("/missing-home"))
            .join("engine")
            .join("installation.sqlite3");
    }
    #[cfg(not(target_os = "linux"))]
    {
        dirs::data_local_dir()
            .unwrap_or_default()
            .join("VniLink")
            .join("engine")
            .join("installation.sqlite3")
    }
}

fn file_stamp(path: &Path) -> Option<(u64, Option<SystemTime>)> {
    let meta = fs::symlink_metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

#[test]
fn consent_revocation_and_reopen_use_the_real_adapter() {
    let old_fixture = Fixture::new();
    let old_vault = memory();
    let old_store = open_store(&old_fixture.engine, &old_vault);
    let old_core = open_core(&old_store);
    old_core.start_hosting().unwrap();
    let events = old_core.subscribe_events().unwrap();
    drain(&events);
    old_store.arm_failure(Failure::Commit);
    assert_eq!(
        old_core
            .set_desktop_sharing(true, true, true, true)
            .unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert!(!saw(&events, |event| {
        matches!(event, FfiEvent::DeviceAuthorized { .. })
    }));
    assert_fenced(
        &old_core,
        &old_store,
        &old_fixture.engine.join("installation.sqlite3"),
    );
    drop(old_core);
    drop(old_store);
    let old_store = open_store(&old_fixture.engine, &old_vault);
    let old_core = open_core(&old_store);
    let policy = old_core.sharing_policy().unwrap();
    assert!(!policy.view);
    assert!(!policy.control);
    assert_eq!(old_core.endpoint_close_count(), 0);

    let new_fixture = Fixture::new();
    let new_vault = memory();
    let new_store = open_store(&new_fixture.engine, &new_vault);
    let new_core = open_core(&new_store);
    let id = new_core.endpoint_id();
    new_core
        .set_sharing_policy(FfiSharingPolicyUpdate {
            view: Some(true),
            terminal: Some(true),
            ..FfiSharingPolicyUpdate::default()
        })
        .unwrap();
    drop(new_core);
    drop(new_store);
    let new_store = open_store(&new_fixture.engine, &new_vault);
    let new_core = open_core(&new_store);
    assert_eq!(new_core.endpoint_id(), id);
    let policy = new_core.sharing_policy().unwrap();
    assert!(policy.view);
    assert!(policy.terminal);
    assert!(!policy.control);
}

#[test]
fn an_uncertain_pairing_save_fences_and_reopen_has_no_peer() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    let peer = controller();
    let invitation = peer.create_invitation(60).unwrap();
    let events = host.subscribe_events().unwrap();
    drain(&events);
    store.arm_failure(Failure::Commit);
    assert_eq!(
        host.accept_invitation(invitation).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert!(!saw(&events, |event| {
        matches!(event, FfiEvent::DevicePaired { .. })
    }));
    assert_fenced(&host, &store, &fixture.engine.join("installation.sqlite3"));
    drop(host);
    drop(store);
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    assert!(host.snapshot().unwrap().devices.is_empty());
}

#[test]
fn an_uncertain_forget_keeps_the_paired_record() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    let peer = controller();
    let invitation = peer.create_invitation(60).unwrap();
    let paired = host.accept_invitation(invitation).unwrap();
    let events = host.subscribe_events().unwrap();
    drain(&events);
    store.arm_failure(Failure::Commit);
    assert_eq!(
        host.revoke(paired.endpoint_id).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert!(!saw(&events, |event| {
        matches!(
            event,
            FfiEvent::DeviceUnpaired { .. } | FfiEvent::DeviceRevoked { .. }
        )
    }));
    assert_fenced(&host, &store, &fixture.engine.join("installation.sqlite3"));
    drop(host);
    drop(store);
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    assert_eq!(host.snapshot().unwrap().devices.len(), 1);
    assert_eq!(
        host.snapshot().unwrap().devices[0].endpoint_id,
        peer.endpoint_id()
    );
}

#[test]
fn an_uncertain_reset_keeps_the_bound_identity_and_reopen_uses_the_old_record() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    host.save_custom_relays(vec![FfiRelayInput {
        url: "https://relay.example.com./".into(),
        authenticated: true,
        replacement_token: "relay-credential-alpha".into(),
    }])
    .unwrap();
    let id = host.endpoint_id();
    store.arm_failure(Failure::Commit);
    assert_eq!(
        host.reset_identity().unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert_eq!(host.endpoint_id(), id);
    assert_fenced(&host, &store, &fixture.engine.join("installation.sqlite3"));
    drop(host);
    drop(store);
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    assert_eq!(host.endpoint_id(), id);
    let relays = host.saved_relays().unwrap();
    assert_eq!(relays.len(), 1);
    assert!(relays[0].authenticated);
    assert_eq!(relays[0].url, "https://relay.example.com./");
}

#[test]
fn a_durable_reset_changes_identity_and_keeps_the_relay() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    let peer = controller();
    host.accept_invitation(peer.create_invitation(60).unwrap())
        .unwrap();
    host.set_desktop_sharing(true, true, false, false).unwrap();
    host.save_custom_relays(vec![FfiRelayInput {
        url: "https://relay.example.com./".into(),
        authenticated: true,
        replacement_token: "relay-credential-alpha".into(),
    }])
    .unwrap();
    let old_id = host.endpoint_id();
    host.reset_identity().unwrap();
    assert_eq!(host.snapshot().unwrap_err(), FfiError::ShutDown);
    assert_eq!(host.endpoint_id(), old_id);
    assert_eq!(host.endpoint_close_count(), 1);
    drop(host);
    drop(store);
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    assert_ne!(host.endpoint_id(), old_id);
    assert!(host.snapshot().unwrap().devices.is_empty());
    assert!(!host.sharing_policy().unwrap().view);
    let relays = host.saved_relays().unwrap();
    assert!(relays[0].authenticated);
    assert!(!host.snapshot().unwrap().public_relay);
}

#[test]
fn a_known_rollback_keeps_the_old_policy_and_does_not_fence() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    store.arm_failure(Failure::EndedStatement);
    assert_eq!(
        host.set_desktop_sharing(true, true, true, true)
            .unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert!(!host.sharing_policy().unwrap().view);
    host.start_hosting().unwrap();
    assert_eq!(host.endpoint_close_count(), 0);
    host.set_desktop_sharing(true, false, false, false).unwrap();
    assert!(host.sharing_policy().unwrap().view);
    assert!(!host.sharing_policy().unwrap().control);
    host.shutdown();
    assert_eq!(host.endpoint_close_count(), 1);
}

#[test]
fn pairing_relay_policy_and_identity_survive_reopen() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    let peer = controller();
    let paired = host
        .accept_invitation(peer.create_invitation(60).unwrap())
        .unwrap();
    host.set_sharing_policy(FfiSharingPolicyUpdate {
        view: Some(true),
        clipboard: Some(true),
        terminal: Some(true),
        ..FfiSharingPolicyUpdate::default()
    })
    .unwrap();
    host.save_custom_relays(vec![FfiRelayInput {
        url: "https://relay.example.com./".into(),
        authenticated: true,
        replacement_token: "relay-credential-alpha".into(),
    }])
    .unwrap();
    let id = host.endpoint_id();
    drop(host);
    drop(store);
    let store = open_store(&fixture.engine, &vault);
    let host = open_core(&store);
    assert_eq!(host.endpoint_id(), id);
    let snapshot = host.snapshot().unwrap();
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.devices[0].endpoint_id, paired.endpoint_id);
    assert!(snapshot.devices[0].grants.view);
    assert!(snapshot.devices[0].grants.clipboard);
    let policy = host.sharing_policy().unwrap();
    assert!(policy.view);
    assert!(policy.clipboard);
    assert!(policy.terminal);
    let relays = host.saved_relays().unwrap();
    assert_eq!(relays[0].url, "https://relay.example.com./");
    assert!(relays[0].authenticated);
    assert_eq!(vault.stored_len(), Some(32));
}

#[test]
fn a_temporary_engine_does_not_touch_the_production_store_or_the_fixture() {
    let fixture = Fixture::new();
    let production = production_database();
    let parent = production.parent().unwrap().to_path_buf();
    let before = file_stamp(&production);
    let parent_existed = parent.exists();
    let temporary = VnilinkCore::create(true, Vec::new());
    if let Ok(core) = temporary {
        assert!(!core.endpoint_id().is_empty());
        core.shutdown();
    }
    assert_eq!(file_stamp(&production), before);
    if !parent_existed {
        assert!(!parent.exists());
    }
    assert!(!fixture.engine.join("installation.sqlite3").exists());
    assert!(!fixture.engine.join("installation.lock").exists());
}
