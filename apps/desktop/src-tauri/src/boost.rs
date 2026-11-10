//! Boost at the IPC boundary: the transfer types, the two commands behind Home's Boost button and
//! the Boost section of Settings, and the event the tray sends when it ran Boost itself.
//!
//! The rules live in `volt_core::boost`; this module translates them into the shell's own types
//! (see [`crate::api`] for why the shell owns its transfer types). The tray and the `boost`
//! command call the same [`boost_of`], so a Boost from the tray with no window open does exactly
//! what the button does.
//!
//! The report never says disk space was freed: cleaning moves files to the Recycle Bin, so the
//! figure is `moved_to_bin`.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;
use volt_core::{Availability, BoostConfig, BoostReport, MemoryArea, Source, run_boost};

use crate::IpcError;
use crate::cleaner::{count32, count64};
use crate::commands::blocking;
use crate::memory::{
    AreaResult, MemoryAreaView, MemoryListsView, OperationState, area_result, reason_for, state_for,
};
use crate::wiring::{AppCore, AppState};

/// What Boost includes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BoostConfigView {
    /// The memory operations that run.
    pub areas: Vec<MemoryAreaView>,
    /// Also move the user temp files to the Recycle Bin, without a preview.
    pub move_temp_to_recycle_bin: bool,
}

impl BoostConfigView {
    fn from_core(config: &BoostConfig) -> Self {
        Self {
            areas: config
                .areas
                .iter()
                .filter_map(|area| MemoryAreaView::from_core(*area))
                .collect(),
            move_temp_to_recycle_bin: config.move_temp_to_recycle_bin,
        }
    }

    fn to_core(&self) -> BoostConfig {
        let mut areas: Vec<MemoryArea> = Vec::new();
        for area in self.areas.iter().map(|area| area.to_core()) {
            if !areas.contains(&area) {
                areas.push(area);
            }
        }
        BoostConfig {
            areas,
            move_temp_to_recycle_bin: self.move_temp_to_recycle_bin,
        }
    }
}

/// A selected operation Boost did not run, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkippedOperationView {
    /// Which operation.
    pub area: MemoryAreaView,
    /// Why it could not run.
    pub state: OperationState,
    /// One sentence saying why.
    pub reason: String,
}

/// What the read-only junk scan found.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JunkView {
    /// Files the Safe cleaning rules could move to the Recycle Bin.
    pub files: u32,
    /// Their size in bytes.
    pub bytes: f64,
    /// Whether the figures are a lower bound (a rule stopped at its item cap).
    pub truncated: bool,
}

/// What the opt-in temp cleaning moved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MovedToBinView {
    /// Files moved to the Recycle Bin.
    pub files: u32,
    /// Their size in bytes. They still use disk space until the Recycle Bin is emptied.
    pub bytes: f64,
    /// Items left in place.
    pub skipped: u32,
}

/// The report of one Boost. There is no "freed" figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BoostResult {
    /// Memory by list before the memory operations, or `null` when none ran or it was unreadable.
    pub memory_before: Option<MemoryListsView>,
    /// Memory by list after them.
    pub memory_after: Option<MemoryListsView>,
    /// Available memory (free plus standby) before, in bytes.
    pub available_before: Option<f64>,
    /// Available memory after, in bytes.
    pub available_after: Option<f64>,
    /// The memory operations that ran, in order, each with how it ended.
    pub areas: Vec<AreaResult>,
    /// Selected operations that were not run (administrator rights, unsupported), with the reason.
    pub skipped: Vec<SkippedOperationView>,
    /// What could be cleaned, or `null` when the cleaner is not available.
    pub junk: Option<JunkView>,
    /// What was moved to the Recycle Bin, or `null` when cleaning did not run.
    pub moved_to_bin: Option<MovedToBinView>,
    /// Whether every change was written to the activity history.
    pub recorded: bool,
}

/// Sent to every window when Boost was started from the tray, with the report. A window that is
/// open shows it on Home; with no window the report is only in the activity history and the tray
/// tooltip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Event)]
pub struct BoostFinished(pub BoostResult);

fn skipped_view(area: MemoryArea, reason: Availability) -> Option<SkippedOperationView> {
    Some(SkippedOperationView {
        area: MemoryAreaView::from_core(area)?,
        state: state_for(reason),
        reason: reason_for(reason).unwrap_or_default(),
    })
}

/// The transfer form of a report.
#[must_use]
pub fn result_of(report: &BoostReport) -> BoostResult {
    let available = report.memory_available();
    BoostResult {
        memory_before: report.memory.before.map(MemoryListsView::from),
        memory_after: report.memory.after.map(MemoryListsView::from),
        available_before: available.map(|(before, _)| count64(before)),
        available_after: available.map(|(_, after)| count64(after)),
        areas: report
            .memory
            .areas
            .iter()
            .filter_map(|entry| area_result(entry.area, &entry.result))
            .collect(),
        skipped: report
            .skipped
            .iter()
            .filter_map(|skip| skipped_view(skip.area, skip.reason))
            .collect(),
        junk: report.junk.map(|junk| JunkView {
            files: count32(junk.files),
            bytes: count64(junk.bytes),
            truncated: junk.truncated,
        }),
        moved_to_bin: report.cleaned.as_ref().map(|cleaned| MovedToBinView {
            files: count32(cleaned.moved_files()),
            bytes: count64(cleaned.moved_bytes()),
            skipped: count32(cleaned.skipped_count()),
        }),
        recorded: !report.memory.activity_write_failed
            && report
                .cleaned
                .as_ref()
                .is_none_or(|cleaned| cleaned.activity_write_failures == 0),
    }
}

/// The saved Boost configuration (the defaults when none is saved).
pub fn boost_config_of(core: &AppCore) -> Result<BoostConfigView, IpcError> {
    Ok(BoostConfigView::from_core(&BoostConfig::load(
        core.settings.as_ref(),
    )))
}

/// Saves the Boost configuration and returns what is stored.
pub fn set_boost_config_of(
    core: &AppCore,
    config: &BoostConfigView,
) -> Result<BoostConfigView, IpcError> {
    let config = config.to_core();
    config.save(core.settings.as_ref())?;
    Ok(BoostConfigView::from_core(&config))
}

/// Runs Boost as saved: the one use case behind the Home button and the tray item. One run at a
/// time, and not while a tweak is being applied. Nothing here can be undone; the memory run and
/// the cleaned rule are recorded in the activity history by the engines.
pub fn boost_of(core: &AppCore) -> Result<BoostResult, IpcError> {
    let memory = core.memory_cleaner()?;
    let config = BoostConfig::load(core.settings.as_ref());
    let _guard = core.exclusive();
    Ok(result_of(&run_boost(
        memory,
        core.clean_engine(),
        &config,
        Source::Manual,
    )))
}

/// A short line for the tray tooltip, in plain words. The text never says space was freed.
#[must_use]
pub fn tooltip_line(outcome: &Result<BoostResult, IpcError>) -> String {
    match outcome {
        Err(_) => "Volt: Boost could not run".to_owned(),
        Ok(result) => match (result.available_before, result.available_after) {
            (Some(before), Some(after)) => format!(
                "Volt: Boost done, memory available {} to {}",
                gib(before),
                gib(after)
            ),
            _ => "Volt: Boost done".to_owned(),
        },
    }
}

fn gib(bytes: f64) -> String {
    format!("{:.1} GB", bytes / f64::from(1u32 << 30))
}

/// Tauri command: runs Boost as saved and returns the before and after report.
#[tauri::command]
#[specta::specta]
pub(crate) async fn boost(state: tauri::State<'_, AppState>) -> Result<BoostResult, IpcError> {
    blocking(&state, boost_of).await
}

/// Tauri command: what Boost includes.
#[tauri::command]
#[specta::specta]
pub(crate) async fn boost_config(
    state: tauri::State<'_, AppState>,
) -> Result<BoostConfigView, IpcError> {
    blocking(&state, boost_config_of).await
}

/// Tauri command: saves what Boost includes and returns what is stored.
#[tauri::command]
#[specta::specta]
pub(crate) async fn set_boost_config(
    state: tauri::State<'_, AppState>,
    config: BoostConfigView,
) -> Result<BoostConfigView, IpcError> {
    blocking(&state, move |core| set_boost_config_of(core, &config)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_view_drops_duplicates_on_the_way_in() {
        let view = BoostConfigView {
            areas: vec![
                MemoryAreaView::StandbyList,
                MemoryAreaView::WorkingSets,
                MemoryAreaView::StandbyList,
            ],
            move_temp_to_recycle_bin: true,
        };
        let core = view.to_core();
        assert_eq!(
            core.areas,
            [MemoryArea::StandbyList, MemoryArea::WorkingSets]
        );
        assert!(core.move_temp_to_recycle_bin);
    }

    #[test]
    fn the_tooltip_names_available_memory_and_never_claims_freed_space() {
        let result = BoostResult {
            memory_before: None,
            memory_after: None,
            available_before: Some(4.0 * f64::from(1u32 << 30)),
            available_after: Some(5.5 * f64::from(1u32 << 30)),
            areas: Vec::new(),
            skipped: Vec::new(),
            junk: None,
            moved_to_bin: None,
            recorded: true,
        };
        let line = tooltip_line(&Ok(result.clone()));
        assert_eq!(line, "Volt: Boost done, memory available 4.0 GB to 5.5 GB");
        assert!(!line.contains("freed"));
        let unread = BoostResult {
            available_before: None,
            ..result
        };
        assert_eq!(tooltip_line(&Ok(unread)), "Volt: Boost done");
        let error = IpcError::Other {
            message: "x".into(),
        };
        assert_eq!(tooltip_line(&Err(error)), "Volt: Boost could not run");
    }
}
