//! App updates at the IPC boundary: what winget offers and installing one update.
//!
//! This uses the network, and only when the page asks: nothing here runs on its own. The rules
//! (the table parser, the fixed argument list, the outcome of an exit code) live in
//! `volt_core::apps`; this module translates them into transfer types. The UI sends back only an
//! id from the last check, never a command. The queue (one update after another, stop after the
//! running one) is the page's: each `update_app` call waits for winget and returns. The
//! administrator prompt, when a package needs one, comes from winget's own installer.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    AppUpdate, Phrase, Source, UpdateError, UpdateListing, UpdateProblem, UpgradeOutcome,
    UpgradeReport,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::phrase::PhraseView;
use crate::wiring::{AppCore, AppState};

/// One update winget offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateView {
    /// The winget package id; pass it to `updateApp`.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// The installed version.
    pub current: String,
    /// The version winget offers.
    pub available: String,
    /// The winget source (`winget`, `msstore`), when known.
    pub source: Option<String>,
}

/// Why no list could be made, with what the user can do about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProblemView {
    /// What went wrong, in English.
    pub message: String,
    /// `message` as a phrase.
    pub message_phrase: PhraseView,
    /// What to do about it, in English.
    pub fix: String,
    /// `fix` as a phrase.
    pub fix_phrase: PhraseView,
    /// Whether the user has to accept winget's source terms (Volt never does).
    pub agreements: bool,
}

/// What a check for updates found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckView {
    /// The updates; empty when there are none or when `problem` is set.
    pub updates: Vec<AppUpdateView>,
    /// Rows winget printed that were cut short and are left out.
    pub skipped: u32,
    /// Why no list could be made.
    pub problem: Option<UpdateProblemView>,
}

/// How updating one app ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum UpdateOutcomeView {
    /// Updated.
    Updated,
    /// Updated; Windows needs a restart to finish.
    NeedsRestart,
    /// Not updated.
    Failed,
}

/// What updating one app did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateResultView {
    /// The package id.
    pub id: String,
    /// Its name.
    pub name: String,
    /// The version before.
    pub from: String,
    /// The version offered.
    pub to: String,
    /// How it ended.
    pub outcome: UpdateOutcomeView,
    /// Why it failed, in English, when it did.
    pub reason: Option<String>,
    /// `reason` as a phrase.
    pub reason_phrase: Option<PhraseView>,
    /// Whether it was written to the activity log (as a change Volt cannot undo).
    pub recorded: bool,
}

fn update_view(update: &AppUpdate) -> AppUpdateView {
    AppUpdateView {
        id: update.id.clone(),
        name: update.name.clone(),
        current: update.current.clone(),
        available: update.available.clone(),
        source: update.source.clone(),
    }
}

fn problem_view(problem: &UpdateProblem) -> UpdateProblemView {
    UpdateProblemView {
        message: problem.phrase().english(),
        message_phrase: PhraseView::from(&problem.phrase()),
        fix: problem.fix().english(),
        fix_phrase: PhraseView::from(&problem.fix()),
        agreements: matches!(problem, UpdateProblem::Agreements),
    }
}

fn result_view(report: &UpgradeReport) -> UpdateResultView {
    let (outcome, reason) = match report.outcome {
        UpgradeOutcome::Updated => (UpdateOutcomeView::Updated, None),
        UpgradeOutcome::NeedsRestart => (UpdateOutcomeView::NeedsRestart, None),
        UpgradeOutcome::Failed(failure) => (UpdateOutcomeView::Failed, Some(failure.phrase())),
        // Anything added later reads as a failure without a reason.
        _ => (UpdateOutcomeView::Failed, None),
    };
    UpdateResultView {
        id: report.id.clone(),
        name: report.name.clone(),
        from: report.from.clone(),
        to: report.to.clone(),
        outcome,
        reason: reason.as_ref().map(Phrase::english),
        reason_phrase: reason.as_ref().map(PhraseView::from),
        recorded: !report.activity_write_failed,
    }
}

impl From<UpdateError> for IpcError {
    fn from(error: UpdateError) -> Self {
        match error {
            UpdateError::NotFound => Self::not_found(&Phrase::new("err.updates.notFound")),
            UpdateError::Busy => Self::invalid(&Phrase::new("err.updates.busy")),
            other => Self::plain(crate::ipc::ErrorClass::InvalidInput, other.to_string()),
        }
    }
}

/// Asks winget for the updates it offers. Uses the network. Reads only: nothing is installed. A
/// problem with winget is part of the answer, not an error.
pub fn check_app_updates_of(core: &AppCore) -> Result<UpdateCheckView, IpcError> {
    Ok(match core.updates()?.check() {
        Ok(UpdateListing { updates, skipped }) => UpdateCheckView {
            updates: updates.iter().map(update_view).collect(),
            skipped,
            problem: None,
        },
        Err(problem) => UpdateCheckView {
            updates: Vec::new(),
            skipped: 0,
            problem: Some(problem_view(&problem)),
        },
    })
}

/// Updates the package `id` of the last check and waits for winget. Recorded in the activity log
/// as a change Volt cannot undo.
pub fn update_app_of(core: &AppCore, id: &str) -> Result<UpdateResultView, IpcError> {
    let report = core.updates()?.upgrade(id, Source::Manual)?;
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    Ok(result_view(&report))
}

/// Tauri command: asks winget which apps have an update. This uses the network and runs only when
/// the user presses the button. Reads only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn check_app_updates(
    state: tauri::State<'_, AppState>,
) -> Result<UpdateCheckView, IpcError> {
    blocking(&state, check_app_updates_of).await
}

/// Tauri command: updates one app with winget and waits for it. A package that needs
/// administrator rights shows the Windows prompt from winget's installer. Volt cannot undo it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn update_app(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<UpdateResultView, IpcError> {
    blocking(&state, move |core| update_app_of(core, &id)).await
}
