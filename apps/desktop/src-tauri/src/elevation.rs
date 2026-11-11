//! Administrator actions at the IPC boundary: whether they can be used and the elevated memory
//! purge.
//!
//! The rules and the helper session live in `volt-core` (the [`Elevator`](volt_core::Elevator)
//! port) and `volt-win` (the real one); this module translates them into the shell's own types
//! (see [`crate::api`] for why the shell owns its transfer types) and keeps the activity record of
//! an elevated run, because the helper never writes the UI's history.
//!
//! Only the Memory page's shield actions call the elevator. Boost and the tray never do: they
//! report "needs administrator rights" for what they cannot do, and never show a prompt.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    ElevationError, ElevationStatus, ErrorCode, MemoryCleanReport, Source, UnavailableReason,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::memory::{MemoryAreaView, MemoryCleanResult, clean_result};
use crate::wiring::{AppCore, AppState};
use volt_core::Phrase;

/// Why administrator actions cannot be used. The UI words each one itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ElevationReasonView {
    /// Volt is not installed for all users (a per-user, portable or development copy).
    NotAllUsers,
    /// The helper is missing from the install folder.
    HelperMissing,
    /// The install folder can be changed by other programs, or could not be checked.
    InstallNotProtected,
    /// A standard user: an administrator has to approve.
    StandardUser,
    /// Anything else.
    Unknown,
}

/// Whether administrator actions can be used now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum ElevationStatusView {
    /// A shield action may start the helper (one administrator prompt per session).
    Available,
    /// They cannot be used.
    #[serde(rename_all = "camelCase")]
    Unavailable {
        /// Why.
        reason: ElevationReasonView,
        /// One sentence in English with the specifics, for the details line.
        detail: String,
    },
}

/// What an elevated purge did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum ElevatedCleanResult {
    /// The helper ran the purges.
    Done {
        /// Memory by list before and after and how each operation ended.
        report: MemoryCleanResult,
    },
    /// The user cancelled the administrator prompt. Nothing was changed and nothing was recorded.
    Declined,
}

fn reason_view(reason: &UnavailableReason) -> ElevationReasonView {
    match reason {
        UnavailableReason::NotAllUsersInstall => ElevationReasonView::NotAllUsers,
        UnavailableReason::HelperMissing => ElevationReasonView::HelperMissing,
        UnavailableReason::InstallNotProtected { .. } => ElevationReasonView::InstallNotProtected,
        UnavailableReason::StandardUser => ElevationReasonView::StandardUser,
        // `UnavailableReason` is non-exhaustive: a reason added later degrades to `Unknown`.
        _ => ElevationReasonView::Unknown,
    }
}

/// Whether administrator actions can be used now. Reads only: never starts the helper and never
/// shows a prompt.
pub fn elevation_status_of(core: &AppCore) -> Result<ElevationStatusView, IpcError> {
    Ok(match core.elevator()?.status() {
        ElevationStatus::Available => ElevationStatusView::Available,
        ElevationStatus::Unavailable { reason } => ElevationStatusView::Unavailable {
            reason: reason_view(&reason),
            detail: reason.to_string(),
        },
        // `ElevationStatus` is non-exhaustive: a status added later is treated as unusable.
        _ => ElevationStatusView::Unavailable {
            reason: ElevationReasonView::Unknown,
            detail: "Administrator actions are not available.".to_owned(),
        },
    })
}

/// The error a failed elevated call becomes.
fn ipc_error(error: &ElevationError) -> IpcError {
    match error {
        ElevationError::Unavailable(_) => {
            IpcError::unsupported(&Phrase::new("err.helper.unavailable"))
        }
        ElevationError::Untrusted(_) => {
            IpcError::access_denied(&Phrase::new("err.helper.untrusted"))
        }
        ElevationError::Mismatch(_) => IpcError::other(&Phrase::new("err.helper.mismatch")),
        ElevationError::Refused(ErrorCode::RateLimited) => {
            IpcError::other(&Phrase::new("err.helper.rateLimited"))
        }
        ElevationError::Refused(_) => IpcError::other(&Phrase::new("err.helper.refused")),
        ElevationError::Disconnected => IpcError::other(&Phrase::new("err.helper.disconnected")),
        // `ElevationError::Declined` is an outcome, not an error, and the enum is non-exhaustive.
        _ => IpcError::other(&Phrase::new("err.helper.unusable")),
    }
}

/// Runs the selected memory operations in the elevated helper (one administrator prompt when no
/// session is open yet) and records the run, like an unelevated one, in the activity history.
/// A cancelled prompt is `Declined`: nothing ran and nothing is recorded. An empty selection is
/// an error.
pub fn clean_memory_elevated_of(
    core: &AppCore,
    areas: &[MemoryAreaView],
) -> Result<ElevatedCleanResult, IpcError> {
    if areas.is_empty() {
        return Err(IpcError::invalid(&Phrase::new("err.noMemoryOperation")));
    }
    // Trimming working sets needs no administrator rights, and the helper does not know the
    // exclusion list: it would trim a protected program. The page never sends it here.
    if areas.contains(&MemoryAreaView::WorkingSets) {
        return Err(IpcError::invalid(&Phrase::new("err.trimNeedsNoAdmin")));
    }
    let selected: Vec<_> = areas.iter().map(|area| area.to_core()).collect();
    // Not under the core's exclusive gate: the prompt can stay open for as long as the user takes,
    // and tweaks must not wait for it. The elevator itself runs one request at a time.
    let reply = match core.elevator()?.memory_purge(&selected) {
        Ok(reply) => reply,
        Err(ElevationError::Declined) => return Ok(ElevatedCleanResult::Declined),
        Err(error) => return Err(ipc_error(&error)),
    };
    let mut report = MemoryCleanReport::from(&reply);
    report.activity_write_failed = !core
        .memory_cleaner()?
        .record_elevated_run(&report, Source::Manual);
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    Ok(ElevatedCleanResult::Done {
        report: clean_result(&report),
    })
}

/// Ends the administrator session (says bye to the helper) when the app exits. Does nothing when
/// there is no session or no elevator.
pub fn end_session_of(core: &AppCore) {
    if let Ok(elevator) = core.elevator() {
        elevator.bye();
    }
}

/// Ends the administrator session of the running app. Called once, when the app exits.
pub(crate) fn end_session<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager as _;
    if let Some(state) = app.try_state::<AppState>()
        && let Ok(core) = state.core()
    {
        end_session_of(&core);
    }
}

/// Tauri command: whether administrator actions can be used now, and if not, why. Reads only; it
/// never starts the helper and never shows a prompt.
#[tauri::command]
#[specta::specta]
pub(crate) async fn elevation_status(
    state: tauri::State<'_, AppState>,
) -> Result<ElevationStatusView, IpcError> {
    blocking(&state, elevation_status_of).await
}

/// Tauri command: runs the selected memory operations as administrator through the helper. The
/// first call of a session shows one administrator prompt; cancelling it resolves with `declined`.
/// This cannot be undone, so it is recorded in the activity log but not in the undo journal.
#[tauri::command]
#[specta::specta]
pub(crate) async fn clean_memory_elevated(
    state: tauri::State<'_, AppState>,
    areas: Vec<MemoryAreaView>,
) -> Result<ElevatedCleanResult, IpcError> {
    blocking(&state, move |core| clean_memory_elevated_of(core, &areas)).await
}
