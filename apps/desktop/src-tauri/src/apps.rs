//! Installed apps at the IPC boundary: the list behind the Apps page and removing one app.
//!
//! The rules (what is listed, what is hidden, which command may be started) live in
//! `volt_core::apps`; this module translates them into the shell's own transfer types and runs
//! the work off the main thread. The UI sends back only an app id from the list, never a command:
//! the core looks the id up in a fresh reading and starts what the registry holds. The queue
//! (one app after another, with a Cancel) is the UI's: each `uninstall_app` call waits for its
//! uninstaller and every process it started, so the UI can run them in turn, and
//! `stop_waiting_for_uninstaller` ends that wait by hand. The queue lives in the window: closing
//! the window ends it after the removal that is running (see the page for what it tells the user).
//!
//! Byte counts cross as `f64` (see [`crate::metrics`]) and dates as `YYYY-MM-DD` text.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{
    AppListing, AppProblem, AppSource, Confidence, HiddenReason, InstalledApp, Leftover,
    LeftoverError, LeftoverKind, LeftoverRemoval, LeftoverScan, Phrase, Source, UninstallError,
    UninstallOutcome, UninstallReport,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::phrase::PhraseView;
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
    /// Whether Volt kept what the app's entry said about where it lived, so the page can offer
    /// to look for what the uninstaller left behind (`scanLeftovers`).
    pub leftovers_available: bool,
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
        leftovers_available: report.leftovers_available,
        listing: list_view(&report.listing),
    }
}

impl From<UninstallError> for IpcError {
    fn from(error: UninstallError) -> Self {
        match error {
            UninstallError::NotFound => {
                Self::not_found(&volt_core::Phrase::new("err.uninstall.notFound"))
            }
            UninstallError::NotUninstallable => {
                Self::invalid(&volt_core::Phrase::new("err.uninstall.notUninstallable"))
            }
            UninstallError::Busy => Self::invalid(&volt_core::Phrase::new("err.uninstall.busy")),
            UninstallError::HiddenByDefault => {
                Self::invalid(&volt_core::Phrase::new("err.uninstall.hidden"))
            }
            // Anything added later.
            other => Self::plain(crate::ipc::ErrorClass::InvalidInput, other.to_string()),
        }
    }
}

/// How sure Volt is that a find belongs to the removed app. Mirrors [`volt_core::Confidence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ConfidenceView {
    /// A record made before the uninstall points exactly there.
    High,
    /// The names fit exactly, or an indirect record points there.
    Medium,
    /// Shared, or only the name fits.
    Low,
}

impl From<Confidence> for ConfidenceView {
    fn from(confidence: Confidence) -> Self {
        match confidence {
            Confidence::High => Self::High,
            Confidence::Medium => Self::Medium,
            Confidence::Low => Self::Low,
        }
    }
}

/// What a leftover is. Mirrors [`volt_core::LeftoverKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LeftoverKindView {
    /// A folder, which goes to the Recycle Bin.
    Folder,
    /// A registry key, which is saved in the undo journal first.
    RegistryKey,
}

impl From<LeftoverKind> for LeftoverKindView {
    fn from(kind: LeftoverKind) -> Self {
        match kind {
            LeftoverKind::Folder => Self::Folder,
            LeftoverKind::RegistryKey => Self::RegistryKey,
        }
    }
}

/// One folder or registry key the uninstaller seems to have left behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverView {
    /// Identity within the app; pass it to `removeLeftovers`.
    pub id: String,
    /// Folder or registry key.
    pub kind: LeftoverKindView,
    /// The path or the key.
    pub location: String,
    /// How sure Volt is.
    pub confidence: ConfidenceView,
    /// Whether the page selects it at first (high confidence and removable).
    pub selected_by_default: bool,
    /// Why Volt thinks it belongs to the app, one English sentence per piece of evidence.
    pub evidence: Vec<String>,
    /// The same sentences as phrases, for the window to write in its own language.
    pub evidence_phrases: Vec<PhraseView>,
    /// A folder's size in bytes (0 for a key).
    pub size_bytes: f64,
    /// The files of a folder, or the keys of a registry tree.
    pub items: f64,
    /// Why it cannot be removed from here, when so (machine-wide keys need the helper).
    pub locked: Option<String>,
    /// `locked` as a phrase.
    pub locked_phrase: Option<PhraseView>,
}

/// What the scan for leftovers found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverScanView {
    /// The app the scan is for.
    pub app_name: String,
    /// The finds, the most certain first.
    pub items: Vec<LeftoverView>,
    /// Something could not be read, or a size is only what was counted before Volt stopped.
    pub incomplete: bool,
}

/// One leftover that was removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverRemovedView {
    /// The find's id.
    pub id: String,
    /// Folder or registry key.
    pub kind: LeftoverKindView,
    /// The path or the key.
    pub location: String,
    /// A folder's size in bytes.
    pub size_bytes: f64,
}

/// One leftover that was chosen and not removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverSkippedView {
    /// The find's id.
    pub id: String,
    /// Why it was left, in English.
    pub reason: String,
    /// `reason` as a phrase.
    pub reason_phrase: PhraseView,
}

/// What removing the chosen leftovers did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverRemovalView {
    /// What was removed.
    pub removed: Vec<LeftoverRemovedView>,
    /// What was chosen and left alone.
    pub skipped: Vec<LeftoverSkippedView>,
    /// The bytes of the folders that went to the Recycle Bin.
    pub removed_bytes: f64,
    /// Whether the activity record could not be written.
    pub history_incomplete: bool,
}

fn leftover_view(item: &Leftover) -> LeftoverView {
    LeftoverView {
        id: item.id.clone(),
        kind: item.kind.into(),
        location: item.location.clone(),
        confidence: item.confidence.into(),
        selected_by_default: item.selected_by_default(),
        evidence: item.evidence.iter().map(Phrase::english).collect(),
        evidence_phrases: item.evidence.iter().map(PhraseView::from).collect(),
        size_bytes: bytes(item.size_bytes),
        items: bytes(item.items),
        locked: item.locked.map(|lock| lock.phrase().english()),
        locked_phrase: item.locked.map(|lock| PhraseView::from(&lock.phrase())),
    }
}

fn scan_view(scan: &LeftoverScan) -> LeftoverScanView {
    LeftoverScanView {
        app_name: scan.app_name.clone(),
        items: scan.items.iter().map(leftover_view).collect(),
        incomplete: scan.incomplete,
    }
}

fn removal_view(removal: &LeftoverRemoval) -> LeftoverRemovalView {
    LeftoverRemovalView {
        removed: removal
            .removed
            .iter()
            .map(|removed| LeftoverRemovedView {
                id: removed.id.clone(),
                kind: removed.kind.into(),
                location: removed.location.clone(),
                size_bytes: bytes(removed.size_bytes),
            })
            .collect(),
        skipped: removal
            .skipped
            .iter()
            .map(|(id, reason)| LeftoverSkippedView {
                id: id.clone(),
                reason: reason.phrase().english(),
                reason_phrase: PhraseView::from(&reason.phrase()),
            })
            .collect(),
        removed_bytes: bytes(
            removal
                .removed
                .iter()
                .map(|removed| removed.size_bytes)
                .sum(),
        ),
        history_incomplete: removal.activity_write_failed,
    }
}

impl From<LeftoverError> for IpcError {
    fn from(error: LeftoverError) -> Self {
        match error {
            LeftoverError::NoRecord => {
                Self::not_found(&volt_core::Phrase::new("err.leftovers.noRecord"))
            }
            LeftoverError::Unavailable => {
                Self::unsupported(&volt_core::Phrase::new("err.leftovers.unavailable"))
            }
            // Anything added later.
            other => Self::plain(crate::ipc::ErrorClass::InvalidInput, other.to_string()),
        }
    }
}

/// Looks for what the uninstaller of the app `id` left behind, from what its entry said before
/// it was removed. Reads only; an app Volt did not remove in this session is `notFound`.
pub fn scan_leftovers_of(core: &AppCore, id: &str) -> Result<LeftoverScanView, IpcError> {
    Ok(scan_view(&core.apps()?.scan_leftovers(id)?))
}

/// Removes the leftovers `ids` of the app `id`: folders to the Recycle Bin, per-user registry keys
/// through the undo journal. Each is found again first; one that is not in a fresh scan, or is
/// locked, is left alone and reported.
pub fn remove_leftovers_of(
    core: &AppCore,
    id: &str,
    ids: &[String],
) -> Result<LeftoverRemovalView, IpcError> {
    if ids.is_empty() {
        return Err(IpcError::invalid(&volt_core::Phrase::new(
            "err.leftovers.noItem",
        )));
    }
    let removal = core.apps()?.remove_leftovers(id, ids, Source::Manual)?;
    if removal.activity_write_failed {
        core.note_history_incomplete();
    }
    Ok(removal_view(&removal))
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

/// Ends the wait for the uninstaller that is running, if any. The uninstaller itself keeps
/// running; the running `uninstall_app` call returns with what the list shows.
pub fn stop_waiting_of(core: &AppCore) -> Result<(), IpcError> {
    core.apps()?.stop_waiting();
    Ok(())
}

/// Tauri command: the installed apps with size, publisher and install date. Reads the system
/// only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_apps(state: tauri::State<'_, AppState>) -> Result<AppListView, IpcError> {
    blocking(&state, list_apps_of).await
}

/// Tauri command: removes one app by running the vendor's own registered uninstaller (or asking
/// Windows to remove a packaged app) and waits for it and every process it started. This cannot be undone by Volt, so it is
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

/// Tauri command: stops waiting for the uninstaller that `uninstall_app` is waiting for. Nothing
/// is closed or killed; the uninstaller keeps running and the app is reported from the list.
#[tauri::command]
#[specta::specta]
pub(crate) async fn stop_waiting_for_uninstaller(
    state: tauri::State<'_, AppState>,
) -> Result<(), IpcError> {
    blocking(&state, stop_waiting_of).await
}

/// Tauri command: looks for what the uninstaller of an app removed in this session left behind.
/// Reads only. Each find has a confidence level and the evidence in words.
#[tauri::command]
#[specta::specta]
pub(crate) async fn scan_leftovers(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<LeftoverScanView, IpcError> {
    blocking(&state, move |core| scan_leftovers_of(core, &id)).await
}

/// Tauri command: removes the chosen leftovers of an app: folders go to the Recycle Bin and
/// per-user registry keys are saved in the undo journal first. The UI shows a preview with sizes
/// and asks first.
#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_leftovers(
    state: tauri::State<'_, AppState>,
    id: String,
    ids: Vec<String>,
) -> Result<LeftoverRemovalView, IpcError> {
    blocking(&state, move |core| remove_leftovers_of(core, &id, &ids)).await
}
