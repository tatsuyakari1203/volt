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
use super::definition::Tweak;
use crate::error::{ExecError, PlanError, PortError, RevertError};
use crate::executor::{ApplyReport, Executor, RevertReport};
use crate::journal::{BatchId, EntryState, Journal, Source};
use crate::plan::{OperationRequest, Plan};
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
        }
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

    /// Applies the steps that are not yet in their target state, in order, as
    /// one journal batch labelled with the tweak id.
    ///
    /// When a step fails, the steps already applied are reverted, and the
    /// system is read again to check it is back where it started; the report
    /// says which of the two happened ([`ApplyOutcome`]) and holds the
    /// executor's per-step results.
    pub fn apply(&self, id: &str, source: Source) -> Result<TweakApplyReport, TweakError> {
        let tweak = self.tweak(id)?;
        let before = self.steps_at_target(tweak)?;
        if before.iter().all(|at_target| *at_target) {
            return Ok(TweakApplyReport {
                tweak: tweak.id.clone(),
                outcome: ApplyOutcome::AlreadyApplied,
                report: None,
            });
        }
        let plan = self.build_plan(tweak, &before)?;
        let report = self
            .executor
            .apply_or_rollback(&plan, source)
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
        let tweak = self.tweak(id)?;
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
                .revert_batch(batch)
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
                    .map(|current| current == operation.target_state())
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
