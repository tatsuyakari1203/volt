//! Machine-wide cleaning through the elevated helper, at the IPC boundary.
//!
//! The rules that need administrator rights (Windows Temp, the machine-wide error report folders,
//! the Windows Update logs) cannot be cleaned by the app: it never runs elevated. The helper scans
//! and cleans them (see the decision on the helper), and this module is the app's half:
//!
//! - **A scan** asks the helper for one rule by its id (the first call of a session shows the one
//!   Windows prompt) and shows counts, sizes and what was left out, never a file name. The helper's
//!   scan id stays in Rust ([`MachineScans`]): the window names the rule, never the id.
//! - **A clean** takes that scan, once. The helper deletes exactly what it scanned that is still the
//!   same and old enough, and for a folder a normal user can write to it first asks a person on its
//!   own surface. **Deletion is permanent**: nothing goes to the Recycle Bin, every sentence says so,
//!   and the activity record of the run says it cannot be undone (it has no undo journal entry).
//!
//! Boost and the tray never call this: only the Clean page's shield actions do.
//!
//! Byte and item counts cross as `f64` or `u32`, like the other pages (see [`crate::metrics`]).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::clean::machine::{ApplySkips, CleanApplyReply, CleanScanReply, ScanSkips};
use volt_core::{
    ActivityOutcome, CleaningRule, Clock, ElevationError, ErrorCode, MachineProblem, NewActivity,
    Phrase, Settings, Source, SystemClock,
};

use crate::IpcError;
use crate::cleaner::{count32, count64};
use crate::commands::blocking;
use crate::elevation::ipc_error;
use crate::phrase::PhraseView;
use crate::wiring::{AppCore, AppState};

/// The scan ids the helper gave, by rule id, until the clean that uses them (or a newer scan of
/// the same rule). They live here and never reach the window.
#[derive(Debug, Default)]
pub struct MachineScans {
    ids: Mutex<HashMap<String, String>>,
}

impl MachineScans {
    fn put(&self, rule: &str, id: String) {
        let mut ids = self.ids.lock().unwrap_or_else(PoisonError::into_inner);
        if id.is_empty() {
            ids.remove(rule);
        } else {
            ids.insert(rule.to_owned(), id);
        }
    }

    /// Takes the id: a scan is good for one clean.
    fn take(&self, rule: &str) -> Option<String> {
        self.ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(rule)
    }
}

/// What the helper found for one machine rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MachineScanView {
    /// The rule's id.
    pub rule_id: String,
    /// Files a clean would delete.
    pub files: u32,
    /// Empty folders a clean would delete.
    pub folders: u32,
    /// Size of those files in bytes.
    pub bytes: f64,
    /// The minimum age in days the scan applied (never below the rule's own).
    pub min_age_days: u32,
    /// Whether the scan stopped at a limit: there is more than what is counted.
    pub truncated: bool,
    /// Whether the helper will ask a person on its own surface before it deletes.
    pub confirm: bool,
    /// Whether there is anything to delete now (a clean can be started).
    pub can_clean: bool,
    /// What was left out and why, in a few words, or `null`.
    pub skipped: Option<String>,
    /// `skipped` as a code and parameters.
    pub skipped_phrase: Option<PhraseView>,
    /// Why nothing was scanned, or `null`.
    pub problem: Option<String>,
    /// `problem` as a code and parameters.
    pub problem_phrase: Option<PhraseView>,
}

/// The answer to a machine scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum MachineScanResult {
    /// The helper scanned.
    Done {
        /// What it found.
        scan: MachineScanView,
    },
    /// The user cancelled the administrator prompt. Nothing was scanned.
    Declined,
}

/// The answer to a machine clean.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum MachineCleanResult {
    /// The helper deleted what it could. Permanent: it did not go to the Recycle Bin.
    #[serde(rename_all = "camelCase")]
    Done {
        /// The rule's id.
        rule_id: String,
        /// Files deleted.
        files: u32,
        /// Empty folders deleted.
        folders: u32,
        /// Size of the files deleted, in bytes.
        bytes: f64,
        /// Items left in place.
        skipped_total: u32,
        /// What was left in place and why, in a few words, or `null`.
        skipped: Option<String>,
        /// `skipped` as a code and parameters.
        skipped_phrase: Option<PhraseView>,
        /// Whether the helper stopped before it had looked at everything (its time limit).
        stopped: bool,
        /// The sentence of the activity record, in English.
        message: String,
        /// The same sentence as a code and parameters.
        message_phrase: PhraseView,
        /// Whether the activity record could not be written (the delete itself is unaffected).
        history_incomplete: bool,
    },
    /// The user cancelled the administrator prompt. Nothing was deleted.
    Declined,
    /// The person did not confirm on the helper's own surface (or it could not be shown). Nothing
    /// was deleted.
    NotConfirmed,
    /// A confirmation of this rule was declined lately, so the helper refused without asking.
    /// Nothing was deleted.
    Blocked,
    /// No answer came in time. The helper may still finish: scan again to see what is left.
    TimedOut,
    /// There is no scan to clean (it was used, replaced, or the helper's session ended). Scan again.
    NoScan,
}

fn count_phrase(code: &str, count: u64) -> Phrase {
    Phrase::new(code).count("count", count)
}

/// What a scan left out, in a few words, or `None` when it left out nothing worth saying.
fn scan_skipped(skips: &ScanSkips) -> Option<Phrase> {
    let parts: Vec<Phrase> = [
        (skips.too_new, "scan.tooNew"),
        (skips.age_unknown, "scan.ageUnknown"),
        (skips.user_writable, "machine.userWritable"),
        (skips.pending_restart, "machine.pendingRestart"),
        (skips.access_denied, "scan.accessDenied"),
        (skips.unreadable, "scan.unreadable"),
        (skips.links, "scan.links"),
        (skips.too_deep, "scan.deep"),
        (skips.invalid_name, "scan.oddNames"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, code)| count_phrase(code, count))
    .collect();
    (!parts.is_empty()).then(|| Phrase::new("scan.skipped").list("parts", ", ", parts))
}

/// What a clean left in place, in a few words, or `None` when it left nothing.
fn apply_skipped(skips: &ApplySkips) -> Option<Phrase> {
    let parts: Vec<Phrase> = [
        (skips.locked, "machine.left.locked"),
        (skips.changed, "machine.left.changed"),
        (skips.gone, "machine.left.gone"),
        (skips.hard_links, "machine.left.hardLinks"),
        (skips.not_empty, "machine.left.notEmpty"),
        (skips.links, "scan.links"),
        (skips.user_writable, "machine.left.userWritable"),
        (skips.pending_restart, "machine.pendingRestart"),
        (skips.failed, "machine.left.failed"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, code)| count_phrase(code, count))
    .collect();
    (!parts.is_empty()).then(|| Phrase::new("scan.skipped").list("parts", ", ", parts))
}

fn problem_phrase(problem: MachineProblem) -> Phrase {
    Phrase::new(match problem {
        MachineProblem::RootMissing => "machine.problem.rootMissing",
        MachineProblem::RootRefused => "machine.problem.rootRefused",
        MachineProblem::AccessDenied => "machine.problem.accessDenied",
        MachineProblem::Unreadable => "machine.problem.unreadable",
        MachineProblem::BadAge => "machine.problem.badAge",
        MachineProblem::TimedOut => "machine.problem.timedOut",
        // `MachineProblem` is non-exhaustive: a problem added later degrades to the general one.
        _ => "machine.problem.generic",
    })
}

fn scan_view(rule: &str, reply: &CleanScanReply) -> MachineScanView {
    let skipped = scan_skipped(&reply.skipped);
    let problem = reply.problem.map(problem_phrase);
    MachineScanView {
        rule_id: rule.to_owned(),
        files: count32(usize::try_from(reply.files).unwrap_or(usize::MAX)),
        folders: count32(usize::try_from(reply.folders).unwrap_or(usize::MAX)),
        bytes: count64(reply.bytes),
        min_age_days: reply.min_age_days,
        truncated: reply.truncated,
        confirm: reply.confirm,
        can_clean: !reply.scan.is_empty() && reply.problem.is_none(),
        skipped: skipped.as_ref().map(Phrase::english),
        skipped_phrase: skipped.as_ref().map(PhraseView::from),
        problem: problem.as_ref().map(Phrase::english),
        problem_phrase: problem.as_ref().map(PhraseView::from),
    }
}

/// `3 files` or `3 files and 2 empty folders`, as a phrase.
fn counted(files: u64, folders: u64) -> Phrase {
    let files = Phrase::new("count.file").count("count", files);
    if folders == 0 {
        files
    } else {
        Phrase::new("join.and").phrase("first", files).phrase(
            "second",
            Phrase::new("count.emptyFolder").count("count", folders),
        )
    }
}

/// The sentence of the activity record of a run: what was deleted, that it is permanent and that
/// it cannot be undone.
fn summary_phrase(rule: &str, reply: &CleanApplyReply) -> Phrase {
    Phrase::new("summary.cleanMachine")
        .text("rule", rule)
        .phrase("deleted", counted(reply.files, reply.folders))
        .size("size", reply.bytes)
        .maybe(
            "left",
            (reply.skipped.total() > 0).then(|| {
                Phrase::new("summary.leftItems")
                    .phrase("items", count_phrase("count.item", reply.skipped.total()))
            }),
        )
}

/// The rule when it is one the helper cleans: it needs administrator rights and its deletion is
/// permanent.
fn machine_rule<'a>(core: &'a AppCore, id: &str) -> Result<&'a CleaningRule, IpcError> {
    let engine = core
        .clean_engine()
        .ok_or_else(|| IpcError::unsupported(&Phrase::new("err.cleanerUnavailable")))?;
    engine
        .rules()
        .get(id)
        .filter(|rule| rule.requires_admin && rule.permanent)
        .ok_or_else(|| IpcError::invalid(&Phrase::new("err.machine.notMachineRule").text("id", id)))
}

/// The minimum age in days the user set for this rule (temp-like rules have a setting), or `None`.
/// The helper takes the higher of this and the catalogue's own.
fn requested_age(settings: &dyn Settings, rule: &CleaningRule) -> Option<u32> {
    let key = rule.min_age_setting.as_deref()?;
    settings
        .get(key)
        .ok()
        .flatten()
        .and_then(|value| value.as_u64())
        .and_then(|days| u32::try_from(days).ok())
}

/// Scans the machine rule `rule_id` in the helper (one Windows prompt when no session is open).
/// Read-only.
///
/// # Errors
/// `invalidInput` for a rule that is not a machine rule, `unsupported` and the other helper errors
/// when administrator actions cannot be used.
pub fn scan_machine_rule_of(core: &AppCore, rule_id: &str) -> Result<MachineScanResult, IpcError> {
    let rule = machine_rule(core, rule_id)?;
    let age = requested_age(core.settings.as_ref(), rule);
    match core.elevator()?.clean_scan(rule_id, age) {
        Ok(reply) => {
            core.machine_scans().put(rule_id, reply.scan.clone());
            Ok(MachineScanResult::Done {
                scan: scan_view(rule_id, &reply),
            })
        }
        Err(ElevationError::Declined) => Ok(MachineScanResult::Declined),
        Err(ElevationError::Refused(ErrorCode::UnknownRule)) => Err(IpcError::invalid(
            &Phrase::new("err.machine.notMachineRule").text("id", rule_id),
        )),
        Err(error) => Err(ipc_error(&error)),
    }
}

/// Deletes, permanently, what the last scan of `rule_id` found, and writes the activity record of
/// the run (a record that cannot be undone).
///
/// # Errors
/// `invalidInput` for a rule that is not a machine rule and the helper errors that are not
/// outcomes of their own.
pub fn clean_machine_rule_of(
    core: &AppCore,
    rule_id: &str,
) -> Result<MachineCleanResult, IpcError> {
    machine_rule(core, rule_id)?;
    let Some(scan) = core.machine_scans().take(rule_id) else {
        return Ok(MachineCleanResult::NoScan);
    };
    // Not under the core's exclusive gate: the helper's confirmation can stay open for as long as
    // the person takes.
    let reply = match core.elevator()?.clean_apply(&scan) {
        Ok(reply) => reply,
        Err(ElevationError::Declined) => return Ok(MachineCleanResult::Declined),
        Err(ElevationError::Refused(ErrorCode::NotConfirmed)) => {
            return Ok(MachineCleanResult::NotConfirmed);
        }
        Err(ElevationError::Refused(ErrorCode::Blocked)) => return Ok(MachineCleanResult::Blocked),
        Err(ElevationError::Refused(ErrorCode::UnknownScan)) => {
            return Ok(MachineCleanResult::NoScan);
        }
        Err(ElevationError::TimedOut) => return Ok(MachineCleanResult::TimedOut),
        Err(error) => return Err(ipc_error(&error)),
    };
    let summary = summary_phrase(rule_id, &reply);
    let outcome = if reply.files + reply.folders > 0 {
        ActivityOutcome::Applied
    } else {
        ActivityOutcome::Failed
    };
    let record = NewActivity::said(
        SystemClock.now(),
        Source::Manual,
        outcome,
        summary.clone(),
        Some(rule_id.to_owned()),
    );
    let history_incomplete = core.activity.record(record).is_err();
    if history_incomplete {
        core.note_history_incomplete();
    }
    let skipped = apply_skipped(&reply.skipped);
    Ok(MachineCleanResult::Done {
        rule_id: rule_id.to_owned(),
        files: count32(usize::try_from(reply.files).unwrap_or(usize::MAX)),
        folders: count32(usize::try_from(reply.folders).unwrap_or(usize::MAX)),
        bytes: count64(reply.bytes),
        skipped_total: count32(usize::try_from(reply.skipped.total()).unwrap_or(usize::MAX)),
        skipped: skipped.as_ref().map(Phrase::english),
        skipped_phrase: skipped.as_ref().map(PhraseView::from),
        stopped: reply.stopped,
        message: summary.english(),
        message_phrase: PhraseView::from(&summary),
        history_incomplete,
    })
}

/// Tauri command: scans a machine-wide cleaning rule as administrator, through the helper. The
/// first call of a session shows one Windows prompt; cancelling it resolves with `declined`.
/// Read-only: nothing is deleted.
#[tauri::command]
#[specta::specta]
pub(crate) async fn scan_machine_rule(
    state: tauri::State<'_, AppState>,
    rule_id: String,
) -> Result<MachineScanResult, IpcError> {
    blocking(&state, move |core| scan_machine_rule_of(core, &rule_id)).await
}

/// Tauri command: deletes, PERMANENTLY, what the last scan of a machine rule found. The UI asks
/// first, in words that say it cannot be undone and does not use the Recycle Bin; for a folder that
/// a normal user can write to the helper asks again on its own surface.
#[tauri::command]
#[specta::specta]
pub(crate) async fn clean_machine_rule(
    state: tauri::State<'_, AppState>,
    rule_id: String,
) -> Result<MachineCleanResult, IpcError> {
    blocking(&state, move |core| clean_machine_rule_of(core, &rule_id)).await
}
