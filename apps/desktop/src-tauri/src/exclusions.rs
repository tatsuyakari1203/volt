//! The exclusion list at the IPC boundary: the three commands behind the "Protected programs"
//! card on the Memory page.
//!
//! The rule (which names match, how they are normalised, that a trim skips them) lives in
//! `volt_core::exclusions` and `volt_core::memory`; this module only reads and writes the stored
//! list. Every trim path reads it from the settings at the start of a run, so a change takes
//! effect on the next Boost, Memory page clean or tray Boost.

use volt_core::ExclusionList;

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

fn names_of(list: &ExclusionList) -> Vec<String> {
    list.names().to_vec()
}

/// The protected executable names, sorted.
pub fn memory_exclusions_of(core: &AppCore) -> Result<Vec<String>, IpcError> {
    Ok(names_of(&ExclusionList::load(core.settings.as_ref())?))
}

/// Protects `name` (an executable name such as `game.exe`, or a path to one) and returns the
/// list. Adding a name that is already there changes nothing.
pub fn add_memory_exclusion_of(core: &AppCore, name: &str) -> Result<Vec<String>, IpcError> {
    // Read, change and write as one step, so two quick clicks cannot lose a name.
    let _guard = core.exclusive();
    let mut list = ExclusionList::load(core.settings.as_ref())?;
    if list.add(name)? {
        list.save(core.settings.as_ref())?;
    }
    Ok(names_of(&list))
}

/// Stops protecting `name` and returns the list. Removing a name that is not there changes
/// nothing.
pub fn remove_memory_exclusion_of(core: &AppCore, name: &str) -> Result<Vec<String>, IpcError> {
    let _guard = core.exclusive();
    let mut list = ExclusionList::load(core.settings.as_ref())?;
    if list.remove(name) {
        list.save(core.settings.as_ref())?;
    }
    Ok(names_of(&list))
}

/// Tauri command: the programs protected from memory trimming.
#[tauri::command]
#[specta::specta]
pub(crate) async fn memory_exclusions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<String>, IpcError> {
    blocking(&state, memory_exclusions_of).await
}

/// Tauri command: protects a program (by executable name) from memory trimming. Resolves with
/// the new list.
#[tauri::command]
#[specta::specta]
pub(crate) async fn add_memory_exclusion(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<Vec<String>, IpcError> {
    blocking(&state, move |core| add_memory_exclusion_of(core, &name)).await
}

/// Tauri command: stops protecting a program. Resolves with the new list.
#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_memory_exclusion(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<Vec<String>, IpcError> {
    blocking(&state, move |core| remove_memory_exclusion_of(core, &name)).await
}
