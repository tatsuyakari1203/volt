//! In-memory fake of the [`KnownFolders`] port.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

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
}
