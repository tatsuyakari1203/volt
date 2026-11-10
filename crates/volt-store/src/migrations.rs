//! Schema versioning through `PRAGMA user_version`.
//!
//! Each migration runs in its own transaction together with the version bump,
//! so a failed migration leaves the database at the previous version.

use rusqlite::Connection;

/// One schema step. `version` is the `user_version` the database has after it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Migration {
    pub(crate) version: u32,
    pub(crate) sql: &'static str,
}

const V1: &str = "
CREATE TABLE batches (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    created_ms INTEGER NOT NULL
);
CREATE TABLE journal_entries (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    batch        INTEGER NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    source       TEXT NOT NULL,
    state        TEXT NOT NULL,
    operation    TEXT NOT NULL,
    prior        TEXT NOT NULL,
    error        TEXT
);
CREATE INDEX journal_entries_batch ON journal_entries (batch);
CREATE INDEX journal_entries_state ON journal_entries (state);
CREATE TABLE settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);
";

/// Every migration this build knows, in order, with consecutive versions.
pub(crate) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: V1,
}];

/// Why migrating failed.
#[derive(Debug)]
pub(crate) enum MigrateError {
    /// The file's version is above the last known migration.
    TooNew {
        found: u32,
        supported: u32,
    },
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for MigrateError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

/// Brings the database up to the last of `migrations`.
pub(crate) fn migrate(conn: &mut Connection, migrations: &[Migration]) -> Result<(), MigrateError> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let supported = migrations.last().map_or(0, |m| m.version);
    if current > supported {
        return Err(MigrateError::TooNew {
            found: current,
            supported,
        });
    }
    for migration in migrations.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
    }
    Ok(())
}
