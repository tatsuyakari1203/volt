//! Processes at the IPC boundary: the transfer types and the two commands behind the Processes
//! page.
//!
//! The rules (grouping, CPU load, what may be ended) live in `volt_core::process`; this module
//! translates them into the shell's own types (see [`crate::api`] for why the shell owns its
//! transfer types) and runs the work off the main thread. Byte counts cross as `f64`, like the
//! live metrics, so nothing needs a 64-bit integer.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    EndError, EndReport, EndStatus, Endability, ProcessGroup, ProcessSample, Protection, Source,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// Whether and how an app may be ended. Mirrors [`volt_core::Endability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum EndabilityView {
    /// It may be ended after a confirmation.
    Allowed,
    /// It may be ended, but it is the Windows shell: the confirmation gives an extra warning.
    Explorer,
    /// Windows needs it, so it is never offered for ending.
    ProtectedWindows,
    /// It is Volt itself or the `WebView` Volt draws in, so it is never offered for ending.
    ProtectedVolt,
}

impl From<Endability> for EndabilityView {
    fn from(endability: Endability) -> Self {
        match endability {
            Endability::Protected(Protection::Windows) => Self::ProtectedWindows,
            Endability::Protected(Protection::Volt) => Self::ProtectedVolt,
            Endability::Explorer => Self::Explorer,
            Endability::Allowed => Self::Allowed,
        }
    }
}

/// One app: every process that runs one executable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProcessGroupView {
    /// Stable identity of the row (the executable name in lower case); pass it to `endApp`.
    pub id: String,
    /// The executable name.
    pub name: String,
    /// How many processes run it.
    pub process_count: u32,
    /// How many of them ending the app would end; 0 when it is protected.
    pub end_count: u32,
    /// Private working set of all its processes, in bytes.
    pub memory_bytes: f64,
    /// Share of all CPU capacity used since the previous reading (0 to 100), or `null` for the
    /// first reading.
    pub cpu_percent: Option<f64>,
    /// Whether and how it may be ended.
    pub endability: EndabilityView,
}

/// The process list, grouped by app, biggest memory user first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProcessListView {
    /// One entry per app.
    pub groups: Vec<ProcessGroupView>,
    /// How many processes there are in all.
    pub process_count: u32,
    /// Logical processors the CPU share is measured against.
    pub logical_cpus: u32,
}

/// How ending one process went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum EndStatusView {
    /// It was ended.
    Ended,
    /// Windows refused: ending it needs administrator rights.
    NeedsAdministrator,
    /// It had already exited.
    AlreadyGone,
    /// It could not be ended for another reason.
    Failed,
}

impl From<EndStatus> for EndStatusView {
    fn from(status: EndStatus) -> Self {
        match status {
            EndStatus::Ended => Self::Ended,
            EndStatus::NeedsAdministrator => Self::NeedsAdministrator,
            EndStatus::AlreadyGone => Self::AlreadyGone,
            EndStatus::Failed(_) => Self::Failed,
        }
    }
}

/// One process of an ended app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ProcessEndView {
    /// Process id.
    pub pid: u32,
    /// How ending it went.
    pub status: EndStatusView,
}

/// What ending an app did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EndAppResult {
    /// The executable name.
    pub name: String,
    /// Processes that were ended.
    pub ended: u32,
    /// Processes that need administrator rights to end (nothing was changed for them).
    pub needs_administrator: u32,
    /// Processes that had already exited.
    pub already_gone: u32,
    /// Processes that could not be ended for another reason.
    pub failed: u32,
    /// One entry per process that was tried.
    pub processes: Vec<ProcessEndView>,
    /// Whether it was written to the activity log (as a change that cannot be undone).
    pub recorded: bool,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display values; a byte count above 2^53 does not exist on a PC"
)]
fn bytes(value: u64) -> f64 {
    value as f64
}

impl From<&ProcessGroup> for ProcessGroupView {
    fn from(group: &ProcessGroup) -> Self {
        Self {
            id: group.key.clone(),
            name: group.name.clone(),
            process_count: group.process_count,
            end_count: group.end_count,
            memory_bytes: bytes(group.memory_bytes),
            cpu_percent: group.cpu_percent,
            endability: group.endability.into(),
        }
    }
}

fn list_view(sample: &ProcessSample, logical_cpus: u32) -> ProcessListView {
    ProcessListView {
        groups: sample.groups.iter().map(ProcessGroupView::from).collect(),
        process_count: sample.process_count,
        logical_cpus,
    }
}

fn end_result(report: &EndReport) -> EndAppResult {
    EndAppResult {
        name: report.name.clone(),
        ended: report.ended(),
        needs_administrator: report.needs_administrator(),
        already_gone: report.already_gone(),
        failed: report.failed(),
        processes: report
            .outcomes
            .iter()
            .map(|outcome| ProcessEndView {
                pid: outcome.pid,
                status: outcome.status.into(),
            })
            .collect(),
        recorded: !report.activity_write_failed,
    }
}

impl From<EndError> for IpcError {
    fn from(error: EndError) -> Self {
        match error {
            EndError::Protected { .. } => Self::InvalidInput {
                message: error.to_string(),
            },
            EndError::NotRunning(_) => Self::NotFound {
                message: error.to_string(),
            },
            EndError::List(port) => port.into(),
        }
    }
}

/// Reads the process list, grouped by app. Reads only. The first call has no CPU share (there is
/// no earlier reading to compare with); the next ones do.
pub fn list_processes_of(core: &AppCore) -> Result<ProcessListView, IpcError> {
    let monitor = core.process_monitor()?;
    Ok(list_view(&monitor.sample()?, monitor.logical_cpus()))
}

/// Ends every process of the app `id`. Windows-critical apps and Volt are refused here, whatever
/// the UI offered.
pub fn end_app_of(core: &AppCore, id: &str) -> Result<EndAppResult, IpcError> {
    let monitor = core.process_monitor()?;
    Ok(end_result(&monitor.end_group(id, Source::Manual)?))
}

/// Tauri command: the process list grouped by app with memory and CPU. Reads the system only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_processes(
    state: tauri::State<'_, AppState>,
) -> Result<ProcessListView, IpcError> {
    blocking(&state, list_processes_of).await
}

/// Tauri command: ends every process of an app. This cannot be undone (unsaved work is lost), so
/// it is recorded in the activity log but not in the undo journal. The UI asks first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn end_app(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<EndAppResult, IpcError> {
    blocking(&state, move |core| end_app_of(core, &id)).await
}
