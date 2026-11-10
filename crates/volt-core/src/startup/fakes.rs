//! In-memory fake of the [`FilePublisher`] port.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use super::FilePublisher;
use crate::error::PortError;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A [`FilePublisher`] whose answers are set by the test. A program that was
/// not set has no publisher.
#[derive(Debug, Default)]
pub struct FakePublisher {
    answers: Mutex<HashMap<String, Result<Option<String>, PortError>>>,
    asked: Mutex<Vec<String>>,
}

impl FakePublisher {
    /// Knows no publishers.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Says who published `executable` (compared ignoring case).
    pub fn set(&self, executable: &str, publisher: &str) {
        lock(&self.answers).insert(executable.to_lowercase(), Ok(Some(publisher.to_owned())));
    }

    /// Makes asking about `executable` (compared ignoring case) fail.
    pub fn fail_on(&self, executable: &str, error: PortError) {
        lock(&self.answers).insert(executable.to_lowercase(), Err(error));
    }

    /// The programs asked about so far, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<String> {
        lock(&self.asked).clone()
    }
}

impl FilePublisher for FakePublisher {
    fn publisher(&self, executable: &str) -> Result<Option<String>, PortError> {
        lock(&self.asked).push(executable.to_owned());
        lock(&self.answers)
            .get(&executable.to_lowercase())
            .cloned()
            .unwrap_or(Ok(None))
    }
}
