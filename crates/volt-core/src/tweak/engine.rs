//! The tweak engine: reads, applies and reverts catalogue tweaks.
//!
//! Every change goes through the [`Executor`], so write-ahead journaling cannot
//! be bypassed. Each apply is journaled as one batch labelled with the tweak
//! id, which is how a revert finds it again after a restart.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::catalogue::Catalogue;
use super::definition::{Tweak, TweakStep};
use crate::activity::ActivityLog;
use crate::error::{ExecError, PlanError, PortError, RevertError};
use crate::executor::{ApplyReport, Executor, RevertReport};
use crate::journal::{BatchId, EntryState, Journal, Source};
use crate::operation::Operation;
use crate::plan::{OperationRequest, Plan, Preview};
use crate::ports::Ports;
use crate::time::Clock;

/// Whether a tweak's changes are in place right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TweakState {
    /// Every step is in its target state.
    Applied,
    /// No step is in its target state.
    NotApplied,
    /// Some steps are in their target state and some are not.
    Partial,
}

/// Why a tweak operation could not be carried out.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TweakError {
    /// The catalogue has no tweak with this id.
    #[error("unknown tweak `{0}`")]
    UnknownTweak(String),
    /// The tweak changes machine-wide settings and the engine was not told it
    /// has administrator rights. Nothing was changed.
    #[error("tweak `{0}` needs administrator rights")]
    NeedsAdministrator(String),
    /// The current state of a step could not be read. Nothing was changed.
    #[error("tweak `{tweak}`: cannot read the state of steps[{step}]: {error}")]
    Detect {
        /// The tweak id.
        tweak: String,
        /// Zero-based index of the step.
        step: usize,
        /// The underlying failure.
        error: PortError,
    },
    /// The plan could not be built. Nothing was changed.
    #[error("tweak `{tweak}`: {error}")]
    Plan {
        /// The tweak id.
        tweak: String,
        /// The underlying failure.
        error: PlanError,
    },
    /// The journal refused to start the batch, so nothing was changed.
    #[error("tweak `{tweak}`: {error}")]
    Exec {
        /// The tweak id.
        tweak: String,
        /// The underlying failure.
        error: ExecError,
    },
    /// The journal could not be searched for this tweak's batches.
    #[error("tweak `{tweak}`: cannot read the undo journal: {error}")]
    Journal {
        /// The tweak id.
        tweak: String,
        /// The underlying failure.
        error: PortError,
    },
    /// The journal holds nothing to undo for this tweak: Volt did not apply
    /// it, or it was already reverted. A change made outside Volt cannot be
    /// undone by Volt because its prior state was never recorded.
    #[error("tweak `{0}`: the undo journal has no applied change to revert")]
    NothingToRevert(String),
    /// The tweak cannot be reset to the Windows default: a step of it does not say what the
    /// default is, so Volt does not know it.
    #[error("tweak `{0}` cannot be reset to the Windows default")]
    NothingToReset(String),
    /// Volt applied the tweak itself, so its journal holds the exact earlier state: undo is the
    /// way to turn it off, a reset would guess.
    #[error("tweak `{0}` was applied by Volt: undo it instead of resetting it")]
    AppliedByVolt(String),
    /// A journal batch could not be read for revert.
    #[error("tweak `{tweak}`: {error}")]
    Revert {
        /// The tweak id.
        tweak: String,
        /// The underlying failure.
        error: RevertError,
    },
}

/// How an apply ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ApplyOutcome {
    /// Every step was already in its target state; nothing was changed or
    /// journaled.
    AlreadyApplied,
    /// Every step is applied.
    Applied,
    /// A step failed and the system was read again and is exactly as it was
    /// before the apply.
    RolledBack,
    /// A step failed and the system may differ from what it was before: the
    /// rollback was incomplete or the state no longer matches. The report says
    /// which steps were applied, which failed and what the rollback did.
    NeedsAttention,
}

/// Precise result of [`TweakEngine::apply`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TweakApplyReport {
    /// The tweak id.
    pub tweak: String,
    /// How it ended.
    pub outcome: ApplyOutcome,
    /// The executor's report, with the rollback when a step failed. `None`
    /// when there was nothing to apply. Item indexes refer to the steps that
    /// were applied: the ones not already in their target state.
    pub report: Option<ApplyReport>,
}

/// The revert of one journal batch of a tweak.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchRevert {
    /// The batch that was reverted.
    pub batch: BatchId,
    /// What happened to each of its entries.
    pub report: RevertReport,
}

/// Precise result of [`TweakEngine::revert`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TweakRevertReport {
    /// The tweak id.
    pub tweak: String,
    /// The batches reverted, newest first. Stops after the first batch that
    /// could not be fully restored.
    pub batches: Vec<BatchRevert>,
}

impl TweakRevertReport {
    /// Whether everything that was attempted was restored.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.batches.iter().all(|b| b.report.is_complete())
    }
}

/// What one step of a tweak does, and whether it is already in place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPreview {
    /// What is changed, the value now and the value after the tweak.
    pub preview: Preview,
    /// Whether the system is already in this step's target state.
    pub at_target: bool,
}

/// Suffix of the journal label of a reset (`<tweak id>:reset`). It differs from the tweak id on
/// purpose: [`TweakEngine::revert`] undoes the batches labelled with the plain id, and must never
/// find a reset and switch the tweak back on.
pub const RESET_LABEL_SUFFIX: &str = ":reset";

/// Reads, applies and reverts the tweaks of a [`Catalogue`].
///
/// Revert works from the journal alone: a new engine over the same journal,
/// for example after a restart, can revert what an earlier one applied.
#[derive(Clone)]
pub struct TweakEngine {
    catalogue: Arc<Catalogue>,
    ports: Ports,
    journal: Arc<dyn Journal>,
    executor: Executor,
    administrator: bool,
}

impl fmt::Debug for TweakEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TweakEngine")
            .field("tweaks", &self.catalogue.len())
            .finish_non_exhaustive()
    }
}

impl TweakEngine {
    /// Creates an engine over the given catalogue, ports, journal and clock.
    #[must_use]
    pub fn new(
        catalogue: Arc<Catalogue>,
        ports: Ports,
        journal: Arc<dyn Journal>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let executor = Executor::new(ports.clone(), journal.clone(), clock);
        Self {
            catalogue,
            ports,
            journal,
            executor,
            administrator: false,
        }
    }

    /// Tells the engine whether the process has administrator rights. It starts
    /// as `false`: a tweak that [`requires_admin`](Tweak::requires_admin) is then refused with
    /// [`TweakError::NeedsAdministrator`] before anything is read or changed. How a build gets
    /// those rights is up to the composition root; nothing in the core elevates.
    #[must_use]
    pub fn with_administrator(mut self, administrator: bool) -> Self {
        self.administrator = administrator;
        self
    }

    /// Attaches an activity log, see [`Executor::with_activity_log`]. Applies
    /// are recorded with the tweak id as label, and so are the reverts.
    #[must_use]
    pub fn with_activity_log(mut self, activity: Arc<dyn ActivityLog>) -> Self {
        self.executor = self.executor.with_activity_log(activity);
        self
    }

    /// How many activity records could not be written, see
    /// [`Executor::activity_write_failures`].
    #[must_use]
    pub fn activity_write_failures(&self) -> u64 {
        self.executor.activity_write_failures()
    }

    /// The executor this engine applies and reverts through. It is a handle on the same journal,
    /// activity log and failure counter, so a caller that has to undo one entry or one batch (the
    /// Undo page) goes through the same write-ahead path as the engine itself.
    #[must_use]
    pub fn executor(&self) -> &Executor {
        &self.executor
    }

    /// The catalogue this engine works on.
    #[must_use]
    pub fn catalogue(&self) -> &Catalogue {
        &self.catalogue
    }

    /// Whether the tweak is applied right now. Read-only.
    ///
    /// Compares the live state of every step with its target, so a tweak
    /// changed outside Volt is reported correctly too. A step whose state
    /// cannot be read is an error, never a guess.
    pub fn state(&self, id: &str) -> Result<TweakState, TweakError> {
        let tweak = self.tweak(id)?;
        let steps = self.steps_at_target(tweak)?;
        Ok(if steps.iter().all(|at_target| *at_target) {
            TweakState::Applied
        } else if steps.iter().any(|at_target| *at_target) {
            TweakState::Partial
        } else {
            TweakState::NotApplied
        })
    }

    /// The dry run: what [`TweakEngine::apply`] would change right now. Only
    /// reads the system. Steps already in their target state are left out, so
    /// the plan is empty for an applied tweak.
    pub fn plan(&self, id: &str) -> Result<Plan, TweakError> {
        let tweak = self.tweak(id)?;
        let at_target = self.steps_at_target(tweak)?;
        self.build_plan(tweak, &at_target)
    }

    /// Every step of the tweak with its current and target value, whether or not it is in place
    /// already. Read-only. [`TweakEngine::plan`] leaves out the steps already in place; this does
    /// not, so the screen can say exactly what a tweak changes even when it is on.
    pub fn describe(&self, id: &str) -> Result<Vec<StepPreview>, TweakError> {
        let tweak = self.tweak(id)?;
        let at_target = self.steps_at_target(tweak)?;
        let requests = tweak
            .steps
            .iter()
            .map(|step| OperationRequest::new(step.operation(), tweak.risk, tweak.evidence))
            .collect();
        let plan = Plan::build(requests, &self.ports).map_err(|error| TweakError::Plan {
            tweak: tweak.id.clone(),
            error,
        })?;
        Ok(plan
            .items
            .into_iter()
            .zip(at_target)
            .map(|(item, at_target)| StepPreview {
                preview: item.preview,
                at_target,
            })
            .collect())
    }

    /// Whether the undo journal holds a change Volt made for this tweak that is still applied.
    /// `false` for a tweak that is on because the user or another program set it: Volt then has
    /// no earlier value to restore. Read-only.
    pub fn applied_by_volt(&self, id: &str) -> Result<bool, TweakError> {
        let tweak = self.tweak(id)?;
        let journal_error = |error| TweakError::Journal {
            tweak: tweak.id.clone(),
            error,
        };
        for batch in self
            .journal
            .batches_with_label(&tweak.id)
            .map_err(journal_error)?
        {
            let entries = self.journal.batch_entries(batch).map_err(journal_error)?;
            if entries
                .iter()
                .any(|entry| entry.state == EntryState::Applied)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// What [`TweakEngine::reset`] would do right now: put every step that is in place back to
    /// its Windows default (see [`TweakStep::reset_operation`]), last step first. Read-only.
    /// `None` when the tweak cannot be reset (a step does not declare its default), and an empty
    /// plan when nothing of it is in place.
    pub fn reset_plan(&self, id: &str) -> Result<Option<Plan>, TweakError> {
        let tweak = self.tweak(id)?;
        let Some(operations) = Self::reset_operations(tweak) else {
            return Ok(None);
        };
        let at_target = self.steps_at_target(tweak)?;
        self.build_reset_plan(tweak, operations, &at_target)
            .map(Some)
    }

    /// Turns a tweak off that Volt has no record of applying, by putting each of its steps back to
    /// the Windows default the catalogue declares. The change is journaled like any other (so the
    /// Undo page can bring the tweak back) under the label `<id>:reset`, not under the tweak id.
    /// Refused with [`TweakError::NothingToReset`] when a step has no declared default, and with
    /// [`TweakError::AppliedByVolt`] when Volt applied the tweak itself: its journal then holds
    /// the exact earlier state and undo is the right way.
    pub fn reset(&self, id: &str, source: Source) -> Result<TweakApplyReport, TweakError> {
        let tweak = self.tweak(id)?;
        self.check_rights(tweak)?;
        if self.applied_by_volt(id)? {
            return Err(TweakError::AppliedByVolt(tweak.id.clone()));
        }
        let operations = Self::reset_operations(tweak)
            .ok_or_else(|| TweakError::NothingToReset(tweak.id.clone()))?;
        let before = self.steps_at_target(tweak)?;
        let plan = self.build_reset_plan(tweak, operations, &before)?;
        if plan.is_empty() {
            return Ok(TweakApplyReport {
                tweak: tweak.id.clone(),
                outcome: ApplyOutcome::AlreadyApplied,
                report: None,
            });
        }
        self.execute(tweak, &plan, &before, source)
    }

    /// What turns each step off, when every step says so.
    fn reset_operations(tweak: &Tweak) -> Option<Vec<Operation>> {
        tweak.steps.iter().map(TweakStep::reset_operation).collect()
    }

    /// The reset of the steps that are in place, in reverse step order, so a value goes before
    /// the key it sits in.
    fn build_reset_plan(
        &self,
        tweak: &Tweak,
        operations: Vec<Operation>,
        at_target: &[bool],
    ) -> Result<Plan, TweakError> {
        let requests = operations
            .into_iter()
            .zip(at_target)
            .filter(|(_, at_target)| **at_target)
            .map(|(operation, _)| OperationRequest::new(operation, tweak.risk, tweak.evidence))
            .rev()
            .collect();
        Plan::build(requests, &self.ports)
            .map(|plan| plan.with_label(format!("{}{RESET_LABEL_SUFFIX}", tweak.id)))
            .map_err(|error| TweakError::Plan {
                tweak: tweak.id.clone(),
                error,
            })
    }

    /// Applies the steps that are not yet in their target state, in order, as
    /// one journal batch labelled with the tweak id.
    ///
    /// When a step fails, the steps already applied are reverted, and the
    /// system is read again to check it is back where it started; the report
    /// says which of the two happened ([`ApplyOutcome`]) and holds the
    /// executor's per-step results.
    pub fn apply(&self, id: &str, source: Source) -> Result<TweakApplyReport, TweakError> {
        let tweak = self.tweak(id)?;
        self.check_rights(tweak)?;
        let before = self.steps_at_target(tweak)?;
        if before.iter().all(|at_target| *at_target) {
            return Ok(TweakApplyReport {
                tweak: tweak.id.clone(),
                outcome: ApplyOutcome::AlreadyApplied,
                report: None,
            });
        }
        let plan = self.build_plan(tweak, &before)?;
        self.execute(tweak, &plan, &before, source)
    }

    /// Runs `plan` as one journal batch and classifies how it ended. `before` is the per-step
    /// state to compare with after a failure.
    fn execute(
        &self,
        tweak: &Tweak,
        plan: &Plan,
        before: &[bool],
        source: Source,
    ) -> Result<TweakApplyReport, TweakError> {
        let report = self
            .executor
            .apply_or_rollback(plan, source)
            .map_err(|error| TweakError::Exec {
                tweak: tweak.id.clone(),
                error,
            })?;
        let outcome = if report.is_success() {
            ApplyOutcome::Applied
        } else {
            let rollback_complete = report
                .rollback
                .as_ref()
                .is_some_and(RevertReport::is_complete);
            let restored = self
                .steps_at_target(tweak)
                .is_ok_and(|after| after == before);
            if rollback_complete && restored {
                ApplyOutcome::RolledBack
            } else {
                ApplyOutcome::NeedsAttention
            }
        };
        Ok(TweakApplyReport {
            tweak: tweak.id.clone(),
            outcome,
            report: Some(report),
        })
    }

    /// Reverts what Volt applied for this tweak, restoring the state the
    /// journal recorded before each change.
    ///
    /// Finds the journal batches labelled with the tweak id that still have
    /// applied entries and reverts them newest first, so it works after a
    /// restart. Steps that were already in their target state when the tweak
    /// was applied were not changed by Volt and are left alone.
    pub fn revert(&self, id: &str) -> Result<TweakRevertReport, TweakError> {
        self.revert_as(id, Source::Manual)
    }

    /// [`TweakEngine::revert`] that records `source` as who started the revert.
    pub fn revert_as(&self, id: &str, source: Source) -> Result<TweakRevertReport, TweakError> {
        let tweak = self.tweak(id)?;
        self.check_rights(tweak)?;
        let journal_error = |error| TweakError::Journal {
            tweak: tweak.id.clone(),
            error,
        };
        let mut batches = self
            .journal
            .batches_with_label(&tweak.id)
            .map_err(journal_error)?;
        batches.reverse();

        let mut reverted = Vec::new();
        for batch in batches {
            let has_applied = self
                .journal
                .batch_entries(batch)
                .map_err(journal_error)?
                .iter()
                .any(|entry| entry.state == EntryState::Applied);
            if !has_applied {
                continue;
            }
            let report = self
                .executor
                .revert_batch(batch, source, Some(&tweak.id))
                .map_err(|error| TweakError::Revert {
                    tweak: tweak.id.clone(),
                    error,
                })?;
            let complete = report.is_complete();
            reverted.push(BatchRevert { batch, report });
            if !complete {
                break;
            }
        }
        if reverted.is_empty() {
            return Err(TweakError::NothingToRevert(tweak.id.clone()));
        }
        Ok(TweakRevertReport {
            tweak: tweak.id.clone(),
            batches: reverted,
        })
    }

    fn tweak(&self, id: &str) -> Result<&Tweak, TweakError> {
        self.catalogue
            .get(id)
            .ok_or_else(|| TweakError::UnknownTweak(id.to_owned()))
    }

    /// Refuses a tweak that needs administrator rights the engine does not have.
    fn check_rights(&self, tweak: &Tweak) -> Result<(), TweakError> {
        if tweak.requires_admin && !self.administrator {
            Err(TweakError::NeedsAdministrator(tweak.id.clone()))
        } else {
            Ok(())
        }
    }

    /// For every step: whether the system is already in its target state.
    fn steps_at_target(&self, tweak: &Tweak) -> Result<Vec<bool>, TweakError> {
        tweak
            .steps
            .iter()
            .enumerate()
            .map(|(step, definition)| {
                let operation = definition.operation();
                operation
                    .detect(&self.ports)
                    .map(|current| operation.is_target(&current))
                    .map_err(|error| TweakError::Detect {
                        tweak: tweak.id.clone(),
                        step,
                        error,
                    })
            })
            .collect()
    }

    fn build_plan(&self, tweak: &Tweak, at_target: &[bool]) -> Result<Plan, TweakError> {
        let requests = tweak
            .steps
            .iter()
            .zip(at_target)
            .filter(|(_, at_target)| !**at_target)
            .map(|(step, _)| OperationRequest::new(step.operation(), tweak.risk, tweak.evidence))
            .collect();
        Plan::build(requests, &self.ports)
            .map(|plan| plan.with_label(tweak.id.clone()))
            .map_err(|error| TweakError::Plan {
                tweak: tweak.id.clone(),
                error,
            })
    }
}
