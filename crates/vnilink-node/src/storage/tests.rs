// ---------------------------------------------------------------------------------------------
// Copyright (c) 2026 Sudosy Labs. All rights reserved.
// Licensed under the GNU Affero General Public License, version 3 only.
// See LICENSE in the project root for license information.
// SPDX-License-Identifier: AGPL-3.0-only
// ---------------------------------------------------------------------------------------------

//! Behavioral coverage for the encrypted installation store.
//! Fixture directories and key files stay outside the production installation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vnilink_app::{CredentialStore, FfiError};

use super::database::{
    arm_schema_fault, bundled_sqlite_version, expected_vfs_name, SchemaFault, CREATE_SQL,
};
use super::key::{FileVault, KeyVault, KeyringVault, MemoryVault, KEY_ACCOUNT, KEY_SERVICE};
#[cfg(unix)]
use super::paths::linux_app_dir;
use super::paths::{
    self, linux_fstype_is_remote, linux_mount_fstype, linux_path_is_remote, resolve_mount_path,
};
use super::{recover_installation, EncryptedInstallationStore, Failure};

const OLD_SENTINEL: &[u8] = b"VNILINK-PLAINTEXT-SENTINEL-OLD-9f3c1a";
const NEW_SENTINEL: &[u8] = b"VNILINK-PLAINTEXT-SENTINEL-NEW-9f3c1b";

fn unique() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}-{n}-{nanos}", std::process::id())
}

struct Fixture {
    root: PathBuf,
    engine: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("vnilink-store-{}", unique()));
        fs::create_dir(&root).unwrap();
        let engine = root.join("engine");
        create_private_dir(&engine);
        Self { root, engine }
    }

    fn key_path(&self) -> PathBuf {
        self.root.join("key")
    }

    fn database(&self) -> PathBuf {
        self.engine.join("installation.sqlite3")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The engine directory the store expects: owner-only, as production creates
/// it. On Windows a plain directory inherits the temp folder's broader DACL,
/// which the store rightly refuses.
fn create_private_dir(path: &Path) {
    super::paths::ensure_private_dir(path).expect("private fixture directory");
}

fn memory() -> Arc<MemoryVault> {
    Arc::new(MemoryVault::new())
}

fn open(engine: &Path, vault: Arc<dyn KeyVault>) -> EncryptedInstallationStore {
    EncryptedInstallationStore::open_fixture(engine, vault).expect("fixture opens")
}

fn as_vault(vault: &Arc<MemoryVault>) -> Arc<dyn KeyVault> {
    vault.clone()
}

fn file_vault(path: PathBuf) -> Arc<FileVault> {
    Arc::new(FileVault::new(path))
}

fn as_file(vault: &Arc<FileVault>) -> Arc<dyn KeyVault> {
    vault.clone()
}

#[cfg(unix)]
fn mode_bits(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn nonce_of(database: &Path) -> Vec<u8> {
    let connection = rusqlite::Connection::open(database).unwrap();
    connection
        .query_row("SELECT nonce FROM installation WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn contains_bytes(path: &Path, needle: &[u8]) -> bool {
    match fs::read(path) {
        Ok(bytes) => bytes.windows(needle.len()).any(|window| window == needle),
        Err(_) => false,
    }
}

fn child_env_is(mode: &str) -> bool {
    std::env::var("VNILINK_STORE_CHILD").ok().as_deref() == Some(mode)
}

#[test]
fn production_key_names_are_not_the_legacy_record_entry() {
    assert_eq!(KEY_SERVICE, "com.vnilink.storage");
    assert_eq!(KEY_ACCOUNT, "installation-key");
    assert_ne!(KEY_SERVICE, "com.vnilink.installation");
    assert_ne!(KEY_ACCOUNT, "primary");
}

#[test]
fn bundled_sqlite_is_the_pinned_351_release() {
    assert_eq!(bundled_sqlite_version(), "3.51.3");
}

// Linux path rules over Unix paths; on Windows `/var/data` is not even absolute.
#[cfg(unix)]
#[test]
fn linux_paths_follow_absolute_xdg_then_home() {
    let home = Path::new("/home/someone");
    assert_eq!(
        linux_app_dir(Some(home), Some(std::ffi::OsStr::new("/var/data"))).unwrap(),
        PathBuf::from("/var/data/vnilink")
    );
    assert_eq!(
        linux_app_dir(Some(home), Some(std::ffi::OsStr::new(""))).unwrap(),
        PathBuf::from("/home/someone/.local/share/vnilink")
    );
    assert_eq!(
        linux_app_dir(Some(home), Some(std::ffi::OsStr::new("relative"))).unwrap(),
        PathBuf::from("/home/someone/.local/share/vnilink")
    );
    assert_eq!(
        linux_app_dir(None, Some(std::ffi::OsStr::new("/var/data"))).unwrap(),
        PathBuf::from("/var/data/vnilink")
    );
    assert!(linux_app_dir(None, Some(std::ffi::OsStr::new("relative"))).is_err());
    assert!(linux_app_dir(None, None).is_err());
}

#[test]
fn linux_mountinfo_uses_the_longest_match() {
    let info = "\
1 0 8:1 / / rw - ext4 /dev/sda1 rw
2 1 0:45 / /mnt/remote rw - nfs server:/export rw
3 2 8:2 / /mnt/remote/local rw - ext4 /dev/sdb1 rw
4 1 0:46 / /mnt/my\\040disk rw - cifs //server/share rw
5 1 0:47 / /home/\\303\\251 rw - nfs server:/home rw
";
    assert_eq!(
        linux_mount_fstype(info, Path::new("/mnt/remote/local/engine")).as_deref(),
        Some("ext4")
    );
    assert_eq!(
        linux_mount_fstype(info, Path::new("/mnt/remote/engine")).as_deref(),
        Some("nfs")
    );
    assert_eq!(
        linux_mount_fstype(info, Path::new("/mnt/my disk/engine")).as_deref(),
        Some("cifs")
    );
    assert_eq!(
        linux_mount_fstype(info, Path::new("/home")).as_deref(),
        Some("ext4")
    );
    assert_eq!(
        linux_mount_fstype(info, Path::new("/home/é/engine")).as_deref(),
        Some("nfs")
    );
    assert_eq!(
        linux_mount_fstype(info, Path::new("/home/someone/engine")).as_deref(),
        Some("ext4")
    );
    assert!(linux_mount_fstype("", Path::new("/home")).is_none());
    for fstype in ["nfs", "nfs4", "cifs", "smb", "smb2", "smb3", "smbfs"] {
        assert!(linux_fstype_is_remote(fstype), "{fstype}");
    }
    for fstype in ["ext4", "apfs", "zfs", "tmpfs"] {
        assert!(!linux_fstype_is_remote(fstype), "{fstype}");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_onto_nfs_is_remote_even_when_the_lexical_parent_is_local() {
    let root = std::env::temp_dir().join(format!("vnilink-mount-{}", unique()));
    fs::create_dir(&root).unwrap();
    let home = root.join("home");
    let remote = root.join("mnt").join("remote");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&remote).unwrap();
    std::os::unix::fs::symlink(&remote, home.join("data")).unwrap();
    let lexical = home.join("data").join("engine");
    let resolved_root = resolve_mount_path(&root).unwrap();
    let resolved_remote = resolve_mount_path(&remote).unwrap();
    let info = format!(
        "1 0 8:1 / {resolved_root} rw - ext4 /dev/sda1 rw\n\
         2 1 0:45 / {resolved_remote} rw - nfs server:/export rw\n\
         3 1 8:1 / {lexical_root} rw - ext4 /dev/sda1 rw\n",
        resolved_root = resolved_root.display(),
        resolved_remote = resolved_remote.display(),
        lexical_root = root.display(),
    );
    assert_eq!(
        linux_mount_fstype(&info, &lexical).as_deref(),
        Some("ext4"),
        "the lexical path still sits on the parent mount"
    );
    assert_eq!(
        resolve_mount_path(&lexical).unwrap(),
        resolved_remote.join("engine")
    );
    assert!(linux_path_is_remote(&info, &lexical).unwrap());
    std::os::unix::fs::symlink(&home, root.join("alias")).unwrap();
    let local = root.join("alias").join("engine");
    assert!(!linux_path_is_remote(&info, &local).unwrap());
    let loop_a = root.join("loop-a");
    let loop_b = root.join("loop-b");
    std::os::unix::fs::symlink(&loop_b, &loop_a).unwrap();
    std::os::unix::fs::symlink(&loop_a, &loop_b).unwrap();
    assert!(resolve_mount_path(&loop_a.join("engine")).is_err());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_fresh_directory_round_trips_and_keeps_only_the_key_in_the_vault() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap(), None);
    assert_eq!(vault.writes(), 1);
    assert_eq!(vault.stored_len(), Some(32));
    let record = b"installation-record-one".to_vec();
    store.save(record.clone()).unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some(record.as_slice()));
    assert_eq!(vault.stored_len(), Some(32));
    assert_eq!(vault.writes(), 1);
    drop(store);

    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(record.as_slice()));
    assert_eq!(vault.writes(), 1);
    let above_windows_credential_limit = vec![0x5a; 3000];
    store.save(above_windows_credential_limit.clone()).unwrap();
    assert_eq!(vault.stored_len(), Some(32));
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(above_windows_credential_limit.as_slice())
    );
}

#[test]
fn an_empty_schema_is_fresh_only_for_the_process_that_created_it() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap(), None);
    drop(store);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap_err(), FfiError::CorruptRecord);
    assert!(fixture.database().is_file());
}

#[test]
fn identical_plaintext_is_rewritten_with_a_new_nonce() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    let record = b"same-plaintext".to_vec();
    store.save(record.clone()).unwrap();
    let first = nonce_of(&fixture.database());
    store.save(record.clone()).unwrap();
    let second = nonce_of(&fixture.database());
    assert_ne!(first, second);
    assert_eq!(store.load().unwrap().as_deref(), Some(record.as_slice()));
    assert!(!contains_bytes(&fixture.database(), &record));
}

#[test]
fn plaintext_bounds_reject_an_empty_or_oversized_record_before_sql() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    let kept = b"kept-record".to_vec();
    store.save(kept.clone()).unwrap();
    let before = fs::read(&fixture.database()).unwrap();
    assert_eq!(
        store.save(Vec::new()).unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert_eq!(
        store.save(vec![7; 1_048_577]).unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert_eq!(fs::read(&fixture.database()).unwrap(), before);
    assert_eq!(store.load().unwrap().as_deref(), Some(kept.as_slice()));
    let max = vec![9; 1_048_576];
    store.save(max.clone()).unwrap();
    assert_eq!(store.load().unwrap().as_deref(), Some(max.as_slice()));
    assert_eq!(vault.stored_len(), Some(32));
}

#[test]
fn missing_halves_are_unavailable_and_are_left_in_place() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"present".to_vec()).unwrap();
    drop(store);

    let key = vault.read().unwrap().unwrap();
    vault.delete().unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert!(fixture.database().is_file());
    assert_eq!(vault.stored_len(), None);

    let mut restored = [0u8; 32];
    restored.copy_from_slice(&key);
    vault.write(&restored).unwrap();
    fs::remove_file(fixture.database()).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert!(!fixture.database().exists());
    assert_eq!(vault.stored_len(), Some(32));
    assert_eq!(vault.writes(), 2);
}

#[test]
fn an_uncertain_schema_commit_keeps_the_key_and_is_not_fresh_on_reopen() {
    let fixture = Fixture::new();
    let vault = memory();
    arm_schema_fault(SchemaFault::Commit);
    let err =
        EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault)).unwrap_err();
    assert_eq!(err, FfiError::StoreRecoveryRequired);
    assert_eq!(vault.writes(), 1);
    assert_eq!(vault.stored_len(), Some(32));
    assert!(fixture.database().is_file());
    let err =
        EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault)).unwrap_err();
    assert_ne!(err, FfiError::StoreLocked);
    assert_eq!(vault.writes(), 1);
    assert!(fixture.database().is_file());
}

#[test]
fn a_rolled_back_schema_statement_does_not_provision_again() {
    let fixture = Fixture::new();
    let vault = memory();
    arm_schema_fault(SchemaFault::Statement);
    let err =
        EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault)).unwrap_err();
    assert_eq!(err, FfiError::StoreUnavailable);
    assert_eq!(vault.writes(), 1);
    assert!(fixture.database().is_file());
    let err =
        EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault)).unwrap_err();
    assert_eq!(vault.writes(), 1);
    assert!(fixture.database().is_file());
    let _ = err;
}

#[test]
fn a_failed_schema_directory_sync_requires_restart_and_keeps_the_empty_schema() {
    let fixture = Fixture::new();
    let vault = memory();
    arm_schema_fault(SchemaFault::DirectorySync);
    let err =
        EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault)).unwrap_err();
    assert_eq!(err, FfiError::StoreRecoveryRequired);
    assert_eq!(vault.writes(), 1);
    assert!(fixture.database().is_file());
    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM installation", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
    drop(connection);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap_err(), FfiError::CorruptRecord);
    assert_eq!(
        store.save(b"do-not-adopt".to_vec()).unwrap_err(),
        FfiError::CorruptRecord
    );
    assert_eq!(vault.writes(), 1);
    assert!(fixture.database().is_file());
}

#[test]
fn provisioning_that_stops_after_the_key_does_not_adopt_a_new_database() {
    let fixture = Fixture::new();
    let vault = memory();
    let err =
        EncryptedInstallationStore::open_fixture_fail_after_key(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert!(!fixture.database().exists());
    assert_eq!(vault.writes(), 1);
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert_eq!(vault.writes(), 1);
    assert!(!fixture.database().exists());
}

#[test]
fn a_wrong_sized_key_or_failed_authentication_is_corrupt_and_preserved() {
    let fixture = Fixture::new();
    let vault = file_vault(fixture.key_path());
    let store = open(&fixture.engine, as_file(&vault));
    store.save(b"authentic".to_vec()).unwrap();
    drop(store);

    fs::write(fixture.key_path(), vec![4; 16]).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_file(&vault));
    assert_eq!(err.unwrap_err(), FfiError::CorruptRecord);
    assert_eq!(fs::read(fixture.key_path()).unwrap(), vec![4; 16]);
    assert!(fixture.database().is_file());
}

#[test]
fn tampering_with_the_row_fails_authentication_and_keeps_the_file() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"authentic-row".to_vec()).unwrap();
    drop(store);
    let before = fs::read(fixture.database()).unwrap();
    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    let mut ciphertext: Vec<u8> = connection
        .query_row(
            "SELECT ciphertext FROM installation WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 0xff;
    connection
        .execute(
            "UPDATE installation SET ciphertext = ?1 WHERE id = 1",
            [ciphertext],
        )
        .unwrap();
    drop(connection);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap_err(), FfiError::CorruptRecord);
    assert_ne!(fs::read(fixture.database()).unwrap(), before);
    drop(store);
    assert!(fixture.database().is_file());
}

#[test]
fn wal_shm_and_a_journal_without_a_database_are_corrupt() {
    let fixture = Fixture::new();
    let wal = fixture.engine.join("installation.sqlite3-wal");
    fs::write(&wal, b"wal").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let vault = memory();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::CorruptRecord);
    assert!(wal.is_file());
    assert_eq!(vault.writes(), 0);

    fs::remove_file(&wal).unwrap();
    let journal = fixture.engine.join("installation.sqlite3-journal");
    fs::write(&journal, b"journal").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&journal, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::CorruptRecord);
    assert!(journal.is_file());
    assert!(!fixture.database().exists());
}

#[test]
fn oversized_artifacts_are_rejected_without_truncation() {
    let fixture = Fixture::new();
    let database = fixture.database();
    fs::write(&database, b"x").unwrap();
    let file = fs::OpenOptions::new().write(true).open(&database).unwrap();
    file.set_len(8 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let vault = memory();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert_eq!(fs::metadata(&database).unwrap().len(), 8 * 1024 * 1024 + 1);

    fs::remove_file(&database).unwrap();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"small".to_vec()).unwrap();
    drop(store);
    let journal = fixture.engine.join("installation.sqlite3-journal");
    fs::write(&journal, b"j").unwrap();
    let file = fs::OpenOptions::new().write(true).open(&journal).unwrap();
    file.set_len(16 * 1024 * 1024 + 1).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert_eq!(fs::metadata(&journal).unwrap().len(), 16 * 1024 * 1024 + 1);
    assert!(fixture.database().is_file());
}

#[test]
fn the_open_database_uses_the_private_sqlite_settings() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"settings".to_vec()).unwrap();
    assert_eq!(store.pragma_text("journal_mode").unwrap(), "delete");
    assert_eq!(store.pragma_i64("synchronous").unwrap(), 2);
    assert_eq!(store.pragma_i64("temp_store").unwrap(), 2);
    assert_eq!(store.pragma_i64("page_size").unwrap(), 4096);
    assert_eq!(store.pragma_i64("max_page_count").unwrap(), 2048);
    assert_eq!(store.pragma_i64("application_id").unwrap(), 0x564E_4C49);
    #[cfg(target_os = "macos")]
    assert_eq!(store.pragma_i64("fullfsync").unwrap(), 1);
    assert_eq!(store.vfs_name().unwrap(), expected_vfs_name());
    assert_eq!(
        store
            .sqlite_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH)
            .unwrap(),
        1_114_112
    );
    assert_eq!(
        store
            .sqlite_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH)
            .unwrap(),
        16_384
    );
    assert_eq!(
        store
            .sqlite_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED)
            .unwrap(),
        0
    );
    drop(store);

    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    let mut statement = connection
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY name")
        .unwrap();
    let rows: Vec<(String, String, Option<String>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    let application: Vec<_> = rows
        .iter()
        .filter(|(_, name, _)| !name.starts_with("sqlite_"))
        .collect();
    assert_eq!(application.len(), 1);
    assert_eq!(application[0].0, "table");
    assert_eq!(application[0].1, "installation");
    assert_eq!(application[0].2.as_deref(), Some(CREATE_SQL));
}

#[test]
fn a_trigger_outside_sqlite_bookkeeping_is_corrupt_and_kept() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"kept".to_vec()).unwrap();
    drop(store);
    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER sqliteXdelete BEFORE UPDATE ON installation \
             BEGIN DELETE FROM installation; END",
        )
        .unwrap();
    drop(connection);
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::CorruptRecord);
    assert!(fixture.database().is_file());
    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM installation", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn dropping_the_store_keeps_the_database_and_the_lock() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"kept".to_vec()).unwrap();
    drop(store);
    let lock = fixture.engine.join("installation.lock");
    assert!(lock.is_file());
    assert!(fixture.database().is_file());
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(b"kept".as_slice()));
    drop(store);
    assert!(lock.is_file());
}

#[test]
fn a_second_open_in_this_process_is_unavailable_until_the_lease_drops() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"leased".to_vec()).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    let lock = fixture.engine.join("installation.lock");
    assert!(lock.is_file());
    drop(store);
    assert!(lock.is_file());
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(b"leased".as_slice()));
}

#[cfg(unix)]
#[test]
fn unsafe_permissions_links_and_acls_are_left_unchanged() {
    let fixture = Fixture::new();
    let loose = fixture.root.join("loose-engine");
    fs::DirBuilder::new().create(&loose).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
    let vault = memory();
    let err = EncryptedInstallationStore::open_fixture(&loose, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert_eq!(mode_bits(&loose), 0o755);
    assert_eq!(vault.writes(), 0);

    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"private".to_vec()).unwrap();
    drop(store);
    assert_eq!(mode_bits(&fixture.database()), 0o600);
    assert_eq!(mode_bits(&fixture.engine.join("installation.lock")), 0o600);
    fs::set_permissions(&fixture.database(), fs::Permissions::from_mode(0o644)).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert_eq!(mode_bits(&fixture.database()), 0o644);

    fs::set_permissions(&fixture.database(), fs::Permissions::from_mode(0o600)).unwrap();
    let linked = fixture.engine.join("installation-link");
    fs::hard_link(fixture.database(), &linked).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert!(linked.is_file());
    assert!(fixture.database().is_file());
    fs::remove_file(&linked).unwrap();

    fs::remove_file(fixture.database()).unwrap();
    std::os::unix::fs::symlink(fixture.root.join("missing-target"), fixture.database()).unwrap();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::CorruptRecord);
    assert!(fs::symlink_metadata(fixture.database())
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(target_os = "macos")]
#[test]
fn an_allow_acl_on_the_engine_directory_is_rejected_and_kept() {
    let fixture = Fixture::new();
    let status = Command::new("chmod")
        .args([
            "+a",
            "everyone allow read",
            fixture.engine.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let vault = memory();
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    let listed = Command::new("ls")
        .args(["-led", fixture.engine.to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8(listed.stdout).unwrap();
    assert!(text.contains("allow"), "{text}");
    assert_eq!(vault.writes(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn an_allow_acl_on_the_lease_is_rejected_and_kept() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"leased".to_vec()).unwrap();
    drop(store);
    let lock = fixture.engine.join("installation.lock");
    let status = Command::new("chmod")
        .args(["+a", "everyone allow read", lock.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(status.success());
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    let listed = Command::new("ls")
        .args(["-le", lock.to_str().unwrap()])
        .output()
        .unwrap();
    let text = String::from_utf8(listed.stdout).unwrap();
    assert!(text.contains("allow"), "{text}");
    assert!(fixture.database().is_file());
    assert_eq!(vault.writes(), 1);
}

#[cfg(target_os = "linux")]
#[test]
fn a_posix_acl_on_the_lease_is_rejected_when_the_filesystem_stores_it() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"leased".to_vec()).unwrap();
    drop(store);
    let lock = fixture.engine.join("installation.lock");
    let text = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
    // Version 2, then user-obj/group-obj/other. Named entries are absent, so
    // there is no mask. `ACL_UNDEFINED_ID` is all-bits-set.
    let mut value = Vec::with_capacity(28);
    value.extend_from_slice(&2u32.to_le_bytes());
    for (tag, perm) in [(1u16, 6u16), (4u16, 4u16), (0x20u16, 4u16)] {
        value.extend_from_slice(&tag.to_le_bytes());
        value.extend_from_slice(&perm.to_le_bytes());
        value.extend_from_slice(&u32::MAX.to_le_bytes());
    }
    let planted = unsafe {
        libc::setxattr(
            text.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    if planted != 0 {
        let error = std::io::Error::last_os_error();
        // Overlay and some container filesystems reject POSIX ACLs with
        // `EOPNOTSUPP` or `EINVAL` and store nothing. `ENOTSUP` is the same
        // value as `EOPNOTSUPP` on Linux.
        assert!(
            matches!(
                error.raw_os_error(),
                Some(libc::EOPNOTSUPP) | Some(libc::EINVAL)
            ),
            "{error}"
        );
        let store = open(&fixture.engine, as_vault(&vault));
        assert_eq!(store.load().unwrap().as_deref(), Some(b"leased".as_slice()));
        return;
    }
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_vault(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    assert!(lock.is_file());
    assert!(fixture.database().is_file());
    assert_eq!(vault.writes(), 1);
}

#[cfg(unix)]
#[test]
fn the_rollback_journal_is_owner_readable_only_while_a_write_is_open() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"first".to_vec()).unwrap();
    let barrier = fixture.root.join("barrier");
    fs::create_dir(&barrier).unwrap();
    store.arm_failure(Failure::Barrier(barrier.clone()));
    let journal = fixture.engine.join("installation.sqlite3-journal");
    let watch = std::thread::spawn(move || {
        let started = Instant::now();
        let marker = barrier.join("after-write");
        while !marker.exists() {
            assert!(started.elapsed() < Duration::from_secs(8), "write barrier");
            std::thread::sleep(Duration::from_millis(5));
        }
        let bits = mode_bits(&journal);
        fs::write(barrier.join("continue"), b"1").unwrap();
        bits
    });
    store.save(b"second-during-journal".to_vec()).unwrap();
    assert_eq!(watch.join().unwrap(), 0o600);
}

#[test]
fn a_statement_failure_keeps_the_committed_row_and_the_connection() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"committed".to_vec()).unwrap();
    let nonce = nonce_of(&fixture.database());
    store.arm_failure(Failure::Statement);
    assert_eq!(
        store.save(b"rejected".to_vec()).unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"committed".as_slice())
    );
    assert_eq!(nonce_of(&fixture.database()), nonce);
    store.save(b"retried".to_vec()).unwrap();
    assert_ne!(nonce_of(&fixture.database()), nonce);
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"retried".as_slice())
    );
}

#[test]
fn a_busy_begin_is_unavailable_and_a_later_save_uses_a_fresh_nonce() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"stable".to_vec()).unwrap();
    let nonce = nonce_of(&fixture.database());
    let connection = rusqlite::Connection::open(fixture.database()).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = Instant::now();
    let error = store.save(b"blocked".to_vec()).unwrap_err();
    let elapsed = started.elapsed();
    assert_eq!(error, FfiError::StoreUnavailable);
    // The save waits out the busy timeout before it gives up; any longer
    // is a busy computer, and only a hang fails.
    assert!(elapsed >= Duration::from_millis(700), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(60), "{elapsed:?}");
    assert_eq!(nonce_of(&fixture.database()), nonce);
    connection.execute_batch("ROLLBACK").unwrap();
    drop(connection);
    store.save(b"after-busy".to_vec()).unwrap();
    assert_ne!(nonce_of(&fixture.database()), nonce);
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"after-busy".as_slice())
    );
}

#[test]
fn a_non_busy_commit_failure_requires_restart_and_keeps_the_old_row() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"before-commit-hook".to_vec()).unwrap();
    store.arm_failure(Failure::Commit);
    assert_eq!(
        store.save(b"should-roll-back".to_vec()).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert_eq!(store.load().unwrap_err(), FfiError::StoreRecoveryRequired);
    assert_eq!(
        store.save(b"again".to_vec()).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    drop(store);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"before-commit-hook".as_slice())
    );
}

#[test]
fn a_full_write_keeps_the_old_row_and_the_connection() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"committed".to_vec()).unwrap();
    let nonce = nonce_of(&fixture.database());
    let pages = store.pragma_i64("page_count").unwrap();
    store.set_max_page_count(pages).unwrap();
    let too_large = vec![0xab; 256 * 1024];
    assert_eq!(
        store.save(too_large).unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"committed".as_slice())
    );
    assert_eq!(nonce_of(&fixture.database()), nonce);
    store.set_max_page_count(2048).unwrap();
    store.save(b"retried".to_vec()).unwrap();
    assert_ne!(nonce_of(&fixture.database()), nonce);
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"retried".as_slice())
    );
}

#[test]
fn an_already_ended_statement_keeps_the_old_row_and_the_connection() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"committed".to_vec()).unwrap();
    let nonce = nonce_of(&fixture.database());
    store.arm_failure(Failure::EndedStatement);
    assert_eq!(
        store.save(b"rejected".to_vec()).unwrap_err(),
        FfiError::StoreUnavailable
    );
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"committed".as_slice())
    );
    assert_eq!(nonce_of(&fixture.database()), nonce);
    store.save(b"retried".to_vec()).unwrap();
    assert_ne!(nonce_of(&fixture.database()), nonce);
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"retried".as_slice())
    );
}

#[test]
fn an_unconfirmed_rollback_requires_restart() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"before-rollback".to_vec()).unwrap();
    store.arm_failure(Failure::Statement);
    store.arm_failure(Failure::Rollback);
    assert_eq!(
        store.save(b"uncertain".to_vec()).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert_eq!(store.load().unwrap_err(), FfiError::StoreRecoveryRequired);
    drop(store);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"before-rollback".as_slice())
    );
}

#[test]
fn a_failed_directory_sync_keeps_the_committed_row_and_fences_this_process() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"old".to_vec()).unwrap();
    store.arm_failure(Failure::DirectorySync);
    assert_eq!(
        store.save(b"committed-then-unsynced".to_vec()).unwrap_err(),
        FfiError::StoreRecoveryRequired
    );
    assert_eq!(store.load().unwrap_err(), FfiError::StoreRecoveryRequired);
    drop(store);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"committed-then-unsynced".as_slice())
    );
}

#[test]
fn a_panic_during_save_fences_this_process_and_the_next_open_sees_the_old_row() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"before-panic".to_vec()).unwrap();
    store.arm_failure(Failure::Panic);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store.save(b"panicked".to_vec());
    }));
    assert!(panicked.is_err());
    assert_eq!(store.load().unwrap_err(), FfiError::StoreRecoveryRequired);
    drop(store);
    let store = open(&fixture.engine, as_vault(&vault));
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"before-panic".as_slice())
    );
}

#[test]
fn aborting_before_commit_restores_the_previous_complete_record() {
    if child_env_is("before-commit") {
        child_crash(false);
        return;
    }
    let fixture = Fixture::new();
    let key = fixture.key_path();
    let vault = file_vault(key.clone());
    let store = open(&fixture.engine, as_file(&vault));
    store.save(OLD_SENTINEL.to_vec()).unwrap();
    assert_eq!(vault.writes(), 1);
    drop(store);
    let status = spawn_child("before-commit", &fixture, &key);
    assert!(!status.success(), "child status {status}");
    let journal = fixture.engine.join("installation.sqlite3-journal");
    assert!(
        journal.is_file(),
        "interrupted transaction kept its journal"
    );
    assert!(!contains_bytes(&fixture.database(), NEW_SENTINEL));
    assert!(!contains_bytes(&journal, NEW_SENTINEL));
    assert!(!contains_bytes(&fixture.database(), OLD_SENTINEL));
    let store = open(&fixture.engine, as_file(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(OLD_SENTINEL));
}

#[test]
fn aborting_after_commit_loads_the_new_complete_record() {
    if child_env_is("after-commit") {
        child_crash(true);
        return;
    }
    let fixture = Fixture::new();
    let key = fixture.key_path();
    let vault = file_vault(key.clone());
    let store = open(&fixture.engine, as_file(&vault));
    store.save(OLD_SENTINEL.to_vec()).unwrap();
    drop(store);
    let status = spawn_child("after-commit", &fixture, &key);
    assert!(!status.success(), "child status {status}");
    assert!(!contains_bytes(&fixture.database(), NEW_SENTINEL));
    assert!(!contains_bytes(&fixture.database(), OLD_SENTINEL));
    let journal = fixture.engine.join("installation.sqlite3-journal");
    if journal.exists() {
        assert!(!contains_bytes(&journal, NEW_SENTINEL));
    }
    let store = open(&fixture.engine, as_file(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(NEW_SENTINEL));
}

#[test]
fn a_second_process_cannot_take_a_held_lease() {
    if child_env_is("hold") {
        child_hold();
        return;
    }
    let fixture = Fixture::new();
    let key = fixture.key_path();
    let vault = file_vault(key.clone());
    let store = open(&fixture.engine, as_file(&vault));
    store.save(b"shared".to_vec()).unwrap();
    drop(store);
    let signal = fixture.root.join("held");
    let continue_at = fixture.root.join("continue");
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .arg(std::thread::current().name().unwrap())
        .arg("--exact")
        .env("VNILINK_STORE_CHILD", "hold")
        .env("VNILINK_STORE_ENGINE", &fixture.engine)
        .env("VNILINK_STORE_KEY", &key)
        .env("VNILINK_STORE_SIGNAL", &signal)
        .env("VNILINK_STORE_CONTINUE", &continue_at)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = child.spawn().unwrap();
    let started = Instant::now();
    while !signal.exists() {
        assert!(started.elapsed() < Duration::from_secs(8), "lease signal");
        std::thread::sleep(Duration::from_millis(10));
    }
    let err = EncryptedInstallationStore::open_fixture(&fixture.engine, as_file(&vault));
    assert_eq!(err.unwrap_err(), FfiError::StoreUnavailable);
    let lock = fixture.engine.join("installation.lock");
    assert!(lock.is_file());
    fs::write(&continue_at, b"1").unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "{status}");
    assert!(lock.is_file());
    let store = open(&fixture.engine, as_file(&vault));
    assert_eq!(store.load().unwrap().as_deref(), Some(b"shared".as_slice()));
}

fn child_crash(after_commit: bool) {
    let engine = PathBuf::from(std::env::var("VNILINK_STORE_ENGINE").unwrap());
    let key = PathBuf::from(std::env::var("VNILINK_STORE_KEY").unwrap());
    let vault = file_vault(key);
    let store = open(&engine, as_file(&vault));
    let failure = if after_commit {
        Failure::CrashAfterCommit
    } else {
        Failure::CrashBeforeCommit
    };
    store.arm_failure(failure);
    let _ = store.save(NEW_SENTINEL.to_vec());
    eprintln!("installation crash hook returned");
    std::process::exit(4);
}

fn child_hold() {
    let engine = PathBuf::from(std::env::var("VNILINK_STORE_ENGINE").unwrap());
    let key = PathBuf::from(std::env::var("VNILINK_STORE_KEY").unwrap());
    let signal = PathBuf::from(std::env::var("VNILINK_STORE_SIGNAL").unwrap());
    let continue_at = PathBuf::from(std::env::var("VNILINK_STORE_CONTINUE").unwrap());
    let vault = file_vault(key);
    let store = open(&engine, as_file(&vault));
    fs::write(&signal, b"held").unwrap();
    let started = Instant::now();
    while !continue_at.exists() {
        if started.elapsed() > Duration::from_secs(10) {
            eprintln!("lease child timed out");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(store);
}

fn spawn_child(mode: &str, fixture: &Fixture, key: &Path) -> std::process::ExitStatus {
    let output = Command::new(std::env::current_exe().unwrap())
        .arg(std::thread::current().name().unwrap())
        .arg("--exact")
        .env("VNILINK_STORE_CHILD", mode)
        .env("VNILINK_STORE_ENGINE", &fixture.engine)
        .env("VNILINK_STORE_KEY", key)
        .output()
        .unwrap();
    if output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    output.status
}

#[test]
fn developer_reset_moves_artifacts_before_deleting_the_key() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"quarantine".to_vec()).unwrap();
    drop(store);
    let wal = fixture.engine.join("installation.sqlite3-wal");
    fs::write(&wal, b"unexpected").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&wal, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let paths = paths::InstallationPaths::from_engine(fixture.engine.clone());
    let recovered = recover_installation(&paths, vault.as_ref()).unwrap();
    assert!(recovered.key_deleted);
    assert!(recovered
        .moved
        .iter()
        .any(|name| name == "installation.sqlite3"));
    assert!(recovered.moved.iter().any(|name| name.contains("wal")));
    assert!(!fixture.database().exists());
    assert!(fixture.engine.join("installation.lock").is_file());
    assert!(vault.read().unwrap().is_none());
    let recovery = fs::read_dir(&fixture.engine)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("installation-recovery-")
        })
        .unwrap();
    assert!(recovery.path().join("installation.sqlite3").is_file());
    assert!(recovery.path().join("installation.sqlite3-wal").is_file());
}

#[test]
fn developer_reset_stops_when_an_artifact_is_unsafe() {
    let fixture = Fixture::new();
    let vault = memory();
    let store = open(&fixture.engine, as_vault(&vault));
    store.save(b"keep-key".to_vec()).unwrap();
    drop(store);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(fixture.database(), fs::Permissions::from_mode(0o644)).unwrap();
        let paths = paths::InstallationPaths::from_engine(fixture.engine.clone());
        let error = recover_installation(&paths, vault.as_ref()).unwrap_err();
        let rendered = error.to_string();
        assert!(rendered.contains("validate"), "{rendered}");
        assert_eq!(vault.stored_len(), Some(32));
        assert!(fixture.database().is_file());
        assert_eq!(mode_bits(&fixture.database()), 0o644);
    }
}

#[test]
fn developer_reset_with_nothing_to_move_still_deletes_the_key() {
    let fixture = Fixture::new();
    let vault = memory();
    vault.write(&[9; 32]).unwrap();
    let paths = paths::InstallationPaths::from_engine(fixture.engine.clone());
    let recovered = recover_installation(&paths, vault.as_ref()).unwrap();
    assert!(recovered.moved.is_empty());
    assert!(recovered.key_deleted);
    assert!(vault.read().unwrap().is_none());
    let names: Vec<_> = fs::read_dir(&fixture.engine)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|name| name == "installation.lock"));
    assert!(!names
        .iter()
        .any(|name| name.starts_with("installation-recovery-")));
}

#[cfg(feature = "reset-installation")]
#[test]
fn reset_cli_stops_before_resolving_a_path_without_confirmation() {
    if std::env::args().any(|arg| arg == "--confirm-forget-installation") {
        return;
    }
    assert_eq!(super::reset_installation_cli(), 2);
}

#[test]
#[ignore = "writes one unique Keychain entry and may prompt"]
fn keychain_round_trip_uses_a_disposable_entry() {
    let service = format!("com.vnilink.storage.test.{}", unique());
    assert_ne!(service, KEY_SERVICE);
    let vault = Arc::new(KeyringVault::named(&service, "installation-key-test"));
    let _ = KeyVault::delete(vault.as_ref());
    let fixture = Fixture::new();
    let store = EncryptedInstallationStore::open_fixture(&fixture.engine, vault.clone()).unwrap();
    store.save(b"keychain-fixture".to_vec()).unwrap();
    drop(store);
    let store = EncryptedInstallationStore::open_fixture(&fixture.engine, vault.clone()).unwrap();
    assert_eq!(
        store.load().unwrap().as_deref(),
        Some(b"keychain-fixture".as_slice())
    );
    drop(store);
    KeyVault::delete(vault.as_ref()).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn electron_application_directory_can_contain_a_private_engine_store() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let app = fixture.root.join("VniLink");
    fs::create_dir(&app).unwrap();
    fs::set_permissions(&app, fs::Permissions::from_mode(0o755)).unwrap();
    let settings = app.join("settings.json");
    fs::write(&settings, b"existing Electron settings").unwrap();

    let prepared = paths::prepare_application(&app).expect("existing Electron parent is trusted");
    let store = open(&prepared.engine_dir, as_vault(&memory()));
    store.save(b"private installation".to_vec()).unwrap();
    assert_eq!(store.load().unwrap().unwrap(), b"private installation");
    assert_eq!(
        mode_bits(&app),
        0o755,
        "must not chmod Electron's directory"
    );
    assert_eq!(mode_bits(&prepared.engine_dir), 0o700);
    assert_eq!(mode_bits(&prepared.database), 0o600);
    assert_eq!(fs::read(settings).unwrap(), b"existing Electron settings");

    let unsafe_app = fixture.root.join("writable-by-others");
    fs::create_dir(&unsafe_app).unwrap();
    fs::set_permissions(&unsafe_app, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(paths::prepare_application(&unsafe_app).is_err());
    assert!(!unsafe_app.join("engine").exists());
    assert_eq!(mode_bits(&unsafe_app), 0o777);
}
