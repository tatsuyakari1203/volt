//! In-memory fake of the [`KnownFolders`] port.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::bin::{RecycleBin, RecycleBinStatus};
use super::folders::{KnownFolders, Location};
use crate::error::PortError;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// In-memory [`KnownFolders`]: locations are set by the test; a location that
/// was not set is [`PortError::NotFound`].
#[derive(Debug, Default)]
pub struct FakeKnownFolders {
    locations: Mutex<HashMap<Location, PathBuf>>,
    protected: Mutex<Vec<PathBuf>>,
    protected_failure: Mutex<Option<PortError>>,
    system: Mutex<Option<Vec<PathBuf>>>,
    env: Mutex<HashMap<String, PathBuf>>,
}

impl FakeKnownFolders {
    /// No locations, nothing protected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets where a location is.
    pub fn set(&self, location: Location, path: impl Into<PathBuf>) {
        lock(&self.locations).insert(location, path.into());
    }

    /// Adds a protected folder.
    pub fn protect(&self, path: impl Into<PathBuf>) {
        lock(&self.protected).push(path.into());
    }

    /// Sets the folders `system_folders` returns (unset: it fails).
    pub fn set_system_folders(&self, folders: Vec<PathBuf>) {
        *lock(&self.system) = Some(folders);
    }

    /// Sets an environment variable (what `env_folder` returns for it).
    pub fn set_env(&self, variable: &str, path: impl Into<PathBuf>) {
        lock(&self.env).insert(variable.to_owned(), path.into());
    }

    /// Makes `protected_folders` fail, or stop failing with `None`.
    pub fn fail_protected_with(&self, error: Option<PortError>) {
        *lock(&self.protected_failure) = error;
    }
}

impl KnownFolders for FakeKnownFolders {
    fn location(&self, location: Location) -> Result<PathBuf, PortError> {
        lock(&self.locations)
            .get(&location)
            .cloned()
            .ok_or_else(|| PortError::NotFound(format!("location {location}")))
    }

    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        if let Some(error) = lock(&self.protected_failure).clone() {
            return Err(error);
        }
        Ok(lock(&self.protected).clone())
    }

    fn env_folder(&self, variable: &str) -> Option<PathBuf> {
        lock(&self.env).get(variable).cloned()
    }

    fn system_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        lock(&self.system)
            .clone()
            .ok_or_else(|| PortError::Unsupported("system folders".to_owned()))
    }
}

#[derive(Debug, Default)]
struct BinState {
    status: RecycleBinStatus,
    status_failure: Option<PortError>,
    empty_failure: Option<PortError>,
    empty_calls: usize,
}

/// In-memory [`RecycleBin`]: holds what the test says and counts how often it
/// was emptied. Emptying it only resets the fake's own numbers.
#[derive(Debug, Default)]
pub struct FakeRecycleBin {
    state: Mutex<BinState>,
}

impl FakeRecycleBin {
    /// An empty bin.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A bin holding `item_count` items of `size_bytes` in all.
    #[must_use]
    pub fn holding(item_count: u64, size_bytes: u64) -> Self {
        let bin = Self::default();
        bin.set(item_count, size_bytes);
        bin
    }

    /// Sets what the bin holds.
    pub fn set(&self, item_count: u64, size_bytes: u64) {
        lock(&self.state).status = RecycleBinStatus {
            item_count,
            size_bytes,
        };
    }

    /// Makes `status` fail, or stop failing with `None`.
    pub fn fail_status_with(&self, error: Option<PortError>) {
        lock(&self.state).status_failure = error;
    }

    /// Makes `empty` fail, or stop failing with `None`.
    pub fn fail_empty_with(&self, error: Option<PortError>) {
        lock(&self.state).empty_failure = error;
    }

    /// How many times `empty` was called.
    #[must_use]
    pub fn empty_calls(&self) -> usize {
        lock(&self.state).empty_calls
    }
}

impl RecycleBin for FakeRecycleBin {
    fn status(&self) -> Result<RecycleBinStatus, PortError> {
        let state = lock(&self.state);
        match &state.status_failure {
            Some(error) => Err(error.clone()),
            None => Ok(state.status),
        }
    }

    fn empty(&self) -> Result<(), PortError> {
        let mut state = lock(&self.state);
        state.empty_calls += 1;
        if let Some(error) = state.empty_failure.clone() {
            return Err(error);
        }
        state.status = RecycleBinStatus::default();
        Ok(())
    }
}
