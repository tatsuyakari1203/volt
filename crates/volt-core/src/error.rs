//! Typed errors. Nothing in the core panics; failures are values that cross
//! the IPC boundary so the UI can say what failed and what was changed.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::journal::EntryId;
use crate::journal::EntryState;

/// Failure reported by a port implementation (registry, services, journal, ...).
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PortError {
    /// The addressed object (value, service, file, entry) does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// The caller lacks the rights for the request (for example not elevated).
    #[error("access denied: {0}")]
    AccessDenied(String),
    /// The request is valid but this platform or adapter cannot do it.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The request itself is malformed (bad key path, mismatched prior state).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Any other failure, with a human-readable message.
    #[error("{0}")]
    Other(String),
}

/// Failure while building a plan. Building only reads the system.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PlanError {
    /// The current state of the operation at `index` could not be read.
    #[error("cannot read current state for operation {index}: {error}")]
    Detect {
        /// Zero-based position of the operation in the request list.
        index: usize,
        /// The underlying port failure.
        error: PortError,
    },
}

/// Failure of one operation while applying a plan, or of the whole batch.
///
/// [`ExecError::system_changed`] tells whether the system may have been
/// modified before the failure.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExecError {
    /// The prior state could not be read. Nothing was journaled or changed.
    #[error("could not read prior state: {0}")]
    Detect(PortError),
    /// The journal refused the pending entry (or a new batch). Nothing was
    /// changed: the write-ahead rule forbids mutating without a journal entry.
    #[error("could not write the undo journal: {0}")]
    Journal(PortError),
    /// The port failed to apply the change. The entry was marked failed. Port
    /// operations are single and atomic, so the system is treated as unchanged.
    #[error("apply failed: {error}")]
    Apply {
        /// The port failure.
        error: PortError,
        /// Set when the entry could not be marked failed either; it then stays
        /// pending in the journal.
        journal: Option<PortError>,
    },
    /// The change was applied but the journal entry could not be marked
    /// applied. The system IS changed; the entry stays pending.
    #[error("applied, but the journal could not be updated: {0}")]
    MarkApplied(PortError),
}

impl ExecError {
    /// Whether the system may have been changed by the failed operation.
    #[must_use]
    pub fn system_changed(&self) -> bool {
        matches!(self, Self::MarkApplied(_))
    }
}

/// Failure while reverting a journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RevertError {
    /// The journal could not be read or updated before any change was made.
    #[error("journal error: {0}")]
    Journal(PortError),
    /// No entry with this id exists.
    #[error("journal entry {0} not found")]
    EntryNotFound(EntryId),
    /// The entry is not in a state that can be reverted (only applied entries can).
    #[error("entry is {0:?} and cannot be reverted")]
    NotRevertible(EntryState),
    /// The port failed to restore the prior state. The entry stays applied and
    /// can be reverted again.
    #[error("revert failed: {0}")]
    Port(PortError),
    /// The prior state was restored but the entry could not be marked reverted.
    /// The system IS restored.
    #[error("reverted, but the journal could not be updated: {0}")]
    MarkReverted(PortError),
}
