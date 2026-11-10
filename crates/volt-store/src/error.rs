//! Errors of opening the store.

use std::path::PathBuf;

use thiserror::Error;

/// Why a [`Store`](crate::Store) could not be opened.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The database could not be opened for a reason other than corruption
    /// (permissions, locked, missing disk). Nothing was moved or changed.
    #[error("cannot open database {path}: {message}")]
    Open {
        /// The database file.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
    /// The database was written by a newer version of Volt. It is left
    /// untouched.
    #[error("database schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew {
        /// Version found in the file.
        found: u32,
        /// Highest version this build understands.
        supported: u32,
    },
    /// The database is corrupt and could not be moved aside, so it was left
    /// in place and no fresh database was started.
    #[error("database {path} is corrupt and could not be backed up: {message}")]
    Backup {
        /// The corrupt database file.
        path: PathBuf,
        /// What went wrong while moving it.
        message: String,
    },
}
