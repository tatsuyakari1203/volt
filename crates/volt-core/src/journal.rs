//! The undo journal: model and port.
//!
//! The journal is what makes undo reliable. An entry records the prior state
//! *before* the system is touched, so revert works after a crash or restart.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::operation::{Operation, PriorState};
use crate::registry::{RegistryKeyRef, RegistryValueRef};
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

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Manual => "manual",
            Self::Automation => "automation",
        })
    }
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
    /// Allocates a new batch id. A `label` (the tweak engine passes the tweak
    /// id) is stored with the batch and makes it findable again with
    /// [`Journal::batches_with_label`], also after a restart.
    fn new_batch(&self, label: Option<&str>) -> Result<BatchId, PortError>;

    /// Batches created with exactly this `label`, ordered by ascending id.
    fn batches_with_label(&self, label: &str) -> Result<Vec<BatchId>, PortError>;

    /// The label a batch was created with. A batch without a label, and an
    /// unknown batch, are both `Ok(None)`.
    fn batch_label(&self, batch: BatchId) -> Result<Option<String>, PortError>;

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

/// What an operation changes, as far as two operations can be compared: the same registry value
/// or the same service. A command has no comparable target.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Registry(RegistryValueRef),
    Key(RegistryKeyRef),
    Service(String),
}

fn target_of(operation: &Operation) -> Option<Target> {
    match operation {
        Operation::SetRegistryValue { target, .. } | Operation::DeleteRegistryValue { target } => {
            Some(Target::Registry(target.clone()))
        }
        Operation::EnsureRegistryKey { key } | Operation::DeleteRegistryKey { key } => {
            Some(Target::Key(key.clone()))
        }
        Operation::SetServiceStartType { service, .. } => {
            Some(Target::Service(service.to_lowercase()))
        }
        _ => None,
    }
}

/// An undo that would silently replace a later change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overwrite {
    /// The entry the user wants to undo.
    pub entry: EntryId,
    /// The newest later entry, still applied, that changed the same target.
    pub later: JournalEntry,
}

/// For each of `undoing` (applied entries about to be undone together), finds the newest entry
/// that is still applied, was written after it, changed the same registry value or service, and
/// is not itself being undone. Undoing the older entry puts back its prior value and so replaces
/// what that later entry set. Reads the journal only.
///
/// # Errors
/// The journal's error when it cannot be read.
pub fn overwrites_of(
    journal: &dyn Journal,
    undoing: &[JournalEntry],
) -> Result<Vec<Overwrite>, PortError> {
    if undoing.is_empty() {
        return Ok(Vec::new());
    }
    let applied = journal.entries_in_state(EntryState::Applied)?;
    Ok(undoing
        .iter()
        .filter_map(|entry| {
            let target = target_of(&entry.operation)?;
            applied
                .iter()
                .filter(|later| {
                    later.id > entry.id
                        && !undoing.iter().any(|other| other.id == later.id)
                        && target_of(&later.operation).as_ref() == Some(&target)
                })
                .max_by_key(|later| later.id)
                .map(|later| Overwrite {
                    entry: entry.id,
                    later: later.clone(),
                })
        })
        .collect())
}

#[cfg(test)]
mod overwrite_tests {
    use super::*;
    use crate::fakes::FakeJournal;
    use crate::registry::{RegistryHive, RegistryValue};
    use crate::service::ServiceStartType;

    fn set(journal: &FakeJournal, batch: BatchId, name: &str, value: u32) -> EntryId {
        let id = journal
            .record_pending(NewEntry {
                batch,
                timestamp: Timestamp::from_unix_millis(1),
                source: Source::Manual,
                operation: Operation::SetRegistryValue {
                    target: RegistryValueRef::new(
                        RegistryHive::CurrentUser,
                        r"Software\VoltTest",
                        name,
                    ),
                    value: RegistryValue::Dword(value),
                },
                prior: PriorState::RegistryValue(None),
            })
            .unwrap();
        journal.set_state(id, EntryState::Applied, None).unwrap();
        id
    }

    fn undoing(journal: &FakeJournal, ids: &[EntryId]) -> Vec<JournalEntry> {
        ids.iter()
            .map(|id| journal.entry(*id).unwrap().unwrap())
            .collect()
    }

    #[test]
    fn a_later_change_to_the_same_value_is_named() {
        let journal = FakeJournal::new();
        let first = set(&journal, BatchId(1), "A", 1);
        let _other = set(&journal, BatchId(2), "B", 1);
        let second = set(&journal, BatchId(3), "A", 2);
        let third = set(&journal, BatchId(4), "A", 3);
        let found = overwrites_of(&journal, &undoing(&journal, &[first])).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entry, first);
        assert_eq!(found[0].later.id, third);
        // The newest entry has nothing after it.
        assert!(
            overwrites_of(&journal, &undoing(&journal, &[third]))
                .unwrap()
                .is_empty()
        );
        let _ = second;
    }

    #[test]
    fn a_later_change_that_is_undone_or_in_the_same_undo_is_not_a_conflict() {
        let journal = FakeJournal::new();
        let first = set(&journal, BatchId(1), "A", 1);
        let second = set(&journal, BatchId(1), "A", 2);
        let both = undoing(&journal, &[first, second]);
        assert!(overwrites_of(&journal, &both).unwrap().is_empty());
        journal
            .set_state(second, EntryState::Reverted, None)
            .unwrap();
        assert!(
            overwrites_of(&journal, &undoing(&journal, &[first]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn services_match_by_name_ignoring_case() {
        let journal = FakeJournal::new();
        let service = |name: &str| NewEntry {
            batch: BatchId(1),
            timestamp: Timestamp::from_unix_millis(1),
            source: Source::Manual,
            operation: Operation::SetServiceStartType {
                service: name.to_owned(),
                start_type: ServiceStartType::Disabled,
            },
            prior: PriorState::ServiceStartType(ServiceStartType::Automatic),
        };
        let a = journal.record_pending(service("DiagTrack")).unwrap();
        let b = journal.record_pending(service("diagtrack")).unwrap();
        for id in [a, b] {
            journal.set_state(id, EntryState::Applied, None).unwrap();
        }
        let found = overwrites_of(&journal, &undoing(&journal, &[a])).unwrap();
        assert_eq!(found[0].later.id, b);
    }
}
