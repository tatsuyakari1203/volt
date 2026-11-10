//! Installed apps at the IPC boundary: the list behind the Apps page and removing one app.
//!
//! The rules (what is listed, what is hidden, which command may be started) live in
//! `volt_core::apps`; this module translates them into the shell's own transfer types and runs
//! the work off the main thread. The UI sends back only an app id from the list, never a command:
//! the core looks the id up in a fresh reading and starts what the registry holds. The queue
//! (one app after another, with a Cancel) is the UI's: each `uninstall_app` call waits for its
//! uninstaller, so the UI can run them in turn.
//!
//! Byte counts cross as `f64` (see [`crate::metrics`]) and dates as `YYYY-MM-DD` text.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    AppListing, AppProblem, AppSource, HiddenReason, InstalledApp, Source, UninstallError,
    UninstallOutcome, UninstallReport,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// Where an app was listed. Mirrors [`volt_core::AppSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum AppSourceView {
    /// Installed for every user (the 64-bit registry view).
    Machine64,
    /// Installed for every user (the 32-bit registry view).
    Machine32,
    /// Installed for the current user only.
    CurrentUser,
    /// A packaged (Store) app of the current user.
    Packaged,
}

impl From<AppSource> for AppSourceView {
    fn from(source: AppSource) -> Self {
        match source {
            AppSource::Machine64 => Self::Machine64,
            AppSource::Machine32 => Self::Machine32,
            AppSource::CurrentUser => Self::CurrentUser,
            // `AppSource` is exhaustive today; keep the packaged case explicit.
            AppSource::Packaged => Self::Packaged,
        }
    }
}

/// Why an app is hidden by default. Mirrors [`volt_core::HiddenReason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum HiddenReasonView {
    /// Marked as a system component.
    SystemComponent,
    /// An update or a patch.
    Update,
    /// A framework other apps depend on.
    Framework,
    /// Cannot be removed.
    NotRemovable,
    /// No usable uninstall command.
    NoUninstallCommand,
}

impl From<HiddenReason> for HiddenReasonView {
    fn from(reason: HiddenReason) -> Self {
        match reason {
            HiddenReason::SystemComponent => Self::SystemComponent,
            HiddenReason::Update => Self::Update,
            HiddenReason::Framework => Self::Framework,
            HiddenReason::NotRemovable => Self::NotRemovable,
            HiddenReason::NoUninstallCommand => Self::NoUninstallCommand,
        }
    }
}

/// One installed app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    /// Stable identity; pass it to `uninstallApp`.
    pub id: String,
    /// The name shown to the user.
    pub name: String,
    /// Who published it, when known.
    pub publisher: Option<String>,
    /// Its version, when known.
    pub version: Option<String>,
    /// Its size on disk in bytes, when known.
    pub size_bytes: Option<f64>,
    /// The day it was installed (`YYYY-MM-DD`), when known.
    pub installed: Option<String>,
    /// Where it was listed (the current-user location is marked in the details).
    pub source: AppSourceView,
    /// Why it is hidden by default; `null` for an ordinary app.
    pub hidden: Option<HiddenReasonView>,
    /// Whether Volt can remove it.
    pub uninstallable: bool,
}

/// A source that could not be read completely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppProblemView {
    /// Which source.
    pub source: AppSourceView,
    /// The whole source could not be read.
    pub unreadable: bool,
    /// Entries of the source that are missing from the list.
    pub skipped: u32,
}

/// Every installed app and what could not be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AppListView {
    /// The apps, sorted by name ignoring case. Hidden ones are included: the UI filters.
    pub apps: Vec<AppView>,
    /// Sources that could not be read completely. The rest of the list is valid.
    pub problems: Vec<AppProblemView>,
}

/// How removing one app ended. Mirrors [`volt_core::UninstallOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum UninstallOutcomeView {
    /// The app is no longer listed.
    Removed,
    /// The uninstaller finished (or was closed) and the app is still listed.
    StillInstalled,
    /// Nothing ran: the uninstaller could not be started or Windows refused the removal.
    Failed,
    /// The uninstaller finished, but the list could not be read again to check.
    Unknown,
}

impl From<UninstallOutcome> for UninstallOutcomeView {
    fn from(outcome: UninstallOutcome) -> Self {
        match outcome {
            UninstallOutcome::Removed => Self::Removed,
            UninstallOutcome::StillInstalled => Self::StillInstalled,
            UninstallOutcome::Unknown => Self::Unknown,
            // `UninstallOutcome` is non-exhaustive: anything new reads as a failure.
            _ => Self::Failed,
        }
    }
}

/// What removing one app did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UninstallResult {
    /// The app that was removed.
    pub id: String,
    /// Its name.
    pub name: String,
    /// Where it was listed.
    pub source: AppSourceView,
    /// How it ended.
    pub outcome: UninstallOutcomeView,
    /// The exit code of the vendor's uninstaller, when it ran and Windows reported one.
    pub exit_code: Option<i32>,
    /// Whether it was written to the activity log (as a change that cannot be undone).
    pub recorded: bool,
    /// The list as it is now.
    pub listing: AppListView,
}

#[allow(
    clippy::cast_precision_loss,
    reason = "display values; a byte count above 2^53 does not exist on a PC"
)]
fn bytes(value: u64) -> f64 {
    value as f64
}

fn app_view(app: &InstalledApp) -> AppView {
    AppView {
        id: app.id.clone(),
        name: app.name.clone(),
        publisher: app.publisher.clone(),
        version: app.version.clone(),
        size_bytes: app.size_bytes.map(bytes),
        installed: app.installed.map(|date| date.to_string()),
        source: app.source.into(),
        hidden: app.hidden.map(Into::into),
        uninstallable: app.uninstallable(),
    }
}

fn problem_view(problem: AppProblem) -> AppProblemView {
    AppProblemView {
        source: problem.source.into(),
        unreadable: problem.unreadable,
        skipped: problem.skipped,
    }
}

fn list_view(listing: &AppListing) -> AppListView {
    AppListView {
        apps: listing.apps.iter().map(app_view).collect(),
        problems: listing.problems.iter().copied().map(problem_view).collect(),
    }
}

fn result_view(report: &UninstallReport) -> UninstallResult {
    UninstallResult {
        id: report.id.clone(),
        name: report.name.clone(),
        source: report.source.into(),
        outcome: report.outcome.into(),
        exit_code: report.exit_code,
        recorded: !report.activity_write_failed,
        listing: list_view(&report.listing),
    }
}

impl From<UninstallError> for IpcError {
    fn from(error: UninstallError) -> Self {
        let message = error.to_string();
        match error {
            UninstallError::NotFound => Self::NotFound { message },
            // Not removable, or another removal is running (and anything added later).
            _ => Self::InvalidInput { message },
        }
    }
}

/// Reads the installed apps (desktop apps from the registry and the current user's packaged
/// apps). Reads only.
pub fn list_apps_of(core: &AppCore) -> Result<AppListView, IpcError> {
    Ok(list_view(&core.apps()?.list()))
}

/// Removes the app `id` and waits for its uninstaller. Refuses an id that is not in a fresh
/// reading and an app that cannot be removed, before anything is started. A system component or
/// an update is refused unless `include_system` is set (the user turned on "Show system
/// components"), so a call that skips the UI cannot reach one by default.
pub fn uninstall_app_of(
    core: &AppCore,
    id: &str,
    include_system: bool,
) -> Result<UninstallResult, IpcError> {
    let apps = core.apps()?;
    let report = if include_system {
        apps.uninstall_including_system(id, Source::Manual)?
    } else {
        apps.uninstall(id, Source::Manual)?
    };
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    Ok(result_view(&report))
}

/// Tauri command: the installed apps with size, publisher and install date. Reads the system
/// only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_apps(state: tauri::State<'_, AppState>) -> Result<AppListView, IpcError> {
    blocking(&state, list_apps_of).await
}

/// Tauri command: removes one app by running the vendor's own registered uninstaller (or asking
/// Windows to remove a packaged app) and waits for it. This cannot be undone by Volt, so it is
/// recorded in the activity log but not in the undo journal. The UI asks first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn uninstall_app(
    state: tauri::State<'_, AppState>,
    id: String,
    include_system: bool,
) -> Result<UninstallResult, IpcError> {
    blocking(&state, move |core| {
        uninstall_app_of(core, &id, include_system)
    })
    .await
}
