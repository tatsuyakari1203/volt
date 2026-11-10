//! The Recycle Bin: a read-only query of what it holds, and the one permanent
//! action Volt has, emptying it.
//!
//! Cleaning only ever moves files into the Recycle Bin, where they keep using
//! disk space. Emptying it is the step that frees space, and it cannot be
//! undone, so it is its own port and its own use case, never part of a clean.

use std::sync::Arc;

use super::apply::human_bytes;
use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::time::Clock;

/// What the Recycle Bin holds, over every drive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecycleBinStatus {
    /// Number of items (a folder counts as one).
    pub item_count: u64,
    /// Total size in bytes.
    pub size_bytes: u64,
}

/// Port: the Recycle Bin. Not part of the [`Ports`](crate::Ports) bundle, like
/// the other cleaning ports.
pub trait RecycleBin: Send + Sync {
    /// How many items the Recycle Bin holds and how big they are. Read-only.
    fn status(&self) -> Result<RecycleBinStatus, PortError>;

    /// Permanently deletes everything in the Recycle Bin, on every drive,
    /// without asking again. There is no way back.
    fn empty(&self) -> Result<(), PortError>;
}

/// What emptying the Recycle Bin did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EmptyReport {
    /// What the Recycle Bin held just before it was emptied.
    pub before: RecycleBinStatus,
    /// Whether the bin was emptied. `false` when it was already empty: nothing
    /// was called and nothing was recorded.
    pub emptied: bool,
    /// Whether the activity record could not be written (the emptying itself is
    /// never affected).
    pub activity_write_failed: bool,
}

/// Reads and empties the Recycle Bin, and records the emptying in the activity
/// log as what it is: permanent.
pub struct RecycleBinManager {
    bin: Arc<dyn RecycleBin>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
}

impl std::fmt::Debug for RecycleBinManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecycleBinManager")
            .field("activity_log", &self.activity.is_some())
            .finish_non_exhaustive()
    }
}

impl RecycleBinManager {
    /// A manager over `bin`. The clock stamps the activity record.
    #[must_use]
    pub fn new(bin: Arc<dyn RecycleBin>, clock: Arc<dyn Clock>) -> Self {
        Self {
            bin,
            clock,
            activity: None,
        }
    }

    /// Also writes an activity record for every emptying.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// What the Recycle Bin holds now. Read-only.
    pub fn status(&self) -> Result<RecycleBinStatus, PortError> {
        self.bin.status()
    }

    /// Permanently empties the Recycle Bin.
    ///
    /// The bin is read first: when it is already empty nothing is called. The
    /// record names counts and sizes only and says it cannot be undone; it is
    /// written after the call, as `Applied` when the bin was emptied and
    /// `Failed` when it was not. A failing log never affects the emptying.
    ///
    /// # Errors
    /// The port's error when the bin cannot be read or emptied.
    pub fn empty(&self, source: Source) -> Result<EmptyReport, PortError> {
        let before = self.bin.status()?;
        if before.item_count == 0 {
            return Ok(EmptyReport {
                before,
                emptied: false,
                activity_write_failed: false,
            });
        }
        let result = self.bin.empty();
        let (outcome, summary) = match &result {
            Ok(()) => (
                ActivityOutcome::Applied,
                format!(
                    "Emptied the Recycle Bin: {} item{} ({}) permanently deleted. This cannot be undone.",
                    before.item_count,
                    if before.item_count == 1 { "" } else { "s" },
                    human_bytes(before.size_bytes),
                ),
            ),
            Err(error) => (
                ActivityOutcome::Failed,
                format!(
                    "Could not empty the Recycle Bin ({}). Items may already have been deleted permanently; that cannot be undone.",
                    error_class(error)
                ),
            ),
        };
        let activity_write_failed = self.record(source, outcome, summary);
        log::info!(
            "Recycle Bin emptied: {}",
            if result.is_ok() { "done" } else { "failed" }
        );
        result.map(|()| EmptyReport {
            before,
            emptied: true,
            activity_write_failed,
        })
    }

    fn record(&self, source: Source, outcome: ActivityOutcome, summary: String) -> bool {
        let Some(log) = &self.activity else {
            return false;
        };
        let activity = NewActivity {
            timestamp: self.clock.now(),
            source,
            outcome,
            summary,
            batch: None,
            entry: None,
            label: Some("recycle-bin".to_owned()),
        };
        match log.record(activity) {
            Ok(_) => false,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for emptying the Recycle Bin ({})",
                    error_class(&error)
                );
                true
            }
        }
    }
}
