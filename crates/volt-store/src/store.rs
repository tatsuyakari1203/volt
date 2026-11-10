//! The `SQLite`-backed [`Store`]: opening, durability settings and recovery.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, ErrorCode};
use volt_core::{Clock, PortError, SystemClock};

use crate::error::StoreError;
use crate::migrations::{MIGRATIONS, MigrateError, migrate};

/// The result of opening a database that was corrupt and had to be replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CorruptionRecovery {
    /// Where the corrupt database was moved. It is never deleted.
    pub backup_path: PathBuf,
    /// What the corruption check reported.
    pub reason: String,
}

/// A freshly opened store, and whether a corrupt database was set aside.
#[derive(Debug)]
pub struct OpenOutcome {
    /// The open store.
    pub store: Store,
    /// `Some` when the existing database was corrupt: it was moved to
    /// [`CorruptionRecovery::backup_path`] and `store` is a fresh database.
    /// The caller must tell the user.
    pub recovery: Option<CorruptionRecovery>,
}

/// One `SQLite` database holding the undo journal and the settings.
///
/// Implements [`Journal`](volt_core::Journal) and
/// [`Settings`](volt_core::Settings). Share it with `Arc`.
pub struct Store {
    conn: Mutex<Connection>,
    pub(crate) clock: Arc<dyn Clock>,
}

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

/// Why one attempt to open and verify the file failed.
enum Failure {
    Corrupt(String),
    Fatal(StoreError),
}

impl Store {
    /// Opens (creating if needed) the database at `path`, migrating it to the
    /// current schema. The parent directory is created if missing; where the
    /// file lives is the caller's decision.
    ///
    /// A corrupt database is moved aside and replaced; see
    /// [`OpenOutcome::recovery`].
    pub fn open(path: &Path) -> Result<OpenOutcome, StoreError> {
        Self::open_with_clock(path, Arc::new(SystemClock))
    }

    /// Like [`Store::open`] with an injected clock (batch and backup times).
    pub fn open_with_clock(path: &Path, clock: Arc<dyn Clock>) -> Result<OpenOutcome, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| open_error(path, &e))?;
        }
        match try_open(path) {
            Ok(conn) => Ok(OpenOutcome {
                store: Self::from_connection(conn, clock),
                recovery: None,
            }),
            Err(Failure::Fatal(error)) => Err(error),
            Err(Failure::Corrupt(reason)) => {
                let backup_path = move_aside(path, &clock)?;
                log::warn!(
                    "the database was corrupt ({reason}); moved it to {} and started a new one",
                    backup_path.display()
                );
                match try_open(path) {
                    Ok(conn) => Ok(OpenOutcome {
                        store: Self::from_connection(conn, clock),
                        recovery: Some(CorruptionRecovery {
                            backup_path,
                            reason,
                        }),
                    }),
                    Err(Failure::Fatal(error)) => Err(error),
                    Err(Failure::Corrupt(reason)) => Err(StoreError::Open {
                        path: path.to_path_buf(),
                        message: format!("a fresh database is also unusable: {reason}"),
                    }),
                }
            }
        }
    }

    fn from_connection(conn: Connection, clock: Arc<dyn Clock>) -> Self {
        Self {
            conn: Mutex::new(conn),
            clock,
        }
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Opens, configures, verifies and migrates the database file.
fn try_open(path: &Path) -> Result<Connection, Failure> {
    let mut conn = Connection::open(path).map_err(|e| classify(path, &e))?;
    configure(&conn).map_err(|e| classify(path, &e))?;
    let verdict: String = conn
        .pragma_query_value(None, "quick_check", |row| row.get(0))
        .map_err(|e| classify(path, &e))?;
    if verdict != "ok" {
        return Err(Failure::Corrupt(format!(
            "integrity check failed: {verdict}"
        )));
    }
    match migrate(&mut conn, MIGRATIONS) {
        Ok(()) => Ok(conn),
        Err(MigrateError::TooNew { found, supported }) => {
            Err(Failure::Fatal(StoreError::SchemaTooNew {
                found,
                supported,
            }))
        }
        Err(MigrateError::Sql(e)) => Err(classify(path, &e)),
    }
}

/// WAL plus `synchronous = FULL`: a committed write is fsynced to disk before
/// the statement returns. This is what makes `record_pending` durable.
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn classify(path: &Path, error: &rusqlite::Error) -> Failure {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => {
            Failure::Corrupt(error.to_string())
        }
        _ => Failure::Fatal(StoreError::Open {
            path: path.to_path_buf(),
            message: error.to_string(),
        }),
    }
}

fn open_error(path: &Path, error: &std::io::Error) -> StoreError {
    StoreError::Open {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

/// Moves the database and its `-wal` / `-shm` files to a timestamped backup
/// name next to it. Returns the backup path of the main file. Never deletes.
fn move_aside(path: &Path, clock: &Arc<dyn Clock>) -> Result<PathBuf, StoreError> {
    let backup_error = |e: &std::io::Error| StoreError::Backup {
        path: path.to_path_buf(),
        message: e.to_string(),
    };
    let millis = clock.now().as_unix_millis();
    let file_name = path
        .file_name()
        .map_or_else(|| OsString::from("volt.db"), std::ffi::OsStr::to_os_string);
    let mut attempt = 0_u32;
    let backup = loop {
        let mut name = file_name.clone();
        name.push(format!(".corrupt-{millis}"));
        if attempt > 0 {
            name.push(format!("-{attempt}"));
        }
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            break candidate;
        }
        attempt += 1;
    };
    fs::rename(path, &backup).map_err(|e| backup_error(&e))?;
    for suffix in ["-wal", "-shm"] {
        let mut from = path.as_os_str().to_os_string();
        from.push(suffix);
        let from = PathBuf::from(from);
        if from.exists() {
            let mut to = backup.as_os_str().to_os_string();
            to.push(suffix);
            fs::rename(&from, PathBuf::from(to)).map_err(|e| backup_error(&e))?;
        }
    }
    Ok(backup)
}

/// Maps a database failure to a port error.
pub(crate) fn db_error(error: &rusqlite::Error) -> PortError {
    PortError::Other(format!("database: {error}"))
}
