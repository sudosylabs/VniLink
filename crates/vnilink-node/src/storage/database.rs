//! One encrypted installation row in a private SQLite file.
//!
//! The connection uses SQLite's serialized mode and the store mutex. SQL is
//! the fixed statements in this file. A commit whose durable outcome is not
//! known is [`WriteResult::Uncertain`] even when a later rollback looks successful.
//! The same rule applies to the schema commit during creation.

#[cfg(test)]
use std::cell::Cell;
use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::crypto::{self, KEY_LEN, MAX_CIPHERTEXT, NONCE_LEN};
use super::paths;
use super::StoreFailure;

const APPLICATION_ID: i32 = 0x564E_4C49;
const PAGE_SIZE: i64 = 4096;
const MAX_PAGE_COUNT: i64 = 2048;
const MAX_DATABASE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
const LENGTH_LIMIT: i32 = 1_114_112;
const SQL_LIMIT: i32 = 16_384;

pub(crate) const CREATE_SQL: &str = "\
CREATE TABLE installation (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    key_id BLOB NOT NULL CHECK (length(key_id) = 32),
    nonce BLOB NOT NULL CHECK (length(nonce) = 24),
    ciphertext BLOB NOT NULL CHECK (length(ciphertext) BETWEEN 17 AND 1048592)
) STRICT";

const SELECT_LENGTHS: &str =
    "SELECT length(key_id), length(nonce), length(ciphertext) FROM installation WHERE id = 1";
const SELECT_ROW: &str = "SELECT key_id, nonce, ciphertext FROM installation WHERE id = 1";
const SELECT_NONCE: &str = "SELECT nonce FROM installation WHERE id = 1";
const UPDATE_ROW: &str =
    "UPDATE installation SET key_id = ?1, nonce = ?2, ciphertext = ?3 WHERE id = 1";
const INSERT_ROW: &str =
    "INSERT INTO installation (id, key_id, nonce, ciphertext) VALUES (1, ?1, ?2, ?3)";

pub(crate) struct InstallationDatabase {
    conn: Connection,
    #[cfg(test)]
    hooks: TestHooks,
}

pub(crate) struct StoredRow {
    pub(crate) key_id: Vec<u8>,
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

pub(crate) enum WriteResult {
    Durable,
    Unchanged,
    Uncertain { code: i32 },
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestHooks {
    pub(crate) fail_statement: bool,
    pub(crate) fail_commit: bool,
    pub(crate) fail_rollback: bool,
    pub(crate) crash_after_write: bool,
    pub(crate) crash_before_commit: bool,
    pub(crate) crash_after_commit: bool,
    pub(crate) panic_in_write: bool,
    /// The statement finds the transaction already ended, as `SQLITE_FULL` can.
    pub(crate) ended_statement: bool,
    pub(crate) barrier: Option<std::path::PathBuf>,
}

impl InstallationDatabase {
    pub(crate) fn create(path: &Path) -> Result<Self, StoreFailure> {
        let conn = open_connection(path)?;
        configure_connection(&conn)?;
        set_page_size(&conn)?;
        apply_durability(&conn)?;
        if conn.execute_batch("BEGIN IMMEDIATE").is_err() {
            return Err(if conn.is_autocommit() {
                StoreFailure::Unavailable
            } else {
                StoreFailure::RecoveryRequired
            });
        }
        #[cfg(test)]
        if take_schema_fault(SchemaFault::Statement) {
            let _ = conn.execute_batch("SELECT * FROM vnilink_absent_relation");
            return Err(abandon_uncommitted(&conn));
        }
        if conn.execute_batch(CREATE_SQL).is_err() {
            return Err(abandon_uncommitted(&conn));
        }
        if conn
            .pragma_update(None, "application_id", APPLICATION_ID)
            .is_err()
        {
            return Err(abandon_uncommitted(&conn));
        }
        #[cfg(test)]
        if take_schema_fault(SchemaFault::Commit) {
            // `true` asks SQLite to turn this COMMIT into a rollback.
            let _ = conn.commit_hook(Some(|| true));
        }
        let committed = conn.execute_batch("COMMIT");
        #[cfg(test)]
        {
            let _ = conn.commit_hook(None::<fn() -> bool>);
        }
        match committed {
            Ok(()) => {
                if !conn.is_autocommit() {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(StoreFailure::RecoveryRequired);
                }
            }
            Err(error) => {
                // A non-busy COMMIT failure stays fatal after an apparent rollback.
                // Busy plus a confirmed ended transaction committed nothing.
                if is_busy(&error) && rollback_ended(&conn) {
                    return Err(StoreFailure::Unavailable);
                }
                let _ = conn.execute_batch("ROLLBACK");
                return Err(StoreFailure::RecoveryRequired);
            }
        }
        // The schema transaction has committed. A later check failure must not
        // look like a retryable fresh installation.
        if verify_schema(&conn)
            .and_then(|_| verify_durability(&conn))
            .is_err()
        {
            return Err(StoreFailure::RecoveryRequired);
        }
        Ok(Self {
            conn,
            #[cfg(test)]
            hooks: TestHooks::default(),
        })
    }

    pub(crate) fn open_existing(path: &Path) -> Result<Self, StoreFailure> {
        let conn = open_connection(path)?;
        configure_connection(&conn)?;
        verify_existing(&conn)?;
        apply_connection_durability(&conn)?;
        verify_durability(&conn)?;
        Ok(Self {
            conn,
            #[cfg(test)]
            hooks: TestHooks::default(),
        })
    }

    pub(crate) fn read(&self) -> Result<Option<StoredRow>, StoreFailure> {
        let lengths = match self.conn.query_row(SELECT_LENGTHS, [], |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        }) {
            Ok(lengths) => lengths,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(error) => return Err(map_read(error)),
        };
        let (key_len, nonce_len, cipher_len) = lengths;
        if key_len != Some(KEY_LEN as i64)
            || nonce_len != Some(NONCE_LEN as i64)
            || cipher_len.is_none_or(|len| !(17..=MAX_CIPHERTEXT as i64).contains(&len))
        {
            return Err(StoreFailure::Corrupt);
        }
        let row = self
            .conn
            .query_row(SELECT_ROW, [], |row| {
                Ok(StoredRow {
                    key_id: row.get(0)?,
                    nonce: row.get(1)?,
                    ciphertext: row.get(2)?,
                })
            })
            .map_err(map_read)?;
        if row.key_id.len() != KEY_LEN
            || row.nonce.len() != NONCE_LEN
            || !(17..=MAX_CIPHERTEXT).contains(&row.ciphertext.len())
        {
            return Err(StoreFailure::Corrupt);
        }
        Ok(Some(row))
    }

    pub(crate) fn write(
        &mut self,
        key_id: &[u8; KEY_LEN],
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<WriteResult, StoreFailure> {
        if !(17..=MAX_CIPHERTEXT).contains(&ciphertext.len()) {
            return Err(StoreFailure::Unavailable);
        }
        if let Err(error) = self.conn.execute_batch("BEGIN IMMEDIATE") {
            return Ok(self.classify_begin(error));
        }
        let before = match self.nonce_snapshot() {
            Ok(nonce) => nonce,
            Err(_) => return Ok(self.abandon_uncertain(0)),
        };
        if let Err(error) = self.write_statement(key_id, nonce, ciphertext) {
            return Ok(self.classify_statement(error, &before));
        }
        #[cfg(test)]
        {
            if self.hooks.panic_in_write {
                self.hooks.panic_in_write = false;
                panic!("installation store test panic");
            }
            if self.hooks.crash_after_write {
                std::process::abort();
            }
            if let Some(dir) = self.hooks.barrier.take() {
                wait_for_barrier(&dir);
            }
            if self.hooks.crash_before_commit && !self.conn.is_autocommit() {
                std::process::abort();
            }
            if self.hooks.fail_commit {
                self.hooks.fail_commit = false;
                // `true` asks SQLite to turn this COMMIT into a rollback.
                let _ = self.conn.commit_hook(Some(|| true));
            }
        }
        match self.conn.execute_batch("COMMIT") {
            Ok(()) => {
                self.clear_commit_hook();
                if !self.conn.is_autocommit() {
                    return Ok(self.abandon_uncertain(0));
                }
                #[cfg(test)]
                if self.hooks.crash_after_commit {
                    self.hooks.crash_after_commit = false;
                    std::process::abort();
                }
                Ok(WriteResult::Durable)
            }
            Err(error) => {
                self.clear_commit_hook();
                Ok(self.classify_commit(error, &before))
            }
        }
    }

    fn write_statement(
        &mut self,
        key_id: &[u8; KEY_LEN],
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
    ) -> Result<(), rusqlite::Error> {
        #[cfg(test)]
        if self.hooks.ended_statement {
            self.hooks.ended_statement = false;
            // SQLite may already have rolled the transaction back. A later
            // ROLLBACK then fails even though the committed row is unchanged.
            self.conn.execute_batch("ROLLBACK")?;
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
                None,
            ));
        }
        #[cfg(test)]
        if self.hooks.fail_statement {
            self.hooks.fail_statement = false;
            self.conn
                .execute_batch("SELECT * FROM vnilink_absent_relation")?;
        }
        let updated = self.conn.execute(
            UPDATE_ROW,
            rusqlite::params![key_id.as_slice(), nonce.as_slice(), ciphertext],
        )?;
        if updated == 0 {
            self.conn.execute(
                INSERT_ROW,
                rusqlite::params![key_id.as_slice(), nonce.as_slice(), ciphertext],
            )?;
        }
        Ok(())
    }

    fn classify_begin(&mut self, error: rusqlite::Error) -> WriteResult {
        if self.conn.is_autocommit() {
            WriteResult::Unchanged
        } else {
            self.abandon_uncertain(sqlite_code(&error))
        }
    }

    fn classify_statement(
        &mut self,
        error: rusqlite::Error,
        before: &Option<Vec<u8>>,
    ) -> WriteResult {
        let code = sqlite_code(&error);
        if self.rollback_confirmed() && self.nonce_matches(before) {
            WriteResult::Unchanged
        } else {
            WriteResult::Uncertain { code }
        }
    }

    fn classify_commit(&mut self, error: rusqlite::Error, before: &Option<Vec<u8>>) -> WriteResult {
        let code = sqlite_code(&error);
        let busy = is_busy(&error);
        let rolled_back = self.rollback_confirmed();
        let unchanged = self.nonce_matches(before);
        if busy && rolled_back && unchanged {
            WriteResult::Unchanged
        } else {
            // A non-busy commit failure stays fatal after an apparent rollback.
            WriteResult::Uncertain { code }
        }
    }

    fn abandon_uncertain(&mut self, code: i32) -> WriteResult {
        let _ = self.rollback_confirmed();
        WriteResult::Uncertain { code }
    }

    fn rollback_confirmed(&mut self) -> bool {
        #[cfg(test)]
        if self.hooks.fail_rollback {
            self.hooks.fail_rollback = false;
            return false;
        }
        // `SQLITE_FULL`, `SQLITE_IOERR`, `SQLITE_NOMEM`, `SQLITE_BUSY`, and
        // `SQLITE_INTERRUPT` can end the transaction before the caller issues
        // ROLLBACK. That later command fails even though the committed row
        // is unchanged. Autocommit is the confirmation, not a successful
        // ROLLBACK string.
        if self.conn.is_autocommit() {
            return true;
        }
        self.conn.execute_batch("ROLLBACK").is_ok() && self.conn.is_autocommit()
    }

    fn nonce_matches(&self, before: &Option<Vec<u8>>) -> bool {
        match self.nonce_snapshot() {
            Ok(after) => after == *before,
            Err(_) => false,
        }
    }

    fn nonce_snapshot(&self) -> Result<Option<Vec<u8>>, rusqlite::Error> {
        match self.conn.query_row(SELECT_NONCE, [], |row| row.get(0)) {
            Ok(nonce) => Ok(Some(nonce)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn clear_commit_hook(&self) {
        #[cfg(test)]
        {
            let _ = self.conn.commit_hook(None::<fn() -> bool>);
        }
    }

    #[cfg(test)]
    pub(crate) fn hooks_mut(&mut self) -> &mut TestHooks {
        &mut self.hooks
    }

    #[cfg(test)]
    pub(crate) fn vfs_name(&self) -> Result<String, StoreFailure> {
        selected_vfs_name(&self.conn)
    }

    #[cfg(test)]
    pub(crate) fn pragma_text(&self, name: &str) -> Result<String, StoreFailure> {
        self.conn
            .pragma_query_value(None, name, |row| row.get(0))
            .map_err(|_| StoreFailure::Unavailable)
    }

    #[cfg(test)]
    pub(crate) fn pragma_i64(&self, name: &str) -> Result<i64, StoreFailure> {
        self.conn
            .pragma_query_value(None, name, |row| row.get(0))
            .map_err(|_| StoreFailure::Unavailable)
    }

    #[cfg(test)]
    pub(crate) fn set_max_page_count(&self, pages: i64) -> Result<(), StoreFailure> {
        self.conn
            .pragma_update(None, "max_page_count", pages)
            .map_err(|_| StoreFailure::Unavailable)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn limit(&self, limit: rusqlite::limits::Limit) -> Result<i32, StoreFailure> {
        self.conn
            .limit(limit)
            .map_err(|_| StoreFailure::Unavailable)
    }
}

pub(crate) fn database_too_large(bytes: u64) -> bool {
    bytes > MAX_DATABASE_BYTES
}

pub(crate) fn journal_too_large(bytes: u64) -> bool {
    bytes > MAX_JOURNAL_BYTES
}

fn open_connection(path: &Path) -> Result<Connection, StoreFailure> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_FULL_MUTEX;
    Connection::open_with_flags(path, flags).map_err(map_open)
}

fn configure_connection(conn: &Connection) -> Result<(), StoreFailure> {
    conn.busy_timeout(Duration::from_millis(1000))
        .map_err(|_| StoreFailure::Unavailable)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, LENGTH_LIMIT)
        .map_err(|_| StoreFailure::Unavailable)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_SQL_LENGTH, SQL_LIMIT)
        .map_err(|_| StoreFailure::Unavailable)?;
    conn.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_ATTACHED, 0)
        .map_err(|_| StoreFailure::Unavailable)?;
    set_config(
        conn,
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE,
        true,
    )?;
    set_config(
        conn,
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA,
        false,
    )?;
    // SQLite starts with sqlite3_load_extension disabled. This adapter never
    // enables it. rusqlite 0.39 does not expose that dbconfig flag.
    conn.pragma_update(None, "trusted_schema", "OFF")
        .map_err(|_| StoreFailure::Unavailable)?;
    Ok(())
}

fn set_config(
    conn: &Connection,
    config: rusqlite::config::DbConfig,
    value: bool,
) -> Result<(), StoreFailure> {
    conn.set_db_config(config, value)
        .map_err(|_| StoreFailure::Unavailable)?;
    Ok(())
}

fn set_page_size(conn: &Connection) -> Result<(), StoreFailure> {
    conn.pragma_update(None, "page_size", PAGE_SIZE)
        .map_err(|_| StoreFailure::Unavailable)?;
    Ok(())
}

fn apply_durability(conn: &Connection) -> Result<(), StoreFailure> {
    conn.pragma_update(None, "journal_mode", "DELETE")
        .map_err(|_| StoreFailure::Unavailable)?;
    apply_connection_durability(conn)
}

fn apply_connection_durability(conn: &Connection) -> Result<(), StoreFailure> {
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|_| StoreFailure::Unavailable)?;
    conn.pragma_update(None, "temp_store", "MEMORY")
        .map_err(|_| StoreFailure::Unavailable)?;
    conn.pragma_update(None, "max_page_count", MAX_PAGE_COUNT)
        .map_err(|_| StoreFailure::Unavailable)?;
    #[cfg(target_os = "macos")]
    {
        conn.pragma_update(None, "fullfsync", "ON")
            .map_err(|_| StoreFailure::Unavailable)?;
    }
    Ok(())
}

fn verify_durability(conn: &Connection) -> Result<(), StoreFailure> {
    let journal: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|_| StoreFailure::Unavailable)?;
    if !journal.eq_ignore_ascii_case("delete") {
        return Err(StoreFailure::Corrupt);
    }
    let synchronous: i64 = conn
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .map_err(|_| StoreFailure::Unavailable)?;
    if synchronous != 2 {
        return Err(StoreFailure::Unavailable);
    }
    let temp_store: i64 = conn
        .pragma_query_value(None, "temp_store", |row| row.get(0))
        .map_err(|_| StoreFailure::Unavailable)?;
    if temp_store != 2 {
        return Err(StoreFailure::Unavailable);
    }
    let page_size: i64 = conn
        .pragma_query_value(None, "page_size", |row| row.get(0))
        .map_err(|_| StoreFailure::Unavailable)?;
    if page_size != PAGE_SIZE {
        return Err(StoreFailure::Corrupt);
    }
    let max_pages: i64 = conn
        .pragma_query_value(None, "max_page_count", |row| row.get(0))
        .map_err(|_| StoreFailure::Unavailable)?;
    if max_pages != MAX_PAGE_COUNT {
        return Err(StoreFailure::Unavailable);
    }
    #[cfg(target_os = "macos")]
    {
        let fullfsync: i64 = conn
            .pragma_query_value(None, "fullfsync", |row| row.get(0))
            .map_err(|_| StoreFailure::Unavailable)?;
        if fullfsync != 1 {
            return Err(StoreFailure::Unavailable);
        }
    }
    // Pragma readback does not prove the disk honored the flush. The VFS name
    // is the implementation that issues `F_FULLFSYNC` (macOS `unix`), `fsync`
    // (Linux `unix`), or `FlushFileBuffers` (Windows `win32`).
    verify_vfs(conn)?;
    Ok(())
}

fn required_vfs_name() -> &'static str {
    #[cfg(windows)]
    {
        "win32"
    }
    #[cfg(not(windows))]
    {
        "unix"
    }
}

fn verify_vfs(conn: &Connection) -> Result<(), StoreFailure> {
    let name = selected_vfs_name(conn)?;
    if name != required_vfs_name() {
        return Err(StoreFailure::Unavailable);
    }
    Ok(())
}

fn selected_vfs_name(conn: &Connection) -> Result<String, StoreFailure> {
    let mut vfs: *mut rusqlite::ffi::sqlite3_vfs = std::ptr::null_mut();
    let schema = std::ffi::CString::new("main").map_err(|_| StoreFailure::Unavailable)?;
    // Safety: `handle` is the connection this adapter opened and still owns.
    // `SQLITE_FCNTL_VFS_POINTER` writes one `sqlite3_vfs*` into `vfs`. SQLite
    // owns that object for the life of the connection. This function reads
    // `zName` and does not free it.
    let code = unsafe {
        rusqlite::ffi::sqlite3_file_control(
            conn.handle(),
            schema.as_ptr(),
            rusqlite::ffi::SQLITE_FCNTL_VFS_POINTER,
            (&mut vfs as *mut *mut rusqlite::ffi::sqlite3_vfs).cast(),
        )
    };
    if code != rusqlite::ffi::SQLITE_OK || vfs.is_null() {
        return Err(StoreFailure::Unavailable);
    }
    // Safety: `vfs` is the non-null pointer written above. `zName` is a
    // NUL-terminated string owned by SQLite.
    let name_ptr = unsafe { (*vfs).zName };
    if name_ptr.is_null() {
        return Err(StoreFailure::Unavailable);
    }
    let text = unsafe { std::ffi::CStr::from_ptr(name_ptr) };
    text.to_str()
        .map(str::to_string)
        .map_err(|_| StoreFailure::Unavailable)
}

fn verify_existing(conn: &Connection) -> Result<(), StoreFailure> {
    let journal: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(map_read)?;
    if !journal.eq_ignore_ascii_case("delete") {
        return Err(StoreFailure::Corrupt);
    }
    verify_schema(conn)
}

fn verify_schema(conn: &Connection) -> Result<(), StoreFailure> {
    let application_id: i64 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .map_err(|_| StoreFailure::Corrupt)?;
    if application_id != i64::from(APPLICATION_ID) {
        return Err(StoreFailure::Corrupt);
    }
    let mut statement = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master \
             WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
        )
        .map_err(|_| StoreFailure::Corrupt)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|_| StoreFailure::Corrupt)?;
    let mut found = false;
    for row in rows {
        let (kind, name, sql) = row.map_err(|_| StoreFailure::Corrupt)?;
        if found || kind != "table" || name != "installation" {
            return Err(StoreFailure::Corrupt);
        }
        if sql.as_deref() != Some(normalize_sql(CREATE_SQL)) && sql.as_deref() != Some(CREATE_SQL) {
            #[cfg(test)]
            eprintln!("stored installation SQL: {sql:?}");
            return Err(StoreFailure::Corrupt);
        }
        found = true;
    }
    if !found {
        return Err(StoreFailure::Corrupt);
    }
    Ok(())
}

fn normalize_sql(sql: &str) -> &str {
    sql
}

fn map_open(error: rusqlite::Error) -> StoreFailure {
    if is_corrupt(&error) {
        StoreFailure::Corrupt
    } else {
        StoreFailure::Unavailable
    }
}

fn map_read(error: rusqlite::Error) -> StoreFailure {
    if is_corrupt(&error) {
        StoreFailure::Corrupt
    } else {
        StoreFailure::Unavailable
    }
}

fn is_corrupt(error: &rusqlite::Error) -> bool {
    matches!(sqlite_code(error) & 0xff, 11 | 26)
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(sqlite_code(error) & 0xff, 5 | 6)
}

fn sqlite_code(error: &rusqlite::Error) -> i32 {
    match error {
        rusqlite::Error::SqliteFailure(code, _) => code.extended_code,
        _ => 0,
    }
}

#[cfg(test)]
fn wait_for_barrier(dir: &Path) {
    let _ = std::fs::write(dir.join("after-write"), b"1");
    let continue_at = dir.join("continue");
    let started = std::time::Instant::now();
    while !continue_at.exists() {
        if started.elapsed() > Duration::from_secs(10) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn sync_created(database: &Path, engine_dir: &Path) -> Result<(), StoreFailure> {
    paths::sync_file(database)?;
    paths::sync_dir(engine_dir)
}

/// A schema transaction that never committed. `Unavailable` only when the
/// connection is back in autocommit, so the caller knows no schema row exists.
fn abandon_uncommitted(conn: &Connection) -> StoreFailure {
    if rollback_ended(conn) {
        StoreFailure::Unavailable
    } else {
        StoreFailure::RecoveryRequired
    }
}

fn rollback_ended(conn: &Connection) -> bool {
    if conn.is_autocommit() {
        return true;
    }
    conn.execute_batch("ROLLBACK").is_ok() && conn.is_autocommit()
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaFault {
    /// The schema statement fails and the transaction ends with no commit.
    Statement,
    /// `COMMIT` is rejected. The failure stays fatal if rollback looks successful.
    Commit,
    /// The schema commit finished and the following directory sync is reported failed.
    DirectorySync,
}

#[cfg(test)]
thread_local! {
    static SCHEMA_FAULT: Cell<Option<SchemaFault>> = const { Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn arm_schema_fault(fault: SchemaFault) {
    SCHEMA_FAULT.with(|cell| cell.set(Some(fault)));
}

#[cfg(test)]
pub(crate) fn clear_schema_fault() {
    SCHEMA_FAULT.with(|cell| cell.set(None));
}

#[cfg(test)]
pub(crate) fn take_schema_fault(expected: SchemaFault) -> bool {
    SCHEMA_FAULT.with(|cell| {
        if cell.get() == Some(expected) {
            cell.set(None);
            true
        } else {
            false
        }
    })
}

#[cfg(test)]
pub(crate) fn expected_vfs_name() -> &'static str {
    required_vfs_name()
}

#[cfg(test)]
pub(crate) fn bundled_sqlite_version() -> &'static str {
    rusqlite::version()
}

const _: usize = crypto::MAX_PLAINTEXT;
