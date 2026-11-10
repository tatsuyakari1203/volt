//! Injectable time, so journal timestamps are deterministic in tests.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// A point in time as milliseconds since the Unix epoch (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Builds a timestamp from Unix milliseconds.
    #[must_use]
    pub const fn from_unix_millis(millis: i64) -> Self {
        Self(millis)
    }

    /// Milliseconds since the Unix epoch.
    #[must_use]
    pub const fn as_unix_millis(self) -> i64 {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// Source of the current time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> Timestamp;
}

/// [`Clock`] backed by the system clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|d| i64::try_from(d.as_millis()).ok())
            .unwrap_or(0);
        Timestamp(millis)
    }
}
