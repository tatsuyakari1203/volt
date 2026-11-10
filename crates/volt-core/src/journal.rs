//! The undo journal: model and port.
//!
//! The journal is what makes undo reliable. An entry records the prior state
//! *before* the system is touched, so revert works after a crash or restart.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::operation::{Operation, PriorState};
use crate::time::Timestamp;

/// Identifier of a journal entry, assigned by the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EntryId(pub u64);

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Identifier of a batch: the entries written by one applied plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BatchId(pub u64);

impl fmt::Display for BatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "batch {}", self.0)
    }
}

/// Who started a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Source {
    /// The user, through the UI or CLI.
    Manual,
    /// A schedule, threshold or profile.
    Automation,
}

/// Lifecycle of a journal entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntryState {
    /// Written before the change; the change may or may not have happened.
    Pending,
    /// The change was made.
    Applied,
    /// The change was made and later undone.
    Reverted,
    /// The change was attempted and failed.
    Failed,
}

/// What the executor hands to the journal before mutating anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewEntry {
    /// Batch this entry belongs to.
    pub batch: BatchId,
    /// When the entry was written.
    pub timestamp: Timestamp,
    /// Who started the change.
    pub source: Source,
    /// The change about to be made.
    pub operation: Operation,
    /// The state that was detected just before; revert restores exactly this.
    pub prior: PriorState,
}

/// A stored journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Entry id.
    pub id: EntryId,
    /// Batch this entry belongs to.
    pub batch: BatchId,
    /// When the entry was written.
    pub timestamp: Timestamp,
    /// Who started the change.
    pub source: Source,
    /// Current lifecycle state.
    pub state: EntryState,
    /// The change.
    pub operation: Operation,
    /// The state before the change.
    pub prior: PriorState,
    /// Failure message when the state is [`EntryState::Failed`].
    pub error: Option<String>,
}

/// Port: the persistent undo journal.
pub trait Journal: Send + Sync {
    /// Allocates a new batch id.
    fn new_batch(&self) -> Result<BatchId, PortError>;

    /// Stores a new entry in state [`EntryState::Pending`] and returns its id.
    ///
    /// Must be durable when it returns: the executor mutates the system only
    /// after this call succeeds. Ids increase with write order.
    fn record_pending(&self, entry: NewEntry) -> Result<EntryId, PortError>;

    /// Moves an entry to `state`, with an optional failure message.
    /// Unknown id is [`PortError::NotFound`].
    fn set_state(
        &self,
        id: EntryId,
        state: EntryState,
        error: Option<String>,
    ) -> Result<(), PortError>;

    /// Reads one entry. Unknown id is `Ok(None)`.
    fn entry(&self, id: EntryId) -> Result<Option<JournalEntry>, PortError>;

    /// All entries of a batch, ordered by ascending id.
    fn batch_entries(&self, batch: BatchId) -> Result<Vec<JournalEntry>, PortError>;

    /// All entries currently in `state`, ordered by ascending id. Used to find
    /// entries a crash left [`EntryState::Pending`].
    fn entries_in_state(&self, state: EntryState) -> Result<Vec<JournalEntry>, PortError>;
}
