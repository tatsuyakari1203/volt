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
use volt_core::activity::{
    exec_phrase, restore_phrase_for, restore_words_for, revert_phrase, revert_reason, subject,
    subject_phrase,
};
use volt_core::startup::STARTUP_LABEL;
use volt_core::{
    ActivityId, ActivityOutcome, ActivityRecord, ApplyOutcome, ApprovalRecord, BatchId, Category,
    ChangeOutcome, Clock, EntryId, EntryState, Evidence, ItemStatus, JournalEntry, MEMORY_LABEL,
    PROCESS_END_LABEL, Phrase, RESET_LABEL_SUFFIX, RestartRequirement, RevertError, RevertOutcome,
    RevertReport, Risk, Source, StartupEntry, StartupLock, StartupOrigin, StartupProtection,
    StartupScope, StartupState, StepPreview, SystemClock, TUNE_LABEL, Timestamp, TweakApplyReport,
    TweakError, TweakRevertReport, TweakState, activity, overwrites_of, render_text,
};

use crate::IpcError;
use crate::ipc::ErrorClass;
use crate::phrase::PhraseView;
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
        /// Why, in words (English).
        message: String,
        /// Why, as a code and parameters, for the window to write in its language.
        phrase: Option<PhraseView>,
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
    /// `subject` as a code and parameters.
    pub subject_phrase: PhraseView,
    /// `current` as a code and parameters.
    pub current_phrase: PhraseView,
    /// `target` as a code and parameters.
    pub target_phrase: PhraseView,
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
    /// The administrator prompt of Windows was cancelled. Nothing was changed.
    Declined,
    /// The helper's own confirmation was declined or timed out. Nothing was changed.
    NotConfirmed,
    /// The same request was declined less than 10 minutes ago (or confirmations are locked for
    /// this session), so it was refused without asking. Nothing was changed.
    Blocked,
    /// The tweak needs a restore point first and Windows could not create one. Nothing was
    /// changed.
    RestorePointFailed,
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
    /// `detail` as a code and parameters, for the window to write in its language.
    pub detail_phrase: Option<PhraseView>,
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
    /// The administrator prompt of Windows was cancelled. Nothing was changed.
    Declined,
    /// The helper's own confirmation was declined or timed out. Nothing was changed.
    NotConfirmed,
    /// The same request was declined lately, so it was refused without asking. Nothing was
    /// changed.
    Blocked,
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
    /// `detail` as a code and parameters, for the window to write in its language.
    pub detail_phrase: Option<PhraseView>,
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
    /// One redacted sentence: names the target and the outcome, never data. English: the text of
    /// `summary_phrase`, or the only text of a record written before phrases existed.
    pub summary: String,
    /// The summary as a code and parameters, for the UI to write in its own language. `null` for
    /// a record written before phrases existed.
    pub summary_phrase: Option<PhraseView>,
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
    /// `previous` as a code and parameters.
    pub previous_phrase: Option<PhraseView>,
    /// Whether this line can be undone, was undone, failed and so on.
    pub state: ChangeState,
    /// One line saying why the change cannot be undone, when `state` is `cannotUndo`. English.
    pub reason: Option<String>,
    /// `reason` as a code and parameters.
    pub reason_phrase: Option<PhraseView>,
    /// Whether the elevated helper made this change (it is mirrored from the helper's own log):
    /// the row carries an Administrator badge and is undone through the helper.
    pub administrator: bool,
    /// For an administrator row, the tweak id to pass to `undoAdministratorChange`.
    pub administrator_tweak: Option<String>,
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
    /// Nothing was changed: the undo would replace a later change the caller has not
    /// acknowledged. `overwrites` lists them; call again with their `laterEntry` ids to go ahead.
    NeedsAcknowledgement,
    /// The administrator prompt of Windows was cancelled. Nothing was changed.
    Declined,
    /// The helper's own confirmation was declined or timed out. Nothing was changed.
    NotConfirmed,
    /// The same request was declined lately, so it was refused without asking. Nothing was
    /// changed.
    Blocked,
}

/// One change an undo could not restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoFailure {
    /// The journal entry, as a decimal string.
    pub entry: String,
    /// What it touches, in words, without any data.
    pub what: String,
    /// `what` as a code and parameters, without the capital letter a sentence starts with.
    pub what_phrase: Option<PhraseView>,
    /// Why it could not be restored, in words. Never contains command output or data.
    pub reason: String,
    /// `reason` as a code and parameters.
    pub reason_phrase: Option<PhraseView>,
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
    /// The later changes the undo would overwrite that were not acknowledged; filled only when
    /// `status` is `needsAcknowledgement`.
    pub overwrites: Vec<UndoOverwrite>,
}

/// One change an undo of a batch would restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UndoPreviewItem {
    /// The journal entry, as a decimal string.
    pub entry: String,
    /// What it touches, in words, without any data.
    pub what: String,
    /// `what` as a code and parameters, without the capital letter a sentence starts with.
    pub what_phrase: PhraseView,
    /// What undoing does to it, redacted: `set back to REG_DWORD 1`, `removed (it did not exist
    /// before)`.
    pub restore: String,
    /// `restore` as a code and parameters.
    pub restore_phrase: PhraseView,
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
    /// `what` as a code and parameters, without the capital letter a sentence starts with.
    pub what_phrase: PhraseView,
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
    /// Records left in the history because their change can still be undone.
    pub kept: u32,
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

/// What the Undo page calls a memory cleaning run (the Memory page, Boost and the tray).
const MEMORY_TITLE: &str = "Memory cleaning";

/// What the Undo page calls ending apps from the Processes page.
const PROCESS_END_TITLE: &str = "Ended apps";

/// What the Undo page calls a change of priority, cores or Efficiency mode of an app.
const PROCESS_TUNE_TITLE: &str = "Process settings";

/// What the Undo page calls emptying the Recycle Bin.
const RECYCLE_BIN_TITLE: &str = "Recycle Bin";

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
    /// The current user's `RunOnce` key.
    UserRunOnce,
    /// The machine-wide `RunOnce` key.
    MachineRunOnce,
    /// The machine-wide `RunOnce` key of 32-bit programs.
    MachineRunOnce32,
    /// A source this version of the UI does not know.
    Other,
}

/// How a startup item is protected. Mirrors [`volt_core::StartupProtection`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StartupProtectionView {
    /// Not protected.
    None,
    /// Microsoft or Windows itself publishes it: locked until Windows items are unlocked.
    Microsoft,
    /// Windows cannot run without it: never switchable.
    Critical,
}

/// Why an item has its switch locked. Mirrors [`volt_core::StartupLock`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StartupLockView {
    /// Windows needs it: never switchable.
    Critical,
    /// It applies to every user: changing it needs administrator rights.
    NeedsAdministrator,
    /// Changing it needs the administrator helper.
    NeedsHelper,
    /// Volt cannot change this kind of item yet.
    NotYet,
    /// Published by Microsoft: locked until the user unlocks Windows items.
    Microsoft,
}

/// Whether the `StartupApproved` flag decides if an entry runs. Mirrors
/// [`volt_core::ApprovalRecord`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum StartupApprovalView {
    /// Windows holds a record that says whether the entry runs.
    Recorded,
    /// There is no record: Windows runs the entry.
    Missing,
    /// `StartupApproved` does not apply to this kind of entry.
    NotUsed,
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
    /// How it is protected.
    pub protection: StartupProtectionView,
    /// Why the switch is locked; `null` when it can be switched now. A `microsoft` lock is lifted
    /// by unlocking Windows items.
    pub lock: Option<StartupLockView>,
    /// Whether the `StartupApproved` flag decides if it runs.
    pub approval: StartupApprovalView,
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

/// A scheduled task that runs when you sign in. Read-only: it has no switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTaskView {
    /// The task's name.
    pub name: String,
    /// The folder of the task in the Task Scheduler library.
    pub folder: String,
    /// The command (program and arguments); empty when the task runs no program.
    pub command: String,
    /// Who published the program, when its version information says.
    pub publisher: Option<String>,
    /// Whether the task will run at sign-in.
    pub enabled: bool,
    /// Whether the task is part of Windows.
    pub microsoft: bool,
    /// How it is protected.
    pub protection: StartupProtectionView,
    /// Why it has no switch.
    pub lock: StartupLockView,
}

/// A Windows service that starts automatically. Read-only here: changing it needs the
/// administrator helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ServiceView {
    /// The service name.
    pub name: String,
    /// The display name.
    pub display_name: String,
    /// The command line as written.
    pub command: String,
    /// Who published the program, when its version information says.
    pub publisher: Option<String>,
    /// Whether the start is delayed until after sign-in has settled.
    pub delayed: bool,
    /// The account it runs as; empty for the default.
    pub account: String,
    /// How it is protected.
    pub protection: StartupProtectionView,
    /// Why it has no switch.
    pub lock: StartupLockView,
}

/// Everything that starts with Windows, read just now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct StartupList {
    /// The entries, sorted by name.
    pub entries: Vec<StartupEntryView>,
    /// Sources that could not be read.
    pub problems: Vec<StartupProblemView>,
    /// Scheduled tasks that run at sign-in, read-only, sorted by name.
    pub tasks: Vec<ScheduledTaskView>,
    /// Why the scheduled tasks could not be read ("access denied", ...), if so.
    pub tasks_problem: Option<String>,
    /// `RunOnce` entries: they run once at the next sign-in. The user's own can be switched.
    pub once: Vec<StartupEntryView>,
    /// Services that start automatically, read-only, sorted by display name.
    pub services: Vec<ServiceView>,
    /// Why the services could not be read ("access denied", ...), if so.
    pub services_problem: Option<String>,
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
    let mirror = crate::administrator::load_mirror(core);
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
            applied_by_volt: if tweak.requires_admin && core.has_elevator() {
                // An administrator change lives in the helper's journal, which the UI mirrors.
                mirror.can_undo(&tweak.id)
            } else {
                core.engine.applied_by_volt(&tweak.id).unwrap_or(false)
            },
        })
        .collect();
    Ok(infos)
}

/// Applies a tweak as the user, through the journal. Reports precisely how it ended.
pub fn apply_tweak(core: &AppCore, id: &str) -> Result<ApplyResult, IpcError> {
    if crate::administrator::through_helper(core, id) {
        return crate::administrator::apply(core, id);
    }
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
        detail: detail.as_ref().map(Phrase::english),
        detail_phrase: detail.as_ref().map(PhraseView::from),
        state: state_view(core.engine.state(id)),
    })
}

/// Reverts what Volt applied for a tweak, restoring the state the journal recorded. Reports
/// whether the revert was complete. Nothing to undo is a result, not an error.
pub fn revert_tweak(core: &AppCore, id: &str) -> Result<RevertResult, IpcError> {
    if crate::administrator::through_helper(core, id) {
        return crate::administrator::revert(core, id);
    }
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
        detail: detail.as_ref().map(Phrase::english),
        detail_phrase: detail.as_ref().map(PhraseView::from),
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
                subject_phrase: (&item.preview.subject_phrase).into(),
                current_phrase: (&item.preview.current_phrase).into(),
                target_phrase: (&item.preview.target_phrase).into(),
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

/// Turns off a tweak Volt did not apply by putting each step back to its Windows default.
/// Journaled. Refused for a tweak Volt applied itself: undo is the way back for that one.
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
        detail: detail.as_ref().map(Phrase::english),
        detail_phrase: detail.as_ref().map(PhraseView::from),
        state: state_view(core.engine.state(id)),
    })
}

pub(crate) fn step_view(step: StepPreview) -> TweakStepView {
    TweakStepView {
        subject: step.preview.subject,
        current: step.preview.current,
        target: step.preview.target,
        subject_phrase: (&step.preview.subject_phrase).into(),
        current_phrase: (&step.preview.current_phrase).into(),
        target_phrase: (&step.preview.target_phrase).into(),
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
                .map_err(|_| IpcError::invalid(&Phrase::new("err.cursor").text("text", text)))
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
    let mut items: Vec<ActivityItem> = records
        .iter()
        .map(|record| activity_item(core, record))
        .collect();
    if before.is_none() {
        // The helper's rows join the first page, by time. A session that is open is pulled first.
        crate::administrator::pull_if_open(core);
        items.extend(crate::administrator::rows(core));
        items.sort_by(|a, b| b.time.cmp(&a.time));
    }
    Ok(ActivityPage { items, next_cursor })
}

/// Undoes one journal entry: puts back the value the journal recorded before the change. Reports
/// precisely how it ended. An entry that is not applied (already undone, failed) is "nothing to
/// undo", not an error; an unknown entry is `notFound`.
///
/// An undo that would overwrite a later change is refused (`needsAcknowledgement`, nothing
/// changed) unless `acknowledged` holds the `laterEntry` id of each such change.
pub fn undo_entry(
    core: &AppCore,
    entry: &str,
    acknowledged: &[String],
) -> Result<UndoResult, IpcError> {
    let id = EntryId(parse_id(entry)?);
    let _exclusive = core.exclusive();
    let found = core
        .journal
        .entry(id)
        .map_err(IpcError::from)?
        .ok_or_else(|| {
            IpcError::not_found(&Phrase::new("err.noEntry").text("id", id.0.to_string()))
        })?;
    if found.state != EntryState::Applied {
        return Ok(undo_result(core, &RevertReport::default()));
    }
    if let Some(refused) = unacknowledged(core, std::slice::from_ref(&found), acknowledged)? {
        return Ok(refused);
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
/// is "nothing to undo". Refused like [`undo_entry`] when a later change would be overwritten and
/// not acknowledged.
pub fn undo_batch(
    core: &AppCore,
    batch: &str,
    acknowledged: &[String],
) -> Result<UndoResult, IpcError> {
    let batch = BatchId(parse_id(batch)?);
    let _exclusive = core.exclusive();
    let entries = core.journal.batch_entries(batch).map_err(IpcError::from)?;
    if !entries
        .iter()
        .any(|entry| entry.state == EntryState::Applied)
    {
        return Ok(undo_result(core, &RevertReport::default()));
    }
    let applied: Vec<JournalEntry> = entries
        .into_iter()
        .filter(|entry| entry.state == EntryState::Applied)
        .collect();
    if let Some(refused) = unacknowledged(core, &applied, acknowledged)? {
        return Ok(refused);
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
    let batch = BatchId(parse_id(batch)?);
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
    let id = EntryId(parse_id(entry)?);
    let found = core.journal.entry(id).map_err(IpcError::from)?;
    let applied: Vec<JournalEntry> = found
        .filter(|entry| entry.state == EntryState::Applied)
        .into_iter()
        .collect();
    undo_preview(core, &applied)
}

fn overwrite_items(
    core: &AppCore,
    applied: &[JournalEntry],
) -> Result<Vec<UndoOverwrite>, IpcError> {
    Ok(overwrites_of(core.journal.as_ref(), applied)
        .map_err(IpcError::from)?
        .into_iter()
        .map(|found| UndoOverwrite {
            entry: found.entry.0.to_string(),
            what: capitalised(&subject(&found.later.operation)),
            what_phrase: (&subject_phrase(&found.later.operation)).into(),
            later_entry: found.later.id.0.to_string(),
            later_time: found.later.timestamp.to_iso8601(),
            later_source: match found.later.source {
                Source::Manual => ActivitySource::Manual,
                Source::Automation => ActivitySource::Automation,
                _ => ActivitySource::Other,
            },
        })
        .collect())
}

/// The refusal for an undo of `applied` that would overwrite a later change the caller did not
/// acknowledge; `None` when it may go ahead.
fn unacknowledged(
    core: &AppCore,
    applied: &[JournalEntry],
    acknowledged: &[String],
) -> Result<Option<UndoResult>, IpcError> {
    let missing: Vec<UndoOverwrite> = overwrite_items(core, applied)?
        .into_iter()
        .filter(|found| !acknowledged.contains(&found.later_entry))
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }
    Ok(Some(UndoResult {
        status: UndoStatus::NeedsAcknowledgement,
        restored: 0,
        failed: 0,
        failures: Vec::new(),
        overwrites: missing,
    }))
}

fn undo_preview(core: &AppCore, applied: &[JournalEntry]) -> Result<UndoPreview, IpcError> {
    let overwrites = overwrite_items(core, applied)?;
    let items = applied
        .iter()
        .map(|entry| UndoPreviewItem {
            entry: entry.id.0.to_string(),
            what: capitalised(&subject(&entry.operation)),
            what_phrase: (&subject_phrase(&entry.operation)).into(),
            restore: restore_words_for(&entry.operation, &entry.prior),
            restore_phrase: (&restore_phrase_for(&entry.operation, &entry.prior)).into(),
        })
        .collect();
    Ok(UndoPreview { items, overwrites })
}

/// Clears the activity history. Only the log is touched: the undo journal is not, and the records
/// of changes that can still be undone stay so they remain on the Undo page.
pub fn clear_history(core: &AppCore) -> Result<ClearResult, IpcError> {
    let _exclusive = core.exclusive();
    let removed = core.activity.clear().map_err(IpcError::from)?
        + crate::administrator::clear_rows(core) as u64;
    // What is left is exactly what was kept: records of changes that can still be undone.
    let mut kept = 0_usize;
    let mut before = None;
    loop {
        let chunk = core
            .activity
            .list(EXPORT_CHUNK, before)
            .map_err(IpcError::from)?;
        kept += chunk.len();
        match chunk.last() {
            Some(last) if chunk.len() == EXPORT_CHUNK => before = Some(last.id),
            _ => break,
        }
    }
    kept += crate::administrator::row_count(core);
    Ok(ClearResult {
        removed: u32::try_from(removed).unwrap_or(u32::MAX),
        kept: u32::try_from(kept).unwrap_or(u32::MAX),
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
            std::io::ErrorKind::PermissionDenied => {
                IpcError::plain(ErrorClass::AccessDenied, message)
            }
            std::io::ErrorKind::NotFound => IpcError::plain(ErrorClass::NotFound, message),
            _ => IpcError::plain(ErrorClass::Other, message),
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
        tasks: listing
            .tasks
            .iter()
            .map(|task| ScheduledTaskView {
                name: task.name.clone(),
                folder: task.folder.clone(),
                command: task.command.clone(),
                publisher: task.publisher.clone(),
                enabled: task.enabled,
                microsoft: task.microsoft,
                protection: protection_view(task.protection),
                lock: lock_view(task.lock()),
            })
            .collect(),
        tasks_problem: listing.tasks_problem.map(str::to_owned),
        once: listing.once.iter().map(startup_entry_view).collect(),
        services: listing
            .services
            .iter()
            .map(|service| ServiceView {
                name: service.name.clone(),
                display_name: service.display_name.clone(),
                command: service.command.clone(),
                publisher: service.publisher.clone(),
                delayed: service.delayed,
                account: service.account.clone(),
                protection: protection_view(service.protection),
                lock: lock_view(service.lock()),
            })
            .collect(),
        services_problem: listing.services_problem.map(str::to_owned),
    })
}

/// Switches a startup entry on or off the way Windows does, through the undo journal. Reports
/// precisely how it ended. Entries that apply to every user are refused with `accessDenied`
/// (they need administrator rights) before anything is touched, and so are the entries Microsoft
/// publishes unless `unlocked` is set (the user chose to unlock Windows items) and those Windows
/// cannot run without whatever `unlocked` says.
pub fn set_startup_entry_enabled(
    core: &AppCore,
    id: &str,
    enabled: bool,
    unlocked: bool,
) -> Result<StartupChangeResult, IpcError> {
    let manager = core.startup_apps()?;
    let _exclusive = core.exclusive();
    let failures_before = core.engine.activity_write_failures();
    let result = manager.set_enabled_with(id, enabled, Source::Manual, unlocked);
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
        StartupOrigin::UserRunOnce => StartupSource::UserRunOnce,
        StartupOrigin::MachineRunOnce => StartupSource::MachineRunOnce,
        StartupOrigin::MachineRunOnce32 => StartupSource::MachineRunOnce32,
        _ => StartupSource::Other,
    }
}

fn protection_view(protection: StartupProtection) -> StartupProtectionView {
    match protection {
        StartupProtection::None => StartupProtectionView::None,
        StartupProtection::Microsoft => StartupProtectionView::Microsoft,
        StartupProtection::Critical => StartupProtectionView::Critical,
    }
}

fn lock_view(lock: StartupLock) -> StartupLockView {
    match lock {
        StartupLock::Critical => StartupLockView::Critical,
        StartupLock::NeedsAdministrator => StartupLockView::NeedsAdministrator,
        StartupLock::NeedsHelper => StartupLockView::NeedsHelper,
        StartupLock::NotYet => StartupLockView::NotYet,
        StartupLock::Microsoft => StartupLockView::Microsoft,
    }
}

fn approval_view(approval: ApprovalRecord) -> StartupApprovalView {
    match approval {
        ApprovalRecord::Recorded => StartupApprovalView::Recorded,
        ApprovalRecord::Missing => StartupApprovalView::Missing,
        ApprovalRecord::NotUsed => StartupApprovalView::NotUsed,
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
        protection: protection_view(entry.protection),
        lock: entry.lock().map(lock_view),
        approval: approval_view(entry.approval),
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

pub(crate) fn state_view(state: Result<TweakState, TweakError>) -> TweakStateView {
    match state {
        Ok(TweakState::Applied) => TweakStateView::Applied,
        Ok(TweakState::NotApplied) => TweakStateView::NotApplied,
        Ok(TweakState::Partial) => TweakStateView::Partial,
        Err(error) => {
            let error = IpcError::from(error);
            TweakStateView::Unreadable {
                message: error.message().to_owned(),
                phrase: error.phrase().cloned(),
            }
        }
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

// The phrase codes of the reasons a line cannot be undone (their English text is in the core).
const REASON_NO_RECORD: &str = "undo.noRecord";
const REASON_RECORD_GONE: &str = "undo.recordGone";
const REASON_UNREADABLE: &str = "undo.unreadable";
const REASON_UNCONFIRMED: &str = "undo.unconfirmed";
const REASON_ACCEPTED: &str = "undo.accepted";

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
                // The user kept the state something else set; restoring would replace it.
                EntryState::Accepted => (ChangeState::CannotUndo, Some(REASON_ACCEPTED)),
            },
        },
    }
}

/// What the Undo page calls a record, from its label: a tweak, a startup change, a cleaning rule,
/// a memory run or ending apps. `None` for a label nothing knows (the page then shows the summary).
fn activity_title(core: &AppCore, label: &str) -> Option<String> {
    match label {
        STARTUP_LABEL => return Some(STARTUP_TITLE.to_owned()),
        MEMORY_LABEL => return Some(MEMORY_TITLE.to_owned()),
        PROCESS_END_LABEL => return Some(PROCESS_END_TITLE.to_owned()),
        TUNE_LABEL => return Some(PROCESS_TUNE_TITLE.to_owned()),
        "recycle-bin" => return Some(RECYCLE_BIN_TITLE.to_owned()),
        _ => {}
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
        .or_else(|| {
            core.clean_engine()
                .and_then(|engine| engine.rules().get(label))
                .map(|rule| format!("Cleaning: {}", rule.title))
        })
}

fn activity_item(core: &AppCore, record: &ActivityRecord) -> ActivityItem {
    let lookup = lookup_entry(core, record);
    let (state, reason) = change_state(record.outcome, &lookup);
    // The value before the change belongs to a change that was made.
    let previous_phrase = match (&lookup, record.outcome) {
        (EntryLookup::Found(entry), ActivityOutcome::Applied) => {
            Some(activity::prior_phrase_for(&entry.operation, &entry.prior))
        }
        _ => None,
    };
    let previous = previous_phrase.as_ref().map(volt_core::Phrase::english);
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
        summary_phrase: record.phrase.as_ref().map(PhraseView::from),
        title: record
            .label
            .as_deref()
            .and_then(|label| activity_title(core, label)),
        label: record.label.clone(),
        batch: record.batch.map(|batch| batch.0.to_string()),
        entry: record.entry.map(|entry| entry.0.to_string()),
        previous,
        previous_phrase: previous_phrase.as_ref().map(PhraseView::from),
        state,
        reason: reason.map(|code| Phrase::new(code).english()),
        reason_phrase: reason.map(|code| (&Phrase::new(code)).into()),
        administrator: false,
        administrator_tweak: None,
    }
}

/// Parses the decimal-string id the UI sends back.
fn parse_id(text: &str) -> Result<u64, IpcError> {
    text.parse::<u64>()
        .map_err(|_| IpcError::invalid(&Phrase::new("err.badId").text("text", text)))
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
        other => IpcError::other(&revert_phrase(&other)),
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
                let operation = match core.journal.entry(outcome.entry) {
                    Ok(Some(entry)) => Some(entry.operation),
                    _ => None,
                };
                let what = operation.as_ref().map_or_else(
                    || "A change".to_owned(),
                    |operation| capitalised(&subject(operation)),
                );
                failures.push(UndoFailure {
                    entry: outcome.entry.0.to_string(),
                    what,
                    what_phrase: operation
                        .as_ref()
                        .map(|operation| (&subject_phrase(operation)).into()),
                    reason: revert_reason(error),
                    reason_phrase: Some((&revert_phrase(error)).into()),
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
        overwrites: Vec::new(),
    }
}

/// Why an apply failed, in words (first failed step), and how many applied steps could not be
/// put back. Built from the core's redacted reasons: no port message, no data.
pub(crate) fn apply_failure_detail(report: &TweakApplyReport) -> Option<Phrase> {
    let report = report.report.as_ref()?;
    let mut parts = Vec::new();
    if let Some(reason) = report.failed().find_map(|item| match &item.status {
        ItemStatus::Failed { error, .. } => Some(exec_phrase(error)),
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
        parts.push(Phrase::new("err.detail.notRestored").count("count", not_restored));
    }
    (!parts.is_empty()).then(|| Phrase::new("err.detail").list("parts", "; ", parts))
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

fn revert_failure_detail(report: &TweakRevertReport) -> Option<Phrase> {
    report
        .batches
        .iter()
        .flat_map(|batch| &batch.report.outcomes)
        .find_map(|outcome| outcome.result.as_ref().err().map(revert_phrase))
}

/// Logs a warning when the executor could not write activity records during a call: the change
/// and its journal entry are safe, but the history the Undo page shows is incomplete.
pub(crate) fn warn_if_history_incomplete(core: &AppCore, failures_before: u64, action: &str) {
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
        assert_eq!(parse_id("42").ok(), Some(42));
        assert!(parse_id("-1").is_err());
        assert!(parse_id("0x2a").is_err());
        assert_eq!(capitalised("registry value X"), "Registry value X");
        assert_eq!(capitalised(""), "");
    }
}
