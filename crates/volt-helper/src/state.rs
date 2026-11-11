//! The helper's own state: the protected folder, the single-helper lock and the store.
//!
//! The helper keeps its journal, activity log and settings in a `SQLite` file of its own inside
//! `<install folder>\state` (see the decision on the helper). The folder is created by the helper
//! with an ACL that only SYSTEM and Administrators can write, and is refused when anyone else can;
//! one helper at a time holds the lock file in it; and a user's rows live in a folder of their own
//! named by their SID, so a history is served to that user only. The UI never opens this store and
//! the helper never opens the UI's.

use std::path::PathBuf;
use std::sync::Arc;

use volt_store::{CorruptionRecovery, Store};
use volt_win::helper::install::InstallLayout;
use volt_win::helper::state::{LockError, StateError, StateLock, prepare_state_dir, user_dir};

/// The helper's database inside the user's state folder.
pub const STORE_FILE_NAME: &str = "helper.db";

/// Why the state could not be opened.
#[derive(Debug)]
pub enum StateFailure {
    /// Another helper holds the lock.
    Locked,
    /// The folder is refused or cannot be made, or the lock file cannot be opened.
    Folder(String),
    /// The store cannot be opened.
    Store(String),
}

impl std::fmt::Display for StateFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => f.write_str("another helper is running"),
            Self::Folder(reason) => f.write_str(reason),
            Self::Store(reason) => write!(f, "the helper's store cannot be opened: {reason}"),
        }
    }
}

impl std::error::Error for StateFailure {}

/// The open state of one helper process. Holds the lock until it is dropped.
#[derive(Debug)]
pub struct HelperState {
    _lock: StateLock,
    store: Arc<Store>,
    store_id: String,
    dir: PathBuf,
    recovery: Option<CorruptionRecovery>,
}

impl HelperState {
    /// Makes sure the state folder is sound, takes the single-helper lock and opens the store of
    /// `user_sid`.
    ///
    /// # Errors
    /// [`StateFailure::Locked`] when another helper runs, otherwise the reason it cannot be used.
    pub fn open(layout: &InstallLayout, user_sid: &str) -> Result<Self, StateFailure> {
        prepare_state_dir(layout)
            .map_err(|error: StateError| StateFailure::Folder(error.to_string()))?;
        let lock = StateLock::acquire(&layout.state_dir).map_err(|error| match error {
            LockError::Held => StateFailure::Locked,
            LockError::Io(reason) => StateFailure::Folder(reason),
        })?;
        let dir =
            user_dir(layout, user_sid).map_err(|error| StateFailure::Folder(error.to_string()))?;
        let opened = Store::open(&dir.join(STORE_FILE_NAME))
            .map_err(|error| StateFailure::Store(error.to_string()))?;
        let store = Arc::new(opened.store);
        let store_id = store
            .store_id()
            .map_err(|error| StateFailure::Store(error.to_string()))?;
        Ok(Self {
            _lock: lock,
            store,
            store_id,
            dir,
            recovery: opened.recovery,
        })
    }

    /// The store: the helper's journal, activity log and settings.
    #[must_use]
    pub fn store(&self) -> Arc<Store> {
        Arc::clone(&self.store)
    }

    /// The identifier of the store, reported with the history.
    #[must_use]
    pub fn store_id(&self) -> &str {
        &self.store_id
    }

    /// The user's folder inside the state folder.
    #[must_use]
    pub fn dir(&self) -> &PathBuf {
        &self.dir
    }

    /// Set when the database was corrupt and was moved aside: the journal that held the previous
    /// values is then gone from the helper's point of view (the old file is kept).
    #[must_use]
    pub fn recovery(&self) -> Option<&CorruptionRecovery> {
        self.recovery.as_ref()
    }
}
