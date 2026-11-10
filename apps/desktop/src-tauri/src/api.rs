//! What the UI may ask of the core: transfer types and the functions behind the commands.
//!
//! The commands in [`crate::commands`] are thin wrappers around the functions here, and the
//! integration tests call these same functions, so the tested path is the shipped path.
//!
//! The transfer types are defined here, in the shell, instead of exposing the domain types
//! wholesale: the UI gets what it shows and no more, and a change in the core cannot change the
//! UI contract by accident. 64-bit values never cross as numbers: record ids are decimal
//! strings and times are ISO 8601 strings (UTC), so nothing can lose precision in JavaScript.

use std::path::Path;

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::activity::{exec_reason, restore_words_for, revert_reason, subject};
use volt_core::startup::STARTUP_LABEL;
use volt_core::{
    ActivityId, ActivityOutcome, ActivityRecord, ApplyOutcome, BatchId, Category, ChangeOutcome,
    Clock, EntryId, EntryState, Evidence, ItemStatus, JournalEntry, RESET_LABEL_SUFFIX,
    RestartRequirement, RevertError, RevertOutcome, RevertReport, Risk, Source, StartupEntry,
    StartupOrigin, StartupScope, StartupState, StepPreview, SystemClock, Timestamp,
    TweakApplyReport, TweakError, TweakRevertReport, TweakState, activity, overwrites_of,
    render_text,
};

use crate::IpcError;
use crate::wiring::AppCore;

/// Most records one [`list_activity`] call returns, whatever the caller asks for.
pub const MAX_ACTIVITY_PAGE: u32 = 200;

/// Records read per round trip when the whole activity log is exported.
const EXPORT_CHUNK: usize = 500;

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
    /// Whether changing it needs administrator rights. The screen shows such a tweak switched
    /// off and labelled; applying it answers `accessDenied` before anything is touched.
    pub needs_admin: bool,
    /// The state read just now.
    pub state: TweakStateView,
    /// Whether the undo journal holds a change Volt made for this tweak. When it is on and this
    /// is `false`, it was set outside Volt: Volt has no earlier value to restore.
    pub applied_by_volt: bool,
}

/// One step of a tweak: what it touches and the value now and after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakStepView {
    /// What is changed, such as `HKCU\Software\X\Y`, `service Spooler start type` or a command.
    pub subject: String,
    /// The value right now, as text.
    pub current: String,
    /// The value the tweak sets (or, in a reset, the value that is left), as text.
    pub target: String,
    /// Whether the system is already in the target state for this step.
    pub at_target: bool,
}

/// What a tweak changes, for the Advanced list and the confirmation dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TweakPlanView {
    /// The tweak id.
    pub id: String,
    /// Every step of the tweak, in order.
    pub steps: Vec<TweakStepView>,
    /// What turning the tweak off would write when Volt has no earlier value to restore: the
    /// values are removed so Windows uses its default. `None` when the tweak cannot be turned
    /// off that way (it has a service or a command).
    pub reset: Option<Vec<TweakStepView>>,
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

/// Where an activity line stands with respect to undo. The Undo page shows it on every row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ChangeState {
    /// The change was made and its earlier value is in the undo journal: it can be undone.
    CanUndo,
    /// The change was made and has since been undone.
    Undone,
    /// The attempt failed (or was interrupted), so there is nothing to undo.
    Failed,
    /// This line has no usable undo record; `reason` says why.
    CannotUndo,
    /// This line itself records an undo (a revert or an automatic rollback).
    Restored,
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
    /// The tweak's title when the label is a catalogue tweak.
    pub title: Option<String>,
    /// The journal batch (the changes made together) as a decimal string, when there is one.
    pub batch: Option<String>,
    /// The journal entry as a decimal string, when there is one. Pass it to [`undo_entry`].
    pub entry: Option<String>,
    /// The value before the change, redacted: numbers and service start types are shown, string
    /// and binary data only as kind and length. Only set for a change that was made.
    pub previous: Option<String>,
    /// Whether this line can be undone, was undone, failed and so on.
    pub state: ChangeState,
    /// One line saying why the change cannot be undone, when `state` is `cannotUndo`.
    pub reason: Option<String>,
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

/// How an undo ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum UndoStatus {
    /// Everything that was attempted was restored.
    Restored,
    /// Some changes were restored and some were not; `failures` says which.
    Partial,
    /// Nothing was restored; `failures` says why.
    Failed,
    /// The journal holds nothing to undo here: it was already undone, or never was a change.
    NothingToUndo,
}

/// One change an undo could not restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoFailure {
    /// The journal entry, as a decimal string.
    pub entry: String,
    /// What it touches, in words, without any data.
    pub what: String,
    /// Why it could not be restored, in words. Never contains command output or data.
    pub reason: String,
}

/// Precise result of [`undo_entry`] and [`undo_batch`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    /// How it ended.
    pub status: UndoStatus,
    /// How many changes were restored.
    pub restored: u32,
    /// How many could not be restored.
    pub failed: u32,
    /// The ones that could not be restored, with the reason.
    pub failures: Vec<UndoFailure>,
}

/// One change an undo of a batch would restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoPreviewItem {
    /// The journal entry, as a decimal string.
    pub entry: String,
    /// What it touches, in words, without any data.
    pub what: String,
    /// What undoing does to it, redacted: `set back to REG_DWORD 1`, `removed (it did not exist
    /// before)`.
    pub restore: String,
}

/// What [`undo_batch`] would restore right now. Reads only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoPreview {
    /// The changes that are still applied, newest first: the order they are undone in.
    pub items: Vec<UndoPreviewItem>,
    /// Changes in `items` whose target was changed again later; undoing would replace that later
    /// change.
    pub overwrites: Vec<UndoOverwrite>,
}

/// A later change an undo would silently overwrite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoOverwrite {
    /// The journal entry being undone, as a decimal string.
    pub entry: String,
    /// What the later change touches, in words, without any data.
    pub what: String,
    /// The later journal entry, as a decimal string.
    pub later_entry: String,
    /// When the later change was made (ISO 8601, UTC).
    pub later_time: String,
    /// Who made the later change.
    pub later_source: ActivitySource,
}

/// Whether the history on the Undo page is complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HistoryStatus {
    /// An activity record could not be written since the app started. The changes and their undo
    /// entries are safe; the list is missing rows.
    pub incomplete: bool,
}

/// The activity log rendered as text, ready to save or copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ActivityExport {
    /// A suggested file name, `volt-activity-<date>.txt`.
    pub file_name: String,
    /// The text: one line per record, newest first. Follows the redaction rule.
    pub text: String,
    /// How many records it holds.
    pub records: u32,
}

/// How many records [`clear_history`] removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ClearResult {
    /// Records removed from the activity history.
    pub removed: u32,
}

/// How [`export_activity`] ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ExportResult {
    /// The file was written.
    Saved {
        /// Where it was written.
        path: String,
        /// How many records it holds.
        records: u32,
    },
    /// The user closed the save dialog without choosing a file. Nothing was written.
    Cancelled,
}

// ---- startup apps: transfer types ------------------------------------------------------------

/// What the Undo page calls the batches the startup manager writes.
const STARTUP_TITLE: &str = "Startup apps";

/// Where a startup entry comes from. Mirrors [`volt_core::StartupOrigin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StartupSource {
    /// The current user's Run key.
    UserRun,
    /// The machine-wide Run key.
    MachineRun,
    /// The machine-wide Run key of 32-bit programs.
    MachineRun32,
    /// The current user's Startup folder.
    UserFolder,
    /// The Startup folder shared by all users.
    CommonFolder,
    /// A source this version of the UI does not know.
    Other,
}

/// One thing that starts with Windows, as the Startup screen shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupEntryView {
    /// Stable id; pass it to [`set_startup_entry_enabled`].
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// Where it comes from.
    pub source: StartupSource,
    /// A short plain-words name of the source, for a badge.
    pub source_label: String,
    /// The command as written, with environment variables unexpanded.
    pub command: String,
    /// Who published the program, when its version information says.
    pub publisher: Option<String>,
    /// Whether it starts with Windows.
    pub enabled: bool,
    /// Whether changing it needs administrator rights (it applies to every user).
    pub needs_admin: bool,
}

/// A source that could not be read; the rest of the list is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupProblemView {
    /// The source.
    pub source: StartupSource,
    /// A short plain-words name of the source.
    pub source_label: String,
    /// The class of the failure ("access denied", "not found", ...), never a message with data.
    pub reason: String,
}

/// Everything that starts with Windows, read just now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupList {
    /// The entries, sorted by name.
    pub entries: Vec<StartupEntryView>,
    /// Sources that could not be read.
    pub problems: Vec<StartupProblemView>,
}

/// How a change of a startup entry ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StartupChangeStatus {
    /// The state was changed (and can be undone on the Undo page).
    Changed,
    /// The entry was already in the requested state; nothing was changed or journaled.
    AlreadyInState,
}

/// Precise result of [`set_startup_entry_enabled`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupChangeResult {
    /// The entry id.
    pub id: String,
    /// The entry's name.
    pub name: String,
    /// How it ended.
    pub status: StartupChangeStatus,
    /// The state read again afterwards; `null` when it could not be read.
    pub enabled: Option<bool>,
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
            needs_admin: tweak.requires_admin,
            state: state_view(core.engine.state(&tweak.id)),
            applied_by_volt: core.engine.applied_by_volt(&tweak.id).unwrap_or(false),
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

/// What a tweak changes now, and what turning it off would write without a journal record.
pub fn get_tweak_plan(core: &AppCore, id: &str) -> Result<TweakPlanView, IpcError> {
    let _exclusive = core.exclusive();
    let steps = core
        .engine
        .describe(id)?
        .into_iter()
        .map(step_view)
        .collect();
    let reset = core.engine.reset_plan(id)?.map(|plan| {
        plan.items
            .into_iter()
            .map(|item| TweakStepView {
                subject: item.preview.subject,
                current: item.preview.current,
                target: item.preview.target,
                at_target: true,
            })
            .collect()
    });
    Ok(TweakPlanView {
        id: id.to_owned(),
        steps,
        reset,
    })
}

/// Turns off a tweak Volt did not apply by removing the registry values it sets. Journaled.
pub fn reset_tweak(core: &AppCore, id: &str) -> Result<RevertResult, IpcError> {
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let report = core.engine.reset(id, Source::Manual);
    warn_if_history_incomplete(core, failures_before, "resetting a tweak");
    let report = report?;
    let restored = report.report.as_ref().map_or(0, |r| {
        u32::try_from(r.applied().count()).unwrap_or(u32::MAX)
    });
    let (status, failed, detail) = match report.outcome {
        ApplyOutcome::Applied | ApplyOutcome::AlreadyApplied => (RevertStatus::Reverted, 0, None),
        _ => (RevertStatus::Incomplete, 1, apply_failure_detail(&report)),
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

fn step_view(step: StepPreview) -> TweakStepView {
    TweakStepView {
        subject: step.preview.subject,
        current: step.preview.current,
        target: step.preview.target,
        at_target: step.at_target,
    }
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
        items: records
            .iter()
            .map(|record| activity_item(core, record))
            .collect(),
        next_cursor,
    })
}

/// Undoes one journal entry: puts back the value the journal recorded before the change. Reports
/// precisely how it ended. An entry that is not applied (already undone, failed) is "nothing to
/// undo", not an error; an unknown entry is `notFound`.
pub fn undo_entry(core: &AppCore, entry: &str) -> Result<UndoResult, IpcError> {
    let id = EntryId(parse_id(entry, "journal entry")?);
    let _exclusive = core.exclusive();
    let found = core
        .journal
        .entry(id)
        .map_err(IpcError::from)?
        .ok_or_else(|| IpcError::NotFound {
            message: format!("there is no journal entry {id}"),
        })?;
    if found.state != EntryState::Applied {
        return Ok(undo_result(core, &RevertReport::default()));
    }
    let failures_before = core.engine.activity_write_failures();
    let result = core
        .engine
        .executor()
        .revert_entry(id, Source::Manual, None);
    warn_if_history_incomplete(core, failures_before, "undoing a change");
    let report = RevertReport {
        outcomes: vec![RevertOutcome { entry: id, result }],
    };
    Ok(undo_result(core, &report))
}

/// Undoes every applied entry of a journal batch, newest first, and carries on past a failure so
/// as much as possible is restored. Reports precisely how it ended; a batch with nothing applied
/// is "nothing to undo".
pub fn undo_batch(core: &AppCore, batch: &str) -> Result<UndoResult, IpcError> {
    let batch = BatchId(parse_id(batch, "batch")?);
    let _exclusive = core.exclusive();
    let entries = core.journal.batch_entries(batch).map_err(IpcError::from)?;
    if !entries
        .iter()
        .any(|entry| entry.state == EntryState::Applied)
    {
        return Ok(undo_result(core, &RevertReport::default()));
    }
    let failures_before = core.engine.activity_write_failures();
    let report = core
        .engine
        .executor()
        .revert_batch(batch, Source::Manual, None);
    warn_if_history_incomplete(core, failures_before, "undoing a batch");
    Ok(undo_result(core, &report.map_err(revert_ipc_error)?))
}

/// What [`undo_batch`] would restore right now, newest first. Reads the journal only, so the
/// confirmation dialog can say exactly what will be put back.
pub fn preview_undo_batch(core: &AppCore, batch: &str) -> Result<UndoPreview, IpcError> {
    let batch = BatchId(parse_id(batch, "batch")?);
    let entries = core.journal.batch_entries(batch).map_err(IpcError::from)?;
    let mut applied: Vec<JournalEntry> = entries
        .into_iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .collect();
    applied.reverse();
    undo_preview(core, &applied)
}

/// What [`undo_entry`] would do right now: the one change, and the later change it would
/// overwrite, if any. An entry that is not applied previews as nothing. Reads the journal only.
pub fn preview_undo_entry(core: &AppCore, entry: &str) -> Result<UndoPreview, IpcError> {
    let id = EntryId(parse_id(entry, "journal entry")?);
    let found = core.journal.entry(id).map_err(IpcError::from)?;
    let applied: Vec<JournalEntry> = found
        .filter(|entry| entry.state == EntryState::Applied)
        .into_iter()
        .collect();
    undo_preview(core, &applied)
}

fn undo_preview(core: &AppCore, applied: &[JournalEntry]) -> Result<UndoPreview, IpcError> {
    let overwrites = overwrites_of(core.journal.as_ref(), applied)
        .map_err(IpcError::from)?
        .into_iter()
        .map(|found| UndoOverwrite {
            entry: found.entry.0.to_string(),
            what: capitalised(&subject(&found.later.operation)),
            later_entry: found.later.id.0.to_string(),
            later_time: found.later.timestamp.to_iso8601(),
            later_source: match found.later.source {
                Source::Manual => ActivitySource::Manual,
                Source::Automation => ActivitySource::Automation,
                _ => ActivitySource::Other,
            },
        })
        .collect();
    let items = applied
        .iter()
        .map(|entry| UndoPreviewItem {
            entry: entry.id.0.to_string(),
            what: capitalised(&subject(&entry.operation)),
            restore: restore_words_for(&entry.operation, &entry.prior),
        })
        .collect();
    Ok(UndoPreview { items, overwrites })
}

/// Clears the activity history. Only the log is touched: the undo journal is not, and the records
/// of changes that can still be undone stay so they remain on the Undo page.
pub fn clear_history(core: &AppCore) -> Result<ClearResult, IpcError> {
    let _exclusive = core.exclusive();
    let removed = core.activity.clear().map_err(IpcError::from)?;
    Ok(ClearResult {
        removed: u32::try_from(removed).unwrap_or(u32::MAX),
    })
}

/// Whether the activity history is complete, for the notice on the Undo page.
pub fn history_status(core: &AppCore) -> Result<HistoryStatus, IpcError> {
    Ok(HistoryStatus {
        incomplete: core.history_incomplete(),
    })
}

/// Renders the whole activity log as text, newest first, for saving or copying. The text holds
/// only what the records hold, so it follows their redaction rule.
pub fn export_activity_text(core: &AppCore) -> Result<ActivityExport, IpcError> {
    let mut records = Vec::new();
    let mut before = None;
    loop {
        let chunk = core
            .activity
            .list(EXPORT_CHUNK, before)
            .map_err(IpcError::from)?;
        let last = chunk.last().map(|record| record.id);
        let complete = chunk.len() < EXPORT_CHUNK;
        records.extend(chunk);
        match last {
            Some(id) if !complete => before = Some(id),
            _ => break,
        }
    }
    Ok(ActivityExport {
        file_name: export_file_name(SystemClock.now()),
        text: render_text(&records),
        records: u32::try_from(records.len()).unwrap_or(u32::MAX),
    })
}

/// The suggested file name of an export made at `now`: `volt-activity-2026-11-10.txt`.
#[must_use]
pub fn export_file_name(now: Timestamp) -> String {
    let iso = now.to_iso8601();
    format!("volt-activity-{}.txt", iso.get(..10).unwrap_or("export"))
}

/// Writes an export to the path the user chose in the save dialog.
pub fn write_export(path: &Path, text: &str) -> Result<(), IpcError> {
    std::fs::write(path, text).map_err(|error| {
        let message = format!("could not write {}: {error}", path.display());
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => IpcError::AccessDenied { message },
            std::io::ErrorKind::NotFound => IpcError::NotFound { message },
            _ => IpcError::Other { message },
        }
    })
}

/// Lists everything that starts with Windows: the Run keys and both Startup folders, each with
/// whether it is enabled. Reads only. A source that cannot be read is listed in `problems`.
pub fn list_startup_entries(core: &AppCore) -> Result<StartupList, IpcError> {
    let manager = core.startup_apps()?;
    let _exclusive = core.exclusive();
    let listing = manager.list();
    Ok(StartupList {
        entries: listing.entries.iter().map(startup_entry_view).collect(),
        problems: listing
            .problems
            .iter()
            .map(|problem| StartupProblemView {
                source: startup_source_view(problem.origin),
                source_label: problem.origin.label().to_owned(),
                reason: problem.reason.to_owned(),
            })
            .collect(),
    })
}

/// Switches a startup entry on or off the way Windows does, through the undo journal. Reports
/// precisely how it ended. Entries that apply to every user are refused with `accessDenied`
/// (they need administrator rights) before anything is touched.
pub fn set_startup_entry_enabled(
    core: &AppCore,
    id: &str,
    enabled: bool,
) -> Result<StartupChangeResult, IpcError> {
    let manager = core.startup_apps()?;
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let result = manager.set_enabled(id, enabled, Source::Manual);
    warn_if_history_incomplete(core, failures_before, "changing a startup entry");
    let change = result?;
    Ok(StartupChangeResult {
        id: change.id,
        name: change.name,
        status: match change.outcome {
            ChangeOutcome::AlreadyInState => StartupChangeStatus::AlreadyInState,
            // `Changed`, and (the enum is non-exhaustive) any outcome this shell does not know
            // yet, which has changed something.
            _ => StartupChangeStatus::Changed,
        },
        enabled: change.state.map(|state| state == StartupState::Enabled),
    })
}

// ---- mapping -------------------------------------------------------------------------------

fn startup_source_view(origin: StartupOrigin) -> StartupSource {
    match origin {
        StartupOrigin::UserRun => StartupSource::UserRun,
        StartupOrigin::MachineRun => StartupSource::MachineRun,
        StartupOrigin::MachineRun32 => StartupSource::MachineRun32,
        StartupOrigin::UserFolder => StartupSource::UserFolder,
        StartupOrigin::CommonFolder => StartupSource::CommonFolder,
        _ => StartupSource::Other,
    }
}

fn startup_entry_view(entry: &StartupEntry) -> StartupEntryView {
    StartupEntryView {
        id: entry.id.clone(),
        name: entry.name.clone(),
        source: startup_source_view(entry.origin),
        source_label: entry.origin.label().to_owned(),
        command: entry.command.clone(),
        publisher: entry.publisher.clone(),
        enabled: entry.state == StartupState::Enabled,
        needs_admin: entry.origin.scope() == StartupScope::Machine,
    }
}

pub(crate) fn risk_view(risk: Risk) -> RiskLevel {
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

/// What the journal says about the entry an activity line links to.
enum EntryLookup {
    /// The line has no link to a journal entry (cleaning and memory actions will be like this).
    NoLink,
    /// The line links to an entry the journal does not hold.
    Missing,
    /// The journal could not be read just now.
    Unreadable,
    /// The entry.
    Found(Box<JournalEntry>),
}

const REASON_NO_RECORD: &str =
    "Volt saved no earlier value for this action, so it cannot restore it.";
const REASON_RECORD_GONE: &str =
    "Its undo record is no longer in the journal, so Volt cannot restore it.";
const REASON_UNREADABLE: &str =
    "The undo journal could not be read just now. Reload the list to try again.";
const REASON_UNCONFIRMED: &str = "Volt has not confirmed that this change happened.";

fn lookup_entry(core: &AppCore, record: &ActivityRecord) -> EntryLookup {
    let Some(id) = record.entry else {
        return EntryLookup::NoLink;
    };
    match core.journal.entry(id) {
        Ok(Some(entry)) => EntryLookup::Found(Box::new(entry)),
        Ok(None) => EntryLookup::Missing,
        Err(error) => {
            log::warn!(
                "the undo journal could not be read for {id}: {}",
                activity::error_class(&error)
            );
            EntryLookup::Unreadable
        }
    }
}

/// The state of an activity line and, for `cannotUndo`, the one line saying why. A pure function
/// of the record's outcome and what the journal says about its entry.
fn change_state(
    outcome: ActivityOutcome,
    lookup: &EntryLookup,
) -> (ChangeState, Option<&'static str>) {
    match outcome {
        ActivityOutcome::Failed => (ChangeState::Failed, None),
        ActivityOutcome::Reverted | ActivityOutcome::RolledBack => (ChangeState::Restored, None),
        // `Applied`, and (the enum is non-exhaustive) any outcome this shell does not know yet,
        // which is judged by the journal like an applied change.
        _ => match lookup {
            EntryLookup::NoLink => (ChangeState::CannotUndo, Some(REASON_NO_RECORD)),
            EntryLookup::Missing => (ChangeState::CannotUndo, Some(REASON_RECORD_GONE)),
            EntryLookup::Unreadable => (ChangeState::CannotUndo, Some(REASON_UNREADABLE)),
            EntryLookup::Found(entry) => match entry.state {
                EntryState::Applied => (ChangeState::CanUndo, None),
                EntryState::Reverted => (ChangeState::Undone, None),
                EntryState::Failed => (ChangeState::Failed, None),
                // Not something Volt will offer to undo until startup has resolved it.
                EntryState::Pending => (ChangeState::CannotUndo, Some(REASON_UNCONFIRMED)),
            },
        },
    }
}

fn activity_item(core: &AppCore, record: &ActivityRecord) -> ActivityItem {
    let lookup = lookup_entry(core, record);
    let (state, reason) = change_state(record.outcome, &lookup);
    // The value before the change belongs to a change that was made.
    let previous = match (&lookup, record.outcome) {
        (EntryLookup::Found(entry), ActivityOutcome::Applied) => {
            Some(activity::redacted_prior_for(&entry.operation, &entry.prior))
        }
        _ => None,
    };
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
        title: record.label.as_deref().and_then(|label| {
            if label == STARTUP_LABEL {
                return Some(STARTUP_TITLE.to_owned());
            }
            if let Some(id) = label.strip_suffix(RESET_LABEL_SUFFIX) {
                return core
                    .engine
                    .catalogue()
                    .get(id)
                    .map(|tweak| format!("{} (set to the Windows default)", tweak.title));
            }
            core.engine
                .catalogue()
                .get(label)
                .map(|tweak| tweak.title.clone())
        }),
        label: record.label.clone(),
        batch: record.batch.map(|batch| batch.0.to_string()),
        entry: record.entry.map(|entry| entry.0.to_string()),
        previous,
        state,
        reason: reason.map(str::to_owned),
    }
}

/// Parses the decimal-string id the UI sends back.
fn parse_id(text: &str, what: &str) -> Result<u64, IpcError> {
    text.parse::<u64>().map_err(|_| IpcError::InvalidInput {
        message: format!("`{text}` is not a {what} id"),
    })
}

/// `registry value X` becomes `Registry value X`.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The error of a revert that could not even start (the journal could not be read).
fn revert_ipc_error(error: RevertError) -> IpcError {
    match error {
        RevertError::Journal(port) | RevertError::Port(port) | RevertError::MarkReverted(port) => {
            IpcError::from(port)
        }
        other => IpcError::Other {
            message: revert_reason(&other),
        },
    }
}

/// Counts what a revert did and names what it could not restore.
fn undo_result(core: &AppCore, report: &RevertReport) -> UndoResult {
    let mut restored = 0_usize;
    let mut failures = Vec::new();
    for outcome in &report.outcomes {
        match &outcome.result {
            Ok(()) => restored += 1,
            Err(error) => {
                let what = match core.journal.entry(outcome.entry) {
                    Ok(Some(entry)) => capitalised(&subject(&entry.operation)),
                    _ => "A change".to_owned(),
                };
                failures.push(UndoFailure {
                    entry: outcome.entry.0.to_string(),
                    what,
                    reason: revert_reason(error),
                });
            }
        }
    }
    let status = match (restored, failures.len()) {
        (0, 0) => UndoStatus::NothingToUndo,
        (_, 0) => UndoStatus::Restored,
        (0, _) => UndoStatus::Failed,
        _ => UndoStatus::Partial,
    };
    UndoResult {
        status,
        restored: u32::try_from(restored).unwrap_or(u32::MAX),
        failed: u32::try_from(failures.len()).unwrap_or(u32::MAX),
        failures,
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
        core.note_history_incomplete();
        log::warn!(
            "{failures} activity record(s) could not be written while {action}; the change and \
             its undo journal entry are safe, but the history on the Undo page is incomplete"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use volt_core::{Operation, PriorState, RegistryHive, RegistryValue, RegistryValueRef};

    fn entry(state: EntryState) -> EntryLookup {
        EntryLookup::Found(Box::new(JournalEntry {
            id: EntryId(1),
            batch: BatchId(1),
            timestamp: Timestamp::from_unix_millis(1),
            source: Source::Manual,
            state,
            operation: Operation::SetRegistryValue {
                target: RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\VoltTest", "X"),
                value: RegistryValue::Dword(1),
            },
            prior: PriorState::RegistryValue(None),
            error: None,
        }))
    }

    #[test]
    fn an_applied_record_takes_its_state_from_the_journal_entry() {
        let state = |lookup| change_state(ActivityOutcome::Applied, &lookup);
        assert_eq!(
            state(entry(EntryState::Applied)),
            (ChangeState::CanUndo, None)
        );
        assert_eq!(
            state(entry(EntryState::Reverted)),
            (ChangeState::Undone, None)
        );
        assert_eq!(
            state(entry(EntryState::Failed)),
            (ChangeState::Failed, None)
        );
        assert_eq!(
            state(entry(EntryState::Pending)),
            (ChangeState::CannotUndo, Some(REASON_UNCONFIRMED))
        );
        assert_eq!(
            state(EntryLookup::NoLink),
            (ChangeState::CannotUndo, Some(REASON_NO_RECORD))
        );
        assert_eq!(
            state(EntryLookup::Missing),
            (ChangeState::CannotUndo, Some(REASON_RECORD_GONE))
        );
        assert_eq!(
            state(EntryLookup::Unreadable),
            (ChangeState::CannotUndo, Some(REASON_UNREADABLE))
        );
    }

    #[test]
    fn failures_and_undo_records_never_offer_an_undo_whatever_the_journal_says() {
        for lookup in [entry(EntryState::Applied), EntryLookup::NoLink] {
            assert_eq!(
                change_state(ActivityOutcome::Failed, &lookup),
                (ChangeState::Failed, None)
            );
            assert_eq!(
                change_state(ActivityOutcome::Reverted, &lookup),
                (ChangeState::Restored, None)
            );
            assert_eq!(
                change_state(ActivityOutcome::RolledBack, &lookup),
                (ChangeState::Restored, None)
            );
        }
    }

    #[test]
    fn ids_must_be_decimal_strings_and_text_starts_with_a_capital() {
        assert_eq!(parse_id("42", "batch").ok(), Some(42));
        assert!(parse_id("-1", "batch").is_err());
        assert!(parse_id("0x2a", "batch").is_err());
        assert_eq!(capitalised("registry value X"), "Registry value X");
        assert_eq!(capitalised(""), "");
    }
}
