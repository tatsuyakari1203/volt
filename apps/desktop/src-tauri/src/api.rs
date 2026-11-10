//! What the UI may ask of the core: transfer types and the functions behind the commands.
//!
//! The commands in [`crate::commands`] are thin wrappers around the functions here, and the
//! integration tests call these same functions, so the tested path is the shipped path.
//!
//! The transfer types are defined here, in the shell, instead of exposing the domain types
//! wholesale: the UI gets what it shows and no more, and a change in the core cannot change the
//! UI contract by accident. 64-bit values never cross as numbers: record ids are decimal
//! strings and times are ISO 8601 strings (UTC), so nothing can lose precision in JavaScript.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::activity::{exec_reason, revert_reason};
use volt_core::{
    ActivityId, ActivityOutcome, ActivityRecord, ApplyOutcome, Category, Evidence, ItemStatus,
    RestartRequirement, Risk, Source, TweakApplyReport, TweakError, TweakRevertReport, TweakState,
};

use crate::IpcError;
use crate::wiring::AppCore;

/// Most records one [`list_activity`] call returns, whatever the caller asks for.
pub const MAX_ACTIVITY_PAGE: u32 = 200;

/// How risky a tweak is. Mirrors [`volt_core::Risk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    /// Harmless and easy to undo.
    Safe,
    /// Can change behaviour the user may notice.
    Caution,
    /// Can break software or the system if misused.
    Advanced,
}

/// Whether the benefit of a tweak was measured. Mirrors [`volt_core::Evidence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum EvidenceLevel {
    /// The effect was measured.
    Measured,
    /// The effect is reported by users but not measured.
    Anecdotal,
}

/// What the user has to do before a tweak takes full effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RestartKind {
    /// Nothing.
    None,
    /// Sign out and in again.
    SignOut,
    /// Restart the machine.
    Reboot,
}

/// The group a tweak is listed under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TweakCategory {
    /// Speed and responsiveness.
    Performance,
    /// Settings for games.
    Gaming,
    /// Telemetry, advertising and tracking.
    Privacy,
    /// Look and feel of the shell.
    Appearance,
    /// Networking.
    Network,
    /// Windows Update behaviour.
    WindowsUpdate,
    /// Security features.
    Security,
    /// Preinstalled software and background tasks.
    Debloat,
}

/// Whether a tweak is in effect right now, read live from the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TweakStateView {
    /// Every step is in its target state.
    Applied,
    /// No step is in its target state.
    NotApplied,
    /// Some steps are in their target state and some are not.
    Partial,
    /// The state could not be read. Nothing was guessed.
    Unreadable {
        /// Why, in words.
        message: String,
    },
}

/// One tweak with its live state, as the Tweaks screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakInfo {
    /// Stable id; pass it to [`apply_tweak`] and [`revert_tweak`].
    pub id: String,
    /// Short name.
    pub title: String,
    /// What it does and why.
    pub description: String,
    /// The group it belongs to.
    pub category: TweakCategory,
    /// How risky it is.
    pub risk: RiskLevel,
    /// Whether its benefit is measured.
    pub evidence: EvidenceLevel,
    /// Where the evidence comes from, in a sentence.
    pub evidence_note: String,
    /// What the user has to do afterwards.
    pub restart: RestartKind,
    /// The state read just now.
    pub state: TweakStateView,
}

/// How an apply ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ApplyStatus {
    /// The tweak is now applied.
    Applied,
    /// It was already in effect; nothing was changed or journaled.
    AlreadyApplied,
    /// A step failed and the system was read again and found exactly as it was before.
    RolledBack,
    /// A step failed and the system may differ from before; the user has to look.
    NeedsAttention,
}

/// Precise result of [`apply_tweak`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    /// The tweak id.
    pub tweak: String,
    /// How it ended.
    pub status: ApplyStatus,
    /// Why it failed, in words, when it did. Never contains command output or data.
    pub detail: Option<String>,
    /// The state read again after the apply.
    pub state: TweakStateView,
}

/// How a revert ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RevertStatus {
    /// Everything that was attempted was restored.
    Reverted,
    /// Something could not be restored; the rest was.
    Incomplete,
    /// The journal holds nothing to undo for this tweak: Volt did not apply it, or it is
    /// already undone. Not an error.
    NothingToRevert,
}

/// Precise result of [`revert_tweak`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RevertResult {
    /// The tweak id.
    pub tweak: String,
    /// Whether the revert was complete.
    pub status: RevertStatus,
    /// How many journal entries were restored.
    pub restored: u32,
    /// How many could not be restored.
    pub failed: u32,
    /// Why something could not be restored, in words.
    pub detail: Option<String>,
    /// The state read again after the revert.
    pub state: TweakStateView,
}

/// Who started an activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ActivitySource {
    /// The user.
    Manual,
    /// A schedule, threshold or profile.
    Automation,
    /// A source this version of the UI does not know.
    Other,
}

/// How an activity ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ActivityStatus {
    /// A change was made.
    Applied,
    /// A change was undone on request.
    Reverted,
    /// An attempt failed or was interrupted.
    Failed,
    /// A change was undone automatically because a later step failed.
    RolledBack,
    /// An outcome this version of the UI does not know.
    Other,
}

/// One line of the activity history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityItem {
    /// Record id as a decimal string.
    pub id: String,
    /// When it happened: ISO 8601, UTC.
    pub time: String,
    /// Who started it.
    pub source: ActivitySource,
    /// How it ended.
    pub status: ActivityStatus,
    /// One redacted sentence: names the target and the outcome, never data.
    pub summary: String,
    /// The tweak id or plan label, when known.
    pub label: Option<String>,
}

/// A page of the activity history, newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPage {
    /// The records of this page.
    pub items: Vec<ActivityItem>,
    /// Pass this as the cursor to get the next (older) page; `None` on the last page.
    pub next_cursor: Option<String>,
}

// ---- the functions behind the commands -----------------------------------------------------

/// Lists every catalogue tweak with its live state. A tweak whose state cannot be read is listed
/// with [`TweakStateView::Unreadable`]; it does not fail the list.
pub fn list_tweaks(core: &AppCore) -> Result<Vec<TweakInfo>, IpcError> {
    let _exclusive = core.exclusive();
    let infos = core
        .engine
        .catalogue()
        .tweaks()
        .iter()
        .map(|tweak| TweakInfo {
            id: tweak.id.clone(),
            title: tweak.title.clone(),
            description: tweak.description.clone(),
            category: category_view(tweak.category),
            risk: risk_view(tweak.risk),
            evidence: evidence_view(tweak.evidence),
            evidence_note: tweak.evidence_note.clone(),
            restart: restart_view(tweak.restart),
            state: state_view(core.engine.state(&tweak.id)),
        })
        .collect();
    Ok(infos)
}

/// Applies a tweak as the user, through the journal. Reports precisely how it ended.
pub fn apply_tweak(core: &AppCore, id: &str) -> Result<ApplyResult, IpcError> {
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let report = core.engine.apply(id, Source::Manual);
    warn_if_history_incomplete(core, failures_before, "applying a tweak");
    let report = report?;

    let (status, detail) = match report.outcome {
        ApplyOutcome::AlreadyApplied => (ApplyStatus::AlreadyApplied, None),
        ApplyOutcome::Applied => (ApplyStatus::Applied, None),
        ApplyOutcome::RolledBack => (ApplyStatus::RolledBack, apply_failure_detail(&report)),
        // `NeedsAttention`, and (the enum is non-exhaustive) any outcome this shell does not
        // know yet, which is treated as the cautious one.
        _ => (ApplyStatus::NeedsAttention, apply_failure_detail(&report)),
    };
    Ok(ApplyResult {
        tweak: report.tweak,
        status,
        detail,
        state: state_view(core.engine.state(id)),
    })
}

/// Reverts what Volt applied for a tweak, restoring the state the journal recorded. Reports
/// whether the revert was complete. Nothing to undo is a result, not an error.
pub fn revert_tweak(core: &AppCore, id: &str) -> Result<RevertResult, IpcError> {
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let result = core.engine.revert(id);
    warn_if_history_incomplete(core, failures_before, "reverting a tweak");

    let (status, restored, failed, detail) = match result {
        Ok(report) => {
            let (restored, failed) = count_outcomes(&report);
            let status = if report.is_complete() {
                RevertStatus::Reverted
            } else {
                RevertStatus::Incomplete
            };
            (status, restored, failed, revert_failure_detail(&report))
        }
        Err(TweakError::NothingToRevert(_)) => (RevertStatus::NothingToRevert, 0, 0, None),
        Err(error) => return Err(error.into()),
    };
    Ok(RevertResult {
        tweak: id.to_owned(),
        status,
        restored,
        failed,
        detail,
        state: state_view(core.engine.state(id)),
    })
}

/// Reads the activity history, newest first: at most `limit` records (1 to
/// [`MAX_ACTIVITY_PAGE`]), starting after `cursor` (the `next_cursor` of the previous page).
pub fn list_activity(
    core: &AppCore,
    limit: u32,
    cursor: Option<&str>,
) -> Result<ActivityPage, IpcError> {
    let limit = limit.clamp(1, MAX_ACTIVITY_PAGE);
    let before = cursor
        .map(|text| {
            text.parse::<u64>()
                .map(ActivityId)
                .map_err(|_| IpcError::InvalidInput {
                    message: format!("`{text}` is not an activity cursor"),
                })
        })
        .transpose()?;
    let page_len = usize::try_from(limit).unwrap_or(usize::MAX);
    // One extra record tells whether there is another page without a second query.
    let mut records = core
        .activity
        .list(page_len.saturating_add(1), before)
        .map_err(IpcError::from)?;
    let has_more = records.len() > page_len;
    records.truncate(page_len);
    let next_cursor = if has_more {
        records.last().map(|record| record.id.0.to_string())
    } else {
        None
    };
    Ok(ActivityPage {
        items: records.iter().map(activity_item).collect(),
        next_cursor,
    })
}

// ---- mapping -------------------------------------------------------------------------------

fn risk_view(risk: Risk) -> RiskLevel {
    match risk {
        Risk::Safe => RiskLevel::Safe,
        Risk::Caution => RiskLevel::Caution,
        Risk::Advanced => RiskLevel::Advanced,
    }
}

fn evidence_view(evidence: Evidence) -> EvidenceLevel {
    match evidence {
        Evidence::Measured => EvidenceLevel::Measured,
        Evidence::Anecdotal => EvidenceLevel::Anecdotal,
    }
}

fn restart_view(restart: RestartRequirement) -> RestartKind {
    match restart {
        RestartRequirement::None => RestartKind::None,
        RestartRequirement::SignOut => RestartKind::SignOut,
        RestartRequirement::Reboot => RestartKind::Reboot,
    }
}

fn category_view(category: Category) -> TweakCategory {
    match category {
        Category::Performance => TweakCategory::Performance,
        Category::Gaming => TweakCategory::Gaming,
        Category::Privacy => TweakCategory::Privacy,
        Category::Appearance => TweakCategory::Appearance,
        Category::Network => TweakCategory::Network,
        Category::WindowsUpdate => TweakCategory::WindowsUpdate,
        Category::Security => TweakCategory::Security,
        Category::Debloat => TweakCategory::Debloat,
    }
}

fn state_view(state: Result<TweakState, TweakError>) -> TweakStateView {
    match state {
        Ok(TweakState::Applied) => TweakStateView::Applied,
        Ok(TweakState::NotApplied) => TweakStateView::NotApplied,
        Ok(TweakState::Partial) => TweakStateView::Partial,
        Err(error) => TweakStateView::Unreadable {
            message: error.to_string(),
        },
    }
}

fn activity_item(record: &ActivityRecord) -> ActivityItem {
    ActivityItem {
        id: record.id.0.to_string(),
        time: record.timestamp.to_iso8601(),
        source: match record.source {
            Source::Manual => ActivitySource::Manual,
            Source::Automation => ActivitySource::Automation,
            _ => ActivitySource::Other,
        },
        status: match record.outcome {
            ActivityOutcome::Applied => ActivityStatus::Applied,
            ActivityOutcome::Reverted => ActivityStatus::Reverted,
            ActivityOutcome::Failed => ActivityStatus::Failed,
            ActivityOutcome::RolledBack => ActivityStatus::RolledBack,
            _ => ActivityStatus::Other,
        },
        summary: record.summary.clone(),
        label: record.label.clone(),
    }
}

/// Why an apply failed, in words (first failed step), and how many applied steps could not be
/// put back. Built from the core's redacted reasons: no port message, no data.
fn apply_failure_detail(report: &TweakApplyReport) -> Option<String> {
    let report = report.report.as_ref()?;
    let mut parts = Vec::new();
    if let Some(reason) = report.failed().find_map(|item| match &item.status {
        ItemStatus::Failed { error, .. } => Some(exec_reason(error)),
        _ => None,
    }) {
        parts.push(reason);
    }
    let not_restored = report.rollback.as_ref().map_or(0, |rollback| {
        rollback
            .outcomes
            .iter()
            .filter(|outcome| outcome.result.is_err())
            .count()
    });
    if not_restored > 0 {
        parts.push(format!(
            "{not_restored} change(s) could not be put back automatically"
        ));
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

fn count_outcomes(report: &TweakRevertReport) -> (u32, u32) {
    let (mut restored, mut failed) = (0_usize, 0_usize);
    for outcome in report
        .batches
        .iter()
        .flat_map(|batch| &batch.report.outcomes)
    {
        if outcome.result.is_ok() {
            restored += 1;
        } else {
            failed += 1;
        }
    }
    (
        u32::try_from(restored).unwrap_or(u32::MAX),
        u32::try_from(failed).unwrap_or(u32::MAX),
    )
}

fn revert_failure_detail(report: &TweakRevertReport) -> Option<String> {
    report
        .batches
        .iter()
        .flat_map(|batch| &batch.report.outcomes)
        .find_map(|outcome| outcome.result.as_ref().err().map(revert_reason))
}

/// Logs a warning when the executor could not write activity records during a call: the change
/// and its journal entry are safe, but the history the Undo page shows is incomplete.
fn warn_if_history_incomplete(core: &AppCore, failures_before: u64, action: &str) {
    let failures = core
        .engine
        .activity_write_failures()
        .saturating_sub(failures_before);
    if failures > 0 {
        log::warn!(
            "{failures} activity record(s) could not be written while {action}; the change and \
             its undo journal entry are safe, but the history on the Undo page is incomplete"
        );
    }
}
