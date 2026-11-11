//! Memory at the IPC boundary: the transfer types and the two commands behind the Memory page.
//!
//! The rules live in `volt_core::memory`; this module translates them into the shell's own types
//! (see [`crate::api`] for why the shell owns its transfer types) and runs the work off the main
//! thread. Byte counts cross as `f64`, like the live metrics, so nothing needs a 64-bit integer.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    Availability, MemoryArea, MemoryCleanReport, MemoryLists, PortError, PurgeOutcome, Source,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// One memory cleaning operation. Mirrors [`volt_core::MemoryArea`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum MemoryAreaView {
    /// Trim the working sets of the processes the user can open.
    WorkingSets,
    /// Flush the standby list.
    StandbyList,
    /// Flush the low-priority part of the standby list.
    StandbyListLowPriority,
    /// Flush the modified list.
    ModifiedList,
    /// Trim the system file cache.
    FileCache,
}

impl MemoryAreaView {
    pub(crate) fn to_core(self) -> MemoryArea {
        match self {
            Self::WorkingSets => MemoryArea::WorkingSets,
            Self::StandbyList => MemoryArea::StandbyList,
            Self::StandbyListLowPriority => MemoryArea::StandbyListLowPriority,
            Self::ModifiedList => MemoryArea::ModifiedList,
            Self::FileCache => MemoryArea::FileCache,
        }
    }

    /// `None` for an area this shell does not know (the core enum is non-exhaustive).
    pub(crate) fn from_core(area: MemoryArea) -> Option<Self> {
        match area {
            MemoryArea::WorkingSets => Some(Self::WorkingSets),
            MemoryArea::StandbyList => Some(Self::StandbyList),
            MemoryArea::StandbyListLowPriority => Some(Self::StandbyListLowPriority),
            MemoryArea::ModifiedList => Some(Self::ModifiedList),
            MemoryArea::FileCache => Some(Self::FileCache),
            _ => None,
        }
    }
}

/// Whether an operation can run now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum OperationState {
    /// It can run.
    Available,
    /// It needs administrator rights, which Volt does not have in this session.
    NeedsAdministrator,
    /// This Windows build cannot do it.
    Unsupported,
}

/// An operation on the Memory page and whether it can run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryOperationView {
    /// Which operation.
    pub area: MemoryAreaView,
    /// Whether it can run now.
    pub state: OperationState,
    /// When it cannot run, one sentence saying why.
    pub reason: Option<String>,
}

/// Physical memory by list, in bytes. The four parts add up to the total.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListsView {
    /// Installed physical memory.
    pub total_bytes: f64,
    /// Memory in use by programs, drivers and the kernel.
    pub in_use_bytes: f64,
    /// Cached pages Windows gives back by itself when something needs memory.
    pub standby_bytes: f64,
    /// Pages waiting to be written to disk.
    pub modified_bytes: f64,
    /// Free memory.
    pub free_bytes: f64,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display values; a byte count above 2^53 does not exist on a PC"
)]
fn bytes(value: u64) -> f64 {
    value as f64
}

impl From<MemoryLists> for MemoryListsView {
    fn from(lists: MemoryLists) -> Self {
        Self {
            total_bytes: bytes(lists.total_bytes),
            in_use_bytes: bytes(lists.in_use_bytes),
            standby_bytes: bytes(lists.standby_bytes),
            modified_bytes: bytes(lists.modified_bytes),
            free_bytes: bytes(lists.free_bytes),
        }
    }
}

/// What the Memory page shows when it opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryState {
    /// Memory by list now, or `null` when it could not be read (`problem` says why).
    pub lists: Option<MemoryListsView>,
    /// Why the lists could not be read.
    pub problem: Option<String>,
    /// The operations in the order a run performs them, each with whether it can run.
    pub operations: Vec<MemoryOperationView>,
}

/// How one operation of a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AreaStatus {
    /// It ran.
    Done,
    /// It needs administrator rights, so nothing was changed.
    NeedsAdministrator,
    /// This Windows build does not support it, so nothing was changed.
    Unsupported,
    /// It failed; `detail` says how.
    Failed,
}

/// One operation of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AreaResult {
    /// Which operation.
    pub area: MemoryAreaView,
    /// How it ended.
    pub status: AreaStatus,
    /// Working sets only: how many processes were trimmed.
    pub processes_trimmed: u32,
    /// Working sets only: how many processes could not be opened and were left alone.
    pub processes_skipped: u32,
    /// Working sets only: how many processes were left alone because they are on the exclusion
    /// list.
    pub processes_excluded: u32,
    /// What went wrong, when `status` is `failed` or `unsupported`.
    pub detail: Option<String>,
}

/// The report of one run: memory by list before and after, and what each operation did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryCleanResult {
    /// Memory by list before the run, or `null` when it could not be read.
    pub before: Option<MemoryListsView>,
    /// Memory by list after the run, or `null` when it could not be read.
    pub after: Option<MemoryListsView>,
    /// One entry per selected operation, in the order they ran.
    pub areas: Vec<AreaResult>,
    /// Whether the run was written to the activity log (as a change that cannot be undone).
    pub recorded: bool,
}

/// Why an operation cannot run, in a sentence for the page.
pub(crate) fn reason_for(state: Availability) -> Option<String> {
    match state {
        Availability::Available => None,
        Availability::NeedsAdministrator => Some(
            "Needs administrator rights. Volt is not running with them, and it does not raise \
             itself."
                .to_owned(),
        ),
        _ => Some("This version of Windows does not support it.".to_owned()),
    }
}

pub(crate) fn state_for(state: Availability) -> OperationState {
    match state {
        Availability::Available => OperationState::Available,
        Availability::NeedsAdministrator => OperationState::NeedsAdministrator,
        _ => OperationState::Unsupported,
    }
}

/// Reads memory by list and which operations can run. Reads only.
pub fn memory_state_of(core: &AppCore) -> Result<MemoryState, IpcError> {
    let cleaner = core.memory_cleaner()?;
    let (lists, problem) = match cleaner.lists() {
        Ok(lists) => (Some(MemoryListsView::from(lists)), None),
        Err(error) => (None, Some(IpcError::from(error).message().to_owned())),
    };
    let operations = cleaner
        .availability()
        .into_iter()
        .filter_map(|(area, availability)| {
            Some(MemoryOperationView {
                area: MemoryAreaView::from_core(area)?,
                state: state_for(availability),
                reason: reason_for(availability),
            })
        })
        .collect();
    Ok(MemoryState {
        lists,
        problem,
        operations,
    })
}

pub(crate) fn area_result(
    area: MemoryArea,
    result: &Result<PurgeOutcome, PortError>,
) -> Option<AreaResult> {
    let view = MemoryAreaView::from_core(area)?;
    let (status, outcome, detail) = match result {
        Ok(outcome) => (AreaStatus::Done, *outcome, None),
        Err(PortError::AccessDenied(_)) => (
            AreaStatus::NeedsAdministrator,
            PurgeOutcome::default(),
            None,
        ),
        Err(error @ PortError::Unsupported(_)) => (
            AreaStatus::Unsupported,
            PurgeOutcome::default(),
            Some(error.to_string()),
        ),
        Err(error) => (
            AreaStatus::Failed,
            PurgeOutcome::default(),
            Some(error.to_string()),
        ),
    };
    Some(AreaResult {
        area: view,
        status,
        processes_trimmed: outcome.processes_trimmed,
        processes_skipped: outcome.processes_skipped,
        processes_excluded: outcome.processes_excluded,
        detail,
    })
}

pub(crate) fn clean_result(report: &MemoryCleanReport) -> MemoryCleanResult {
    MemoryCleanResult {
        before: report.before.map(MemoryListsView::from),
        after: report.after.map(MemoryListsView::from),
        areas: report
            .areas
            .iter()
            .filter_map(|entry| area_result(entry.area, &entry.result))
            .collect(),
        recorded: !report.activity_write_failed,
    }
}

/// Runs the selected operations (each once, in the order Windows needs) and reports memory by
/// list before and after. An operation that cannot run says so in its result; the others still
/// run. An empty selection is an error: there is nothing to do.
pub fn clean_memory_of(
    core: &AppCore,
    areas: &[MemoryAreaView],
) -> Result<MemoryCleanResult, IpcError> {
    if areas.is_empty() {
        return Err(IpcError::invalid(&volt_core::Phrase::new(
            "err.noMemoryOperation",
        )));
    }
    let cleaner = core.memory_cleaner()?;
    let selected: Vec<MemoryArea> = areas.iter().map(|area| area.to_core()).collect();
    // One run at a time, and not while a tweak is being applied.
    let _guard = core.exclusive();
    let result = clean_result(&cleaner.run(&selected, Source::Manual));
    if !result.recorded {
        core.note_history_incomplete();
    }
    Ok(result)
}

/// Tauri command: memory by list now and which operations can run. Reads the system only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn memory_state(
    state: tauri::State<'_, AppState>,
) -> Result<MemoryState, IpcError> {
    blocking(&state, memory_state_of).await
}

/// Tauri command: runs the selected memory operations and returns the report. This cannot be
/// undone (it only releases memory Windows can get back), so it is recorded in the activity log
/// but not in the undo journal.
#[tauri::command]
#[specta::specta]
pub(crate) async fn clean_memory(
    state: tauri::State<'_, AppState>,
    areas: Vec<MemoryAreaView>,
) -> Result<MemoryCleanResult, IpcError> {
    blocking(&state, move |core| clean_memory_of(core, &areas)).await
}
