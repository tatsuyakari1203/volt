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

/// Batches can carry a label (the tweak engine stores the tweak id), so a
/// tweak's batches can be found again after a restart. Existing batches stay
/// unlabelled.
const V2: &str = "
ALTER TABLE batches ADD COLUMN label TEXT;
CREATE INDEX batches_label ON batches (label);
";

/// The activity log: a history for people, written by the executor (and
/// later by cleaning and memory purges). It deliberately has no foreign key to
/// the journal: a record can outlive its journal entry and can describe events
/// that have none (`batch`, `entry` and `label` are optional links). `outcome`
/// and `source` hold the enum names, like the journal's `state` and `source`.
const V3: &str = "
CREATE TABLE activity (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp_ms INTEGER NOT NULL,
    source       TEXT NOT NULL,
    outcome      TEXT NOT NULL,
    summary      TEXT NOT NULL,
    batch        INTEGER,
    entry        INTEGER,
    label        TEXT
);
";

/// Every migration this build knows, in order, with consecutive versions.
pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: V1,
    },
    Migration {
        version: 2,
        sql: V2,
    },
    Migration {
        version: 3,
        sql: V3,
    },
];

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
        log::info!(
            "migrating the database schema to version {}",
            migration.version
        );
        let step = (|| {
            let tx = conn.transaction()?;
            tx.execute_batch(migration.sql)?;
            tx.pragma_update(None, "user_version", migration.version)?;
            tx.commit()
        })();
        if let Err(error) = step {
            // Rolled back: the database stays at the previous version.
            log::error!(
                "schema migration to version {} failed and was rolled back: {error}",
                migration.version
            );
            return Err(error.into());
        }
    }
    Ok(())
}
