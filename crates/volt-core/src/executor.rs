//! The executor: the only way to change the system.
//!
//! It enforces write-ahead journaling so callers cannot get it wrong:
//!
//! 1. detect the prior state (read-only),
//! 2. the journal records a pending entry containing that prior state,
//! 3. apply the operation,
//! 4. mark the entry applied (or failed).
//!
//! If step 2 fails, step 3 never runs. Reverting reads the journal, restores
//! the recorded prior state exactly and marks the entry reverted.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{ExecError, PortError, RevertError};
use crate::journal::{BatchId, EntryId, EntryState, Journal, NewEntry, Source};
use crate::operation::{Operation, PriorState};
use crate::plan::{Plan, PlanItem};
use crate::ports::Ports;
use crate::time::Clock;

/// What happened to one plan item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ItemStatus {
    /// Applied and recorded as applied in the journal.
    Applied {
        /// The journal entry holding the prior state.
        entry: EntryId,
    },
    /// Attempted and failed. See [`ExecError::system_changed`].
    Failed {
        /// The journal entry, if one was written before the failure.
        entry: Option<EntryId>,
        /// Why it failed.
        error: ExecError,
    },
    /// Never started because an earlier item failed.
    NotAttempted,
}

/// Outcome for the plan item at `index`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemOutcome {
    /// Zero-based position in the plan.
    pub index: usize,
    /// What happened.
    pub status: ItemStatus,
}

/// Result of reverting one journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertOutcome {
    /// The entry that was reverted or attempted.
    pub entry: EntryId,
    /// `Ok` when the prior state was restored.
    pub result: Result<(), RevertError>,
}

/// Result of reverting several entries, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevertReport {
    /// One outcome per attempted entry, in the order they were attempted.
    pub outcomes: Vec<RevertOutcome>,
}

impl RevertReport {
    /// Whether every attempted entry was restored.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.outcomes.iter().all(|o| o.result.is_ok())
    }
}

/// How crash reconciliation resolved one entry left pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Resolution {
    /// The system is in the operation's target state: the change happened.
    /// The entry is now applied and can be reverted.
    ConfirmedApplied,
    /// The system still holds the recorded prior state: the change never
    /// happened. The entry is now failed.
    NeverApplied,
    /// The system matches neither the prior nor the target state. The entry is
    /// now failed with a reason saying the state is unknown.
    StateUnknown,
    /// The current state could not be read; the entry stays pending and is
    /// looked at again on the next start.
    Undetermined {
        /// Why detection failed.
        error: PortError,
    },
}

/// Reconciliation result for one entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileOutcome {
    /// The entry that was pending.
    pub entry: EntryId,
    /// What was decided.
    pub resolution: Resolution,
}

/// Result of [`Executor::reconcile_pending`]: one outcome per pending entry,
/// in ascending entry id order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileReport {
    /// One outcome per entry that was pending.
    pub outcomes: Vec<ReconcileOutcome>,
}

const NEVER_APPLIED_REASON: &str =
    "interrupted before the change was made: the system still holds the recorded prior state";
const STATE_UNKNOWN_REASON: &str = "interrupted: the current state matches neither the recorded prior state nor the intended \
     state, so it is unknown";

/// The state the system is in after `operation` has been applied.
fn target_state(operation: &Operation) -> PriorState {
    match operation {
        Operation::SetRegistryValue { value, .. } => PriorState::RegistryValue(Some(value.clone())),
        Operation::DeleteRegistryValue { .. } => PriorState::RegistryValue(None),
        Operation::SetServiceStartType { start_type, .. } => {
            PriorState::ServiceStartType(*start_type)
        }
    }
}

/// Precise result of applying a plan: one outcome per item, in plan order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// The batch all written entries belong to.
    pub batch: BatchId,
    /// One outcome per plan item.
    pub outcomes: Vec<ItemOutcome>,
    /// Present when the plan was applied with rollback and something failed.
    pub rollback: Option<RevertReport>,
}

impl ApplyReport {
    /// Whether every item was applied.
    #[must_use]
    pub fn is_success(&self) -> bool {
        self.outcomes
            .iter()
            .all(|o| matches!(o.status, ItemStatus::Applied { .. }))
    }

    /// Items that were applied.
    pub fn applied(&self) -> impl Iterator<Item = &ItemOutcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, ItemStatus::Applied { .. }))
    }

    /// Items that were attempted and failed.
    pub fn failed(&self) -> impl Iterator<Item = &ItemOutcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, ItemStatus::Failed { .. }))
    }

    /// Items that were never started.
    pub fn not_attempted(&self) -> impl Iterator<Item = &ItemOutcome> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, ItemStatus::NotAttempted))
    }

    /// Entries to undo to get back to the state before the plan, newest first,
    /// with whether each is still pending (applied but not yet marked so).
    fn rollback_targets(&self) -> Vec<(EntryId, bool)> {
        self.outcomes
            .iter()
            .rev()
            .filter_map(|o| match &o.status {
                ItemStatus::Applied { entry } => Some((*entry, false)),
                ItemStatus::Failed {
                    entry: Some(entry),
                    error,
                } if error.system_changed() => Some((*entry, true)),
                _ => None,
            })
            .collect()
    }
}

/// Applies plans and reverts journal entries, enforcing write-ahead journaling.
#[derive(Clone)]
pub struct Executor {
    ports: Ports,
    journal: Arc<dyn Journal>,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for Executor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Executor").finish_non_exhaustive()
    }
}

impl Executor {
    /// Creates an executor over the given ports, journal and clock.
    #[must_use]
    pub fn new(ports: Ports, journal: Arc<dyn Journal>, clock: Arc<dyn Clock>) -> Self {
        Self {
            ports,
            journal,
            clock,
        }
    }

    /// Applies a plan item by item and stops at the first failure.
    ///
    /// Returns `Err` only when no batch could be created, in which case
    /// nothing was changed. Otherwise the report says exactly which items were
    /// applied, which failed and which were not attempted.
    pub fn apply(&self, plan: &Plan, source: Source) -> Result<ApplyReport, ExecError> {
        let batch = self.journal.new_batch().map_err(ExecError::Journal)?;
        let mut outcomes = Vec::with_capacity(plan.items.len());
        let mut halted = false;
        for (index, item) in plan.items.iter().enumerate() {
            let status = if halted {
                ItemStatus::NotAttempted
            } else {
                let status = self.apply_item(item, batch, source);
                halted = !matches!(status, ItemStatus::Applied { .. });
                status
            };
            outcomes.push(ItemOutcome { index, status });
        }
        Ok(ApplyReport {
            batch,
            outcomes,
            rollback: None,
        })
    }

    /// Like [`Executor::apply`], but when an item fails, reverts the items
    /// already applied and attaches the result as `rollback`.
    pub fn apply_or_rollback(&self, plan: &Plan, source: Source) -> Result<ApplyReport, ExecError> {
        let mut report = self.apply(plan, source)?;
        if !report.is_success() {
            report.rollback = Some(self.rollback(&report));
        }
        Ok(report)
    }

    /// Reverts the items a report shows as applied, newest first. Continues
    /// past failures so as much as possible is restored.
    #[must_use]
    pub fn rollback(&self, report: &ApplyReport) -> RevertReport {
        let outcomes = report
            .rollback_targets()
            .into_iter()
            .map(|(entry, allow_pending)| RevertOutcome {
                entry,
                result: self.revert_entry_inner(entry, allow_pending),
            })
            .collect();
        RevertReport { outcomes }
    }

    /// Reverts one applied journal entry. Works from the journal alone, so it
    /// also works after a restart.
    pub fn revert_entry(&self, id: EntryId) -> Result<(), RevertError> {
        self.revert_entry_inner(id, false)
    }

    /// Reverts every applied entry of a batch, newest first. Continues past
    /// failures. `Err` only if the journal cannot be read.
    pub fn revert_batch(&self, batch: BatchId) -> Result<RevertReport, RevertError> {
        let entries = self
            .journal
            .batch_entries(batch)
            .map_err(RevertError::Journal)?;
        let outcomes = entries
            .iter()
            .rev()
            .filter(|entry| entry.state == EntryState::Applied)
            .map(|entry| RevertOutcome {
                entry: entry.id,
                result: self.revert_entry_inner(entry.id, false),
            })
            .collect();
        Ok(RevertReport { outcomes })
    }

    /// Resolves entries a crash left pending. Run once on start, before the
    /// user can apply or undo anything.
    ///
    /// For each pending entry the current state is detected (read-only):
    /// equal to the recorded prior state means the change never happened (the
    /// entry becomes failed); equal to the operation's target means it did
    /// happen (the entry becomes applied, so it can be undone); anything else
    /// becomes failed with a reason saying the state is unknown. When the
    /// state cannot be read the entry stays pending for the next start.
    ///
    /// `Err` only when the journal itself cannot be read or updated.
    pub fn reconcile_pending(&self) -> Result<ReconcileReport, PortError> {
        let pending = self.journal.entries_in_state(EntryState::Pending)?;
        let mut outcomes = Vec::with_capacity(pending.len());
        for entry in pending {
            let resolution = match entry.operation.detect(&self.ports) {
                Err(error) => Resolution::Undetermined { error },
                Ok(current) if current == entry.prior => {
                    self.journal.set_state(
                        entry.id,
                        EntryState::Failed,
                        Some(NEVER_APPLIED_REASON.to_owned()),
                    )?;
                    Resolution::NeverApplied
                }
                Ok(current) if current == target_state(&entry.operation) => {
                    self.journal
                        .set_state(entry.id, EntryState::Applied, None)?;
                    Resolution::ConfirmedApplied
                }
                Ok(_) => {
                    self.journal.set_state(
                        entry.id,
                        EntryState::Failed,
                        Some(STATE_UNKNOWN_REASON.to_owned()),
                    )?;
                    Resolution::StateUnknown
                }
            };
            outcomes.push(ReconcileOutcome {
                entry: entry.id,
                resolution,
            });
        }
        Ok(ReconcileReport { outcomes })
    }

    fn apply_item(&self, item: &PlanItem, batch: BatchId, source: Source) -> ItemStatus {
        let fail = |entry, error| ItemStatus::Failed { entry, error };

        // 1. Detect the prior state. Read-only.
        let prior = match item.operation.detect(&self.ports) {
            Ok(prior) => prior,
            Err(error) => return fail(None, ExecError::Detect(error)),
        };

        // 2. Write-ahead: the journal must hold the prior state before we mutate.
        let new_entry = NewEntry {
            batch,
            timestamp: self.clock.now(),
            source,
            operation: item.operation.clone(),
            prior,
        };
        let entry = match self.journal.record_pending(new_entry) {
            Ok(id) => id,
            Err(error) => return fail(None, ExecError::Journal(error)),
        };

        // 3. Mutate.
        if let Err(error) = item.operation.apply(&self.ports) {
            let journal = self
                .journal
                .set_state(entry, EntryState::Failed, Some(error.to_string()))
                .err();
            return fail(Some(entry), ExecError::Apply { error, journal });
        }

        // 4. Record success.
        match self.journal.set_state(entry, EntryState::Applied, None) {
            Ok(()) => ItemStatus::Applied { entry },
            Err(error) => fail(Some(entry), ExecError::MarkApplied(error)),
        }
    }

    fn revert_entry_inner(&self, id: EntryId, allow_pending: bool) -> Result<(), RevertError> {
        let entry = self
            .journal
            .entry(id)
            .map_err(RevertError::Journal)?
            .ok_or(RevertError::EntryNotFound(id))?;
        let revertible = entry.state == EntryState::Applied
            || (allow_pending && entry.state == EntryState::Pending);
        if !revertible {
            return Err(RevertError::NotRevertible(entry.state));
        }
        // Restoring is idempotent, so a crash between this call and the state
        // update below only means the entry is reverted again later.
        entry
            .operation
            .revert(&entry.prior, &self.ports)
            .map_err(RevertError::Port)?;
        self.journal
            .set_state(id, EntryState::Reverted, None)
            .map_err(RevertError::MarkReverted)
    }
}

#[cfg(test)]
mod tests;
