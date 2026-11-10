//! The Tauri commands for tweaks, activity and undo.
//!
//! Each command is a thin async wrapper over a function in [`crate::api`]. The work reads or
//! writes the registry and `SQLite`, which can block, so it runs on the blocking thread pool and
//! never on the main thread (which also runs the window and the tray).

use tauri::{AppHandle, Manager as _, State};
use tauri_plugin_dialog::DialogExt as _;

use crate::IpcError;
use crate::api::{
    self, ActivityExport, ActivityPage, ApplyResult, ExportResult, RevertResult, TweakInfo,
    UndoPreview, UndoResult,
};
use crate::wiring::{AppCore, AppState};

/// Runs `work` on the blocking thread pool and returns its result.
async fn blocking<T, F>(state: &AppState, work: F) -> Result<T, IpcError>
where
    T: Send + 'static,
    F: FnOnce(&AppCore) -> Result<T, IpcError> + Send + 'static,
{
    let core = state.core()?;
    tauri::async_runtime::spawn_blocking(move || work(&core))
        .await
        .map_err(|error| IpcError::Other {
            message: format!("the background task failed: {error}"),
        })?
}

/// Tauri command: lists the catalogue tweaks with their live state. Reads the system only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_tweaks(state: State<'_, AppState>) -> Result<Vec<TweakInfo>, IpcError> {
    blocking(&state, api::list_tweaks).await
}

/// Tauri command: applies a tweak through the undo journal and reports exactly how it ended
/// (applied, already applied, rolled back or needs attention).
#[tauri::command]
#[specta::specta]
pub(crate) async fn apply_tweak(
    state: State<'_, AppState>,
    id: String,
) -> Result<ApplyResult, IpcError> {
    blocking(&state, move |core| api::apply_tweak(core, &id)).await
}

/// Tauri command: reverts what Volt applied for a tweak and reports whether the revert was
/// complete.
#[tauri::command]
#[specta::specta]
pub(crate) async fn revert_tweak(
    state: State<'_, AppState>,
    id: String,
) -> Result<RevertResult, IpcError> {
    blocking(&state, move |core| api::revert_tweak(core, &id)).await
}

/// Tauri command: reads the activity history, newest first. `limit` is clamped to 1 to 200;
/// `cursor` is the `nextCursor` of the previous page.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_activity(
    state: State<'_, AppState>,
    limit: u32,
    cursor: Option<String>,
) -> Result<ActivityPage, IpcError> {
    blocking(&state, move |core| {
        api::list_activity(core, limit, cursor.as_deref())
    })
    .await
}

/// Tauri command: undoes one journal entry (the `entry` of an activity line) and reports exactly
/// how it ended (restored, partly restored, failed or nothing to undo).
#[tauri::command]
#[specta::specta]
pub(crate) async fn undo_entry(
    state: State<'_, AppState>,
    entry: String,
) -> Result<UndoResult, IpcError> {
    blocking(&state, move |core| api::undo_entry(core, &entry)).await
}

/// Tauri command: undoes every applied entry of a journal batch (the `batch` of an activity
/// line), newest first, and reports exactly how it ended.
#[tauri::command]
#[specta::specta]
pub(crate) async fn undo_batch(
    state: State<'_, AppState>,
    batch: String,
) -> Result<UndoResult, IpcError> {
    blocking(&state, move |core| api::undo_batch(core, &batch)).await
}

/// Tauri command: lists what undoing a batch would restore right now. Reads only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_undo_batch(
    state: State<'_, AppState>,
    batch: String,
) -> Result<UndoPreview, IpcError> {
    blocking(&state, move |core| api::preview_undo_batch(core, &batch)).await
}

/// Tauri command: renders the whole activity log as text, for the copy-to-clipboard fallback.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_activity_text(
    state: State<'_, AppState>,
) -> Result<ActivityExport, IpcError> {
    blocking(&state, api::export_activity_text).await
}

/// Tauri command: asks the user where to save the activity log (a native save dialog) and writes
/// it as a text file. The file system is reached from Rust only: the UI is granted no file
/// system or dialog permission. Closing the dialog is `cancelled`, not an error.
#[tauri::command]
#[specta::specta]
pub(crate) async fn export_activity(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<ExportResult, IpcError> {
    blocking(&state, move |core| save_export(&app, core)).await
}

/// Shows the save dialog and writes the export. Blocks until the dialog closes, so it runs on the
/// blocking pool (the dialog must not be opened from the main thread).
fn save_export(app: &AppHandle, core: &AppCore) -> Result<ExportResult, IpcError> {
    let export = api::export_activity_text(core)?;
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save the activity log")
        .set_file_name(&export.file_name)
        .add_filter("Text file", &["txt"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let Some(chosen) = dialog.blocking_save_file() else {
        return Ok(ExportResult::Cancelled);
    };
    let path = chosen.into_path().map_err(|error| IpcError::Other {
        message: format!("the chosen location cannot be used: {error}"),
    })?;
    api::write_export(&path, &export.text)?;
    Ok(ExportResult::Saved {
        path: path.display().to_string(),
        records: export.records,
    })
}
