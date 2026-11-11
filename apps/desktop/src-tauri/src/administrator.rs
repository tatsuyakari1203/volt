//! Administrator tweaks through the elevated helper, and the helper's history on the Undo page.
//!
//! A tweak that requires administrator rights is applied and reverted by the helper (the app
//! never writes machine-wide settings itself). The helper asks for its own confirmation on a
//! private desktop, keeps its own journal and activity log, and the app mirrors that log by pull
//! ([`volt_core::HelperMirror`]): after each change (the session is open) and whenever the Undo page
//! opens with a session open. Pulling never starts the helper, so looking at the history never
//! shows a Windows prompt.
//!
//! The mirror is saved in the app's settings, so the rows stay on the Undo page when the helper is
//! not running. Without a helper those rows are read-only, with the reason. Undoing a row goes
//! through the helper and restores the earlier value from the helper's journal.
//!
//! This module translates between the core's types and the shell's transfer types (see
//! [`crate::api`]); the rules are the core's and the helper's.

use volt_core::{
    ActivityOutcome, ApplyOutcome, ElevationError, ElevationStatus, ErrorCode, HelperMirror,
    MirrorRow, Phrase, RowState, SettingsExt, Source, UnavailableReason, pull_helper_history,
};

use crate::IpcError;
use crate::api::{
    ActivityItem, ActivitySource, ActivityStatus, ApplyResult, ApplyStatus, ChangeState,
    RevertResult, RevertStatus, UndoResult, UndoStatus, state_view,
};
use crate::commands::blocking;
use crate::elevation::ipc_error;
use crate::phrase::PhraseView;
use crate::wiring::{AppCore, AppState};

/// The settings key of the saved copy of the helper's history.
pub const MIRROR_SETTINGS_KEY: &str = "helper.mirror";

/// Whether a request for this tweak goes through the helper: it requires administrator rights, the
/// app has an elevator and administrator actions can be used now. Otherwise the ordinary path
/// answers (and refuses, for a tweak that needs rights the app does not have).
#[must_use]
pub fn through_helper(core: &AppCore, id: &str) -> bool {
    let administrator = core
        .engine
        .catalogue()
        .get(id)
        .is_some_and(|tweak| tweak.requires_admin);
    administrator
        && core
            .elevator()
            .is_ok_and(|elevator| elevator.status() == ElevationStatus::Available)
}

/// The saved copy of the helper's history, or an empty one when nothing is saved or it cannot be
/// read (a copy that cannot be read is never a reason to fail).
#[must_use]
pub fn load_mirror(core: &AppCore) -> HelperMirror {
    match core.settings.get_typed::<HelperMirror>(MIRROR_SETTINGS_KEY) {
        Ok(Some(mirror)) => mirror,
        Ok(None) => HelperMirror::default(),
        Err(error) => {
            log::warn!("the saved copy of the helper's history cannot be read: {error}");
            HelperMirror::default()
        }
    }
}

fn save_mirror(core: &AppCore, mirror: &HelperMirror) {
    if let Err(error) = core.settings.set_typed(MIRROR_SETTINGS_KEY, mirror) {
        log::warn!("the helper's history could not be saved: {error}");
        core.note_history_incomplete();
    }
}

/// Pulls the helper's log into the saved copy when a session is open. Never starts the helper and
/// never fails: a pull that cannot finish leaves what it got.
pub fn pull_if_open(core: &AppCore) {
    let Ok(elevator) = core.elevator() else {
        return;
    };
    if !elevator.session_open() {
        return;
    }
    let _guard = core.mirror_guard();
    let mut mirror = load_mirror(core);
    let result = pull_helper_history(elevator.as_ref(), &mut mirror);
    if !matches!(result, Ok(0)) {
        save_mirror(core, &mirror);
    }
    if let Err(error) = result {
        log::warn!("the helper's history could not be pulled: {error}");
    }
}

/// How many rows the saved copy holds.
#[must_use]
pub fn row_count(core: &AppCore) -> usize {
    load_mirror(core).rows().len()
}

/// Clears the saved copy the way the Undo page's Clear does: rows that can still be undone stay.
/// Returns how many rows were removed.
pub fn clear_rows(core: &AppCore) -> usize {
    let _guard = core.mirror_guard();
    let mut mirror = load_mirror(core);
    let removed = mirror.clear_rows();
    if removed > 0 {
        save_mirror(core, &mirror);
    }
    removed
}

// ---- apply and revert ------------------------------------------------------------------------

/// Applies the administrator tweak through the helper. The first call of a session shows one
/// Windows prompt; then the helper asks for its own confirmation. Nothing is held while the
/// person decides, so the rest of the app stays usable.
pub fn apply(core: &AppCore, id: &str) -> Result<ApplyResult, IpcError> {
    let answer = core.elevator()?.tweak_apply(id);
    pull_if_open(core);
    let state = || state_view(core.engine.state(id));
    let result = |status, state| ApplyResult {
        tweak: id.to_owned(),
        status,
        detail: None,
        detail_phrase: None,
        state,
    };
    match answer {
        Ok(reply) => Ok(result(
            match reply.outcome {
                ApplyOutcome::Applied => ApplyStatus::Applied,
                ApplyOutcome::AlreadyApplied => ApplyStatus::AlreadyApplied,
                ApplyOutcome::RolledBack => ApplyStatus::RolledBack,
                // `ApplyOutcome` is non-exhaustive: an outcome added later is the cautious one.
                _ => ApplyStatus::NeedsAttention,
            },
            state(),
        )),
        Err(ElevationError::Declined) => Ok(result(ApplyStatus::Declined, state())),
        Err(ElevationError::Refused(ErrorCode::NotConfirmed)) => {
            Ok(result(ApplyStatus::NotConfirmed, state()))
        }
        Err(ElevationError::Refused(ErrorCode::Blocked)) => {
            Ok(result(ApplyStatus::Blocked, state()))
        }
        Err(ElevationError::Refused(ErrorCode::RestorePointFailed)) => {
            Ok(result(ApplyStatus::RestorePointFailed, state()))
        }
        Err(ElevationError::TimedOut) => Ok(result(ApplyStatus::TimedOut, state())),
        Err(error) => Err(ipc_error(&error)),
    }
}

/// Reverts what the helper applied for the tweak, from the helper's own journal.
pub fn revert(core: &AppCore, id: &str) -> Result<RevertResult, IpcError> {
    let answer = core.elevator()?.tweak_revert(id);
    pull_if_open(core);
    let result = |status, restored, failed| RevertResult {
        tweak: id.to_owned(),
        status,
        restored,
        failed,
        detail: None,
        detail_phrase: None,
        state: state_view(core.engine.state(id)),
    };
    match answer {
        Ok(reply) if reply.complete => Ok(result(RevertStatus::Reverted, reply.batches, 0)),
        Ok(reply) => Ok(result(RevertStatus::Incomplete, reply.batches, 1)),
        Err(ElevationError::Declined) => Ok(result(RevertStatus::Declined, 0, 0)),
        Err(ElevationError::Refused(ErrorCode::NothingToRevert)) => {
            Ok(result(RevertStatus::NothingToRevert, 0, 0))
        }
        Err(ElevationError::Refused(ErrorCode::NotConfirmed)) => {
            Ok(result(RevertStatus::NotConfirmed, 0, 0))
        }
        Err(ElevationError::Refused(ErrorCode::Blocked)) => Ok(result(RevertStatus::Blocked, 0, 0)),
        Err(ElevationError::TimedOut) => Ok(result(RevertStatus::TimedOut, 0, 0)),
        Err(error) => Err(ipc_error(&error)),
    }
}

/// Undoes an administrator change from the Undo page: the helper reverts what it applied for the
/// tweak and restores the earlier value from its own journal.
pub fn undo_change(core: &AppCore, tweak: &str) -> Result<UndoResult, IpcError> {
    let answer = core.elevator()?.tweak_revert(tweak);
    pull_if_open(core);
    let outcome = |status, restored, failed| UndoResult {
        status,
        restored,
        failed,
        failures: Vec::new(),
        overwrites: Vec::new(),
    };
    match answer {
        Ok(reply) if reply.complete => Ok(outcome(UndoStatus::Restored, reply.batches, 0)),
        Ok(reply) => Ok(outcome(UndoStatus::Partial, reply.batches, 1)),
        Err(ElevationError::Declined) => Ok(outcome(UndoStatus::Declined, 0, 0)),
        Err(ElevationError::Refused(ErrorCode::NothingToRevert)) => {
            Ok(outcome(UndoStatus::NothingToUndo, 0, 0))
        }
        Err(ElevationError::Refused(ErrorCode::NotConfirmed)) => {
            Ok(outcome(UndoStatus::NotConfirmed, 0, 0))
        }
        Err(ElevationError::Refused(ErrorCode::Blocked)) => Ok(outcome(UndoStatus::Blocked, 0, 0)),
        Err(ElevationError::TimedOut) => Ok(outcome(UndoStatus::TimedOut, 0, 0)),
        Err(error) => Err(ipc_error(&error)),
    }
}

// ---- the rows on the Undo page ---------------------------------------------------------------

/// The reason, as a phrase code, that administrator changes cannot be undone from here now.
fn unavailable_code(reason: &UnavailableReason) -> &'static str {
    match reason {
        UnavailableReason::NotAllUsersInstall => "undo.adminNotAllUsers",
        UnavailableReason::HelperMissing => "undo.adminHelperMissing",
        UnavailableReason::InstallNotProtected { .. } => "undo.adminNotProtected",
        UnavailableReason::StandardUser => "undo.adminStandardUser",
        // `UnavailableReason` is non-exhaustive: a reason added later degrades to the general one.
        _ => "undo.adminUnknown",
    }
}

/// The helper's rows as Undo page rows. A row that could be undone is read-only, with the reason,
/// when administrator actions cannot be used now.
#[must_use]
pub fn rows(core: &AppCore) -> Vec<ActivityItem> {
    let mirror = load_mirror(core);
    let rows = mirror.rows();
    if rows.is_empty() {
        return Vec::new();
    }
    let unavailable = match core.elevator().map(|elevator| elevator.status()) {
        Ok(ElevationStatus::Available) => None,
        Ok(ElevationStatus::Unavailable { reason }) => Some(unavailable_code(&reason)),
        _ => Some("undo.adminUnknown"),
    };
    rows.iter()
        .map(|row| row_item(core, row, unavailable))
        .collect()
}

fn row_item(
    core: &AppCore,
    row: &MirrorRow<'_>,
    unavailable: Option<&'static str>,
) -> ActivityItem {
    let record = row.record;
    let (state, reason) = match row.state {
        RowState::CanUndo => match unavailable {
            None => (ChangeState::CanUndo, None),
            Some(why) => (ChangeState::CannotUndo, Some(why)),
        },
        RowState::Undone => (ChangeState::Undone, None),
        RowState::Failed => (ChangeState::Failed, None),
        RowState::Restored => (ChangeState::Restored, None),
        RowState::OldStore => (ChangeState::CannotUndo, Some("undo.adminOldStore")),
    };
    let short: String = row.store.chars().take(8).collect();
    ActivityItem {
        id: format!("admin-{short}-{}", record.id.0),
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
            .and_then(|label| core.engine.catalogue().get(label))
            .map(|tweak| tweak.title.clone()),
        label: record.label.clone(),
        // The helper's batch and entry ids belong to the helper's journal: they must never be
        // taken for ids of the app's own.
        batch: None,
        entry: None,
        previous: None,
        previous_phrase: None,
        state,
        reason: reason.map(|code| Phrase::new(code).english()),
        reason_phrase: reason.map(|code| (&Phrase::new(code)).into()),
        administrator: true,
        administrator_tweak: record.label.clone(),
    }
}

/// Tauri command: undoes an administrator change (a row of the Undo page with an Administrator
/// badge) through the helper, which asks for its own confirmation and restores the earlier value
/// from its own journal. The first call of a session shows one Windows prompt.
#[tauri::command]
#[specta::specta]
pub(crate) async fn undo_administrator_change(
    state: tauri::State<'_, AppState>,
    tweak: String,
) -> Result<UndoResult, IpcError> {
    blocking(&state, move |core| undo_change(core, &tweak)).await
}
