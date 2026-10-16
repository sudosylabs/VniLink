//! Encrypted installation store.
//!
//! Rust decides when a record is written. This adapter stores that opaque
//! record as one authenticated row and keeps only the 32-byte key in the
//! platform credential store (`com.vnilink.storage` / `installation-key`).
//! The database is `installation.sqlite3` under the per-user engine directory:
//! `~/Library/Application Support/VniLink/engine` on macOS,
//! `%LOCALAPPDATA%\VniLink\engine` on Windows, and
//! `$XDG_DATA_HOME/vnilink/engine` or `~/.local/share/vnilink/engine` on Linux.
//!
//! `load` returns nothing only while this process has just created an empty
//! database and has not committed a row. A missing key, a missing database,
//! a journal without its database, WAL/SHM sidecars, or a failed authentication
//! check is an error. Those files are left in place. An uncertain commit
//! refuses later reads and writes in this process.
//!
//! Confidentiality holds when the key is unavailable to the attacker. An
//! unlocked credential store lets the same user open the record. Replacing
//! the database with an older authentic copy is not detected.
//!
//! The developer reset is not part of the application. With the
//! `reset-installation` feature:
//! `cargo run -p vnilink-node --features reset-installation --bin vnilink-reset-installation -- --confirm-forget-installation`

mod crypto;
mod database;
mod key;
mod paths;
mod recover;

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::Instant;

use zeroize::Zeroizing;

use vnilink_app::{CredentialStore, FfiError};

use crypto::KEY_LEN;
use database::{InstallationDatabase, WriteResult};
use key::{KeyAccess, KeyVault, KeyringVault};
use paths::{InstallationLease, InstallationPaths};

const PHASE_AWAITING: u8 = 1;
const PHASE_READY: u8 = 2;
const PHASE_RECOVERY: u8 = 3;

#[derive(Debug)]
pub(crate) enum StoreFailure {
    Locked,
    Unavailable,
    Corrupt,
    RecoveryRequired,
}

impl From<StoreFailure> for FfiError {
    fn from(failure: StoreFailure) -> Self {
        match failure {
            StoreFailure::Locked => FfiError::StoreLocked,
            StoreFailure::Unavailable => FfiError::StoreUnavailable,
            StoreFailure::Corrupt => FfiError::CorruptRecord,
            StoreFailure::RecoveryRequired => FfiError::StoreRecoveryRequired,
        }
    }
}

enum Phase {
    AwaitingInitialSave,
    Ready,
    RecoveryRequired,
}

struct Inner {
    database: InstallationDatabase,
    phase: Phase,
}

pub struct EncryptedInstallationStore {
    inner: Mutex<Inner>,
    // Drop closes the database before this lease unlocks.
    #[allow(dead_code)]
    lease: InstallationLease,
    key: Zeroizing<[u8; KEY_LEN]>,
    key_id: [u8; KEY_LEN],
    paths: InstallationPaths,
    phase: AtomicU8,
    #[cfg(test)]
    fail_directory_sync: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for EncryptedInstallationStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EncryptedInstallationStore")
    }
}

struct PanicFlag<'a> {
    phase: &'a AtomicU8,
    armed: bool,
}

impl Drop for PanicFlag<'_> {
    fn drop(&mut self) {
        if self.armed && thread::panicking() {
            self.phase.store(PHASE_RECOVERY, Ordering::Release);
        }
    }
}

impl EncryptedInstallationStore {
    pub fn open_production() -> Result<Self, FfiError> {
        let paths = paths::production_paths()?;
        open_at(paths, Arc::new(KeyringVault::production()), false).map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn open_fixture(
        engine_dir: &std::path::Path,
        vault: Arc<dyn KeyVault>,
    ) -> Result<Self, FfiError> {
        open_at(
            InstallationPaths::from_engine(engine_dir.to_path_buf()),
            vault,
            false,
        )
        .map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn open_fixture_fail_after_key(
        engine_dir: &std::path::Path,
        vault: Arc<dyn KeyVault>,
    ) -> Result<Self, FfiError> {
        open_at(
            InstallationPaths::from_engine(engine_dir.to_path_buf()),
            vault,
            true,
        )
        .map_err(FfiError::from)
    }

    fn lock_inner(&self) -> Result<MutexGuard<'_, Inner>, StoreFailure> {
        let started = Instant::now();
        loop {
            match self.inner.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(poison)) => {
                    let mut guard = poison.into_inner();
                    guard.phase = Phase::RecoveryRequired;
                    self.phase.store(PHASE_RECOVERY, Ordering::Release);
                    eprintln!("vnilink: installation store lock was poisoned; restart required");
                    return Err(StoreFailure::RecoveryRequired);
                }
                Err(TryLockError::WouldBlock) => {
                    if started.elapsed() >= paths::lock_budget() {
                        return Err(StoreFailure::Unavailable);
                    }
                    thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }

    fn mark_recovery(guard: &mut Inner, phase: &AtomicU8, code: i32) {
        guard.phase = Phase::RecoveryRequired;
        phase.store(PHASE_RECOVERY, Ordering::Release);
        eprintln!(
            "vnilink: installation save outcome is unknown (sqlite {code}); restart required"
        );
    }

    #[cfg(test)]
    pub(crate) fn arm_failure(&self, kind: Failure) {
        let mut guard = self.lock_inner().expect("arm");
        let hooks = guard.database.hooks_mut();
        match kind {
            Failure::Statement => hooks.fail_statement = true,
            Failure::Commit => hooks.fail_commit = true,
            Failure::Rollback => hooks.fail_rollback = true,
            Failure::DirectorySync => {
                drop(guard);
                self.fail_directory_sync.store(true, Ordering::Release);
            }
            Failure::CrashAfterWrite => hooks.crash_after_write = true,
            Failure::CrashBeforeCommit => hooks.crash_before_commit = true,
            Failure::CrashAfterCommit => hooks.crash_after_commit = true,
            Failure::Panic => hooks.panic_in_write = true,
            Failure::EndedStatement => hooks.ended_statement = true,
            Failure::Barrier(dir) => hooks.barrier = Some(dir),
        }
    }

    #[cfg(test)]
    pub(crate) fn pragma_text(&self, name: &str) -> Result<String, FfiError> {
        let guard = self.lock_inner()?;
        guard.database.pragma_text(name).map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn pragma_i64(&self, name: &str) -> Result<i64, FfiError> {
        let guard = self.lock_inner()?;
        guard.database.pragma_i64(name).map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn set_max_page_count(&self, pages: i64) -> Result<(), FfiError> {
        let guard = self.lock_inner()?;
        guard
            .database
            .set_max_page_count(pages)
            .map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn sqlite_limit(&self, limit: rusqlite::limits::Limit) -> Result<i32, FfiError> {
        let guard = self.lock_inner()?;
        guard.database.limit(limit).map_err(FfiError::from)
    }

    #[cfg(test)]
    pub(crate) fn vfs_name(&self) -> Result<String, FfiError> {
        let guard = self.lock_inner()?;
        guard.database.vfs_name().map_err(FfiError::from)
    }
}

#[cfg(test)]
pub(crate) enum Failure {
    Statement,
    Commit,
    Rollback,
    DirectorySync,
    #[allow(dead_code)]
    CrashAfterWrite,
    CrashBeforeCommit,
    CrashAfterCommit,
    Panic,
    /// The write statement observes a transaction SQLite has already ended.
    EndedStatement,
    Barrier(std::path::PathBuf),
}

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod tests;

fn open_at(
    paths: InstallationPaths,
    vault: Arc<dyn KeyVault>,
    #[cfg(test)] fail_after_key: bool,
    #[cfg(not(test))] fail_after_key: bool,
) -> Result<EncryptedInstallationStore, StoreFailure> {
    paths::validate_dir(&paths.engine_dir)?;
    if paths::location_is_remote(&paths.engine_dir)? {
        return Err(StoreFailure::Unavailable);
    }
    let lease = InstallationLease::acquire(&paths)?;
    let presence = inspect(&paths)?;
    let stored = vault.read().map_err(map_access)?;
    let (key, database, phase) = match stored {
        None if !presence.database && !presence.journal => {
            provision(vault.as_ref(), &paths, fail_after_key)?
        }
        None => return Err(StoreFailure::Unavailable),
        Some(bytes) => {
            let key = key::require_key(bytes)?;
            if !presence.database {
                return Err(StoreFailure::Unavailable);
            }
            paths::validate_file(&paths.database)?;
            if presence.journal {
                paths::validate_file(&paths.journal)?;
            }
            let database = InstallationDatabase::open_existing(&paths.database)?;
            (key, database, Phase::Ready)
        }
    };
    let key_id = crypto::key_id(&key);
    let phase_byte = match phase {
        Phase::AwaitingInitialSave => PHASE_AWAITING,
        Phase::Ready => PHASE_READY,
        Phase::RecoveryRequired => PHASE_RECOVERY,
    };
    Ok(EncryptedInstallationStore {
        inner: Mutex::new(Inner { database, phase }),
        lease,
        key,
        key_id,
        paths,
        phase: AtomicU8::new(phase_byte),
        #[cfg(test)]
        fail_directory_sync: std::sync::atomic::AtomicBool::new(false),
    })
}

struct Presence {
    database: bool,
    journal: bool,
}

fn inspect(paths: &InstallationPaths) -> Result<Presence, StoreFailure> {
    if paths::file_len(&paths.wal)?.is_some() || paths::file_len(&paths.shm)?.is_some() {
        return Err(StoreFailure::Corrupt);
    }
    let database = paths::file_len(&paths.database)?;
    let journal = paths::file_len(&paths.journal)?;
    if journal.is_some() && database.is_none() {
        return Err(StoreFailure::Corrupt);
    }
    if database.is_some_and(database::database_too_large)
        || journal.is_some_and(database::journal_too_large)
    {
        return Err(StoreFailure::Unavailable);
    }
    Ok(Presence {
        database: database.is_some(),
        journal: journal.is_some(),
    })
}

fn provision(
    vault: &dyn KeyVault,
    paths: &InstallationPaths,
    fail_after_key: bool,
) -> Result<(Zeroizing<[u8; KEY_LEN]>, InstallationDatabase, Phase), StoreFailure> {
    let generated = crypto::random_key()?;
    vault.write(&generated).map_err(map_access)?;
    let read_back = vault
        .read()
        .map_err(map_access)?
        .ok_or(StoreFailure::Unavailable)?;
    let key = key::require_key(read_back)?;
    if !crypto::constant_time_eq(key.as_slice(), generated.as_slice()) {
        return Err(StoreFailure::Unavailable);
    }
    if fail_after_key {
        return Err(StoreFailure::Unavailable);
    }
    paths::create_private_file(&paths.database)?;
    // The key is already stored. After this point a retry must not provision
    // again: the next open sees the key and keeps these files.
    let database = match InstallationDatabase::create(&paths.database) {
        Ok(database) => database,
        Err(error) => {
            #[cfg(test)]
            database::clear_schema_fault();
            return Err(error);
        }
    };
    let synced = database::sync_created(&paths.database, &paths.engine_dir);
    #[cfg(test)]
    let synced = if database::take_schema_fault(database::SchemaFault::DirectorySync) {
        Err(StoreFailure::Unavailable)
    } else {
        synced
    };
    if synced.is_err() {
        return Err(StoreFailure::RecoveryRequired);
    }
    Ok((key, database, Phase::AwaitingInitialSave))
}

fn map_access(access: KeyAccess) -> StoreFailure {
    match access {
        KeyAccess::Locked => StoreFailure::Locked,
        KeyAccess::Unavailable => StoreFailure::Unavailable,
    }
}

impl CredentialStore for EncryptedInstallationStore {
    fn load(&self) -> Result<Option<Vec<u8>>, FfiError> {
        if self.phase.load(Ordering::Acquire) == PHASE_RECOVERY {
            return Err(FfiError::StoreRecoveryRequired);
        }
        let guard = self.lock_inner()?;
        match guard.phase {
            Phase::RecoveryRequired => return Err(FfiError::StoreRecoveryRequired),
            Phase::AwaitingInitialSave => return Ok(None),
            Phase::Ready => {}
        }
        let row = guard.database.read()?;
        drop(guard);
        let Some(row) = row else {
            return Err(FfiError::CorruptRecord);
        };
        crypto::decrypt(
            &self.key,
            &self.key_id,
            &row.key_id,
            &row.nonce,
            &row.ciphertext,
        )
        .map(Some)
        .map_err(FfiError::from)
    }

    fn save(&self, record: Vec<u8>) -> Result<(), FfiError> {
        if self.phase.load(Ordering::Acquire) == PHASE_RECOVERY {
            return Err(FfiError::StoreRecoveryRequired);
        }
        let (nonce, ciphertext) = crypto::encrypt(&self.key, &self.key_id, &record)?;
        let mut guard = self.lock_inner()?;
        if matches!(guard.phase, Phase::RecoveryRequired) {
            return Err(FfiError::StoreRecoveryRequired);
        }
        // A reopened schema with no row is an interrupted init. Only the
        // process that created it may treat that as the first save.
        if matches!(guard.phase, Phase::Ready) && guard.database.read()?.is_none() {
            return Err(FfiError::CorruptRecord);
        }
        let effect = {
            let mut flag = PanicFlag {
                phase: &self.phase,
                armed: true,
            };
            let effect = guard.database.write(&self.key_id, &nonce, &ciphertext)?;
            flag.armed = false;
            effect
        };
        match effect {
            WriteResult::Durable => {
                let fail_sync = cfg_sync_failure(self);
                if fail_sync
                    || database::sync_created(&self.paths.database, &self.paths.engine_dir).is_err()
                {
                    Self::mark_recovery(&mut guard, &self.phase, 0);
                    return Err(FfiError::StoreRecoveryRequired);
                }
                guard.phase = Phase::Ready;
                self.phase.store(PHASE_READY, Ordering::Release);
                Ok(())
            }
            WriteResult::Unchanged => Err(FfiError::StoreUnavailable),
            WriteResult::Uncertain { code } => {
                Self::mark_recovery(&mut guard, &self.phase, code);
                Err(FfiError::StoreRecoveryRequired)
            }
        }
    }
}

fn cfg_sync_failure(store: &EncryptedInstallationStore) -> bool {
    #[cfg(test)]
    {
        store.fail_directory_sync.swap(false, Ordering::AcqRel)
    }
    #[cfg(not(test))]
    {
        let _ = store;
        false
    }
}

#[cfg(test)]
pub(crate) fn recover_installation(
    paths: &InstallationPaths,
    vault: &dyn KeyVault,
) -> Result<recover::Recovered, recover::RecoverFailure> {
    recover::recover(paths, vault)
}

#[cfg(feature = "reset-installation")]
pub fn reset_installation_cli() -> i32 {
    if !std::env::args().any(|arg| arg == "--confirm-forget-installation") {
        eprintln!("pass --confirm-forget-installation to forget the installation");
        return 2;
    }
    let paths = match paths::production_paths() {
        Ok(paths) => paths,
        Err(_) => {
            eprintln!("the installation store is unavailable");
            return 1;
        }
    };
    println!("installation directory: {}", paths.engine_dir.display());
    println!("database: {}", paths.database.display());
    println!("credential service: {}", key::KEY_SERVICE);
    println!("credential account: {}", key::KEY_ACCOUNT);
    let vault = KeyringVault::production();
    match recover::recover(&paths, &vault) {
        Ok(report) => {
            println!("preserved {} file(s)", report.moved.len());
            for name in &report.moved {
                println!("moved {name}");
            }
            if report.key_deleted {
                println!("deleted the installation key");
            }
            0
        }
        Err(recover::RecoverFailure::Stopped { step }) => {
            eprintln!("reset stopped at {step}; the installation key was kept");
            1
        }
        Err(recover::RecoverFailure::Partial { step, moved }) => {
            eprintln!("reset stopped at {step}; the installation key was kept");
            for name in &moved {
                eprintln!("moved {name}");
            }
            1
        }
    }
}
