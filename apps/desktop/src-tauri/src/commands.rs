//! The Tauri commands for tweaks and activity.
//!
//! Each command is a thin async wrapper over a function in [`crate::api`]. The work reads or
//! writes the registry and `SQLite`, which can block, so it runs on the blocking thread pool and
//! never on the main thread (which also runs the window and the tray).

use tauri::State;

use crate::IpcError;
use crate::api::{self, ActivityPage, ApplyResult, RevertResult, TweakInfo};
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
