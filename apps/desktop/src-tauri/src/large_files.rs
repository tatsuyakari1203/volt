//! The large files finder at the IPC boundary: pick a folder, scan it, open a file's folder and
//! move chosen files to the Recycle Bin.
//!
//! The rules live in `volt_core::large_files`. What is decided here:
//!
//! - **The scan stays in Rust.** The UI gets the list (full paths: the user asked to see where
//!   the files are) and sends back only the scan id and row numbers, never a path, so it cannot
//!   name a file the scan did not find. A new scan replaces the stored one.
//! - **Moved files leave the list.** A trash removes what was moved from the stored scan and
//!   returns the list that is left, renumbered.
//! - **Nothing is deleted for good.** The only change is the move to the Recycle Bin.
//!
//! Byte counts and times cross as `f64` (see [`crate::metrics`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, State};
use tauri_plugin_dialog::DialogExt as _;
use tauri_specta::Event as _;
use volt_core::{
    DEFAULT_MAX_FILES, FileGroup, LargeFile, LargeFilesError, LargeFilesFinder, LargeFilesObserver,
    LargeFilesScan, Source,
};

use crate::IpcError;
use crate::cleaner::{SkipGroup, count32, count64};
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// The type group of a file. Mirrors [`volt_core::FileGroup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FileGroupView {
    /// Videos.
    Video,
    /// Archives.
    Archive,
    /// Disk and virtual machine images.
    DiskImage,
    /// Installers.
    Installer,
    /// Everything else.
    Other,
}

fn group_view(group: FileGroup) -> FileGroupView {
    match group {
        FileGroup::Video => FileGroupView::Video,
        FileGroup::Archive => FileGroupView::Archive,
        FileGroup::DiskImage => FileGroupView::DiskImage,
        FileGroup::Installer => FileGroupView::Installer,
        // `FileGroup` is non-exhaustive: a group added later degrades to `Other`.
        _ => FileGroupView::Other,
    }
}

/// One file of the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LargeFileView {
    /// Row number, sent back to open its folder or move it.
    pub index: u32,
    /// Full path.
    pub path: String,
    /// Size in bytes.
    pub size_bytes: f64,
    /// Last modified, in milliseconds since the Unix epoch, or `null` when unknown.
    pub modified_ms: Option<f64>,
    /// Its type group.
    pub group: FileGroupView,
}

fn file_views(files: &[LargeFile]) -> Vec<LargeFileView> {
    files
        .iter()
        .enumerate()
        .map(|(index, file)| LargeFileView {
            index: count32(index),
            path: file.path.display().to_string(),
            size_bytes: count64(file.size_bytes),
            modified_ms: file.modified.map(|t| {
                #[allow(clippy::cast_precision_loss, reason = "milliseconds fit 2^53")]
                let millis = t.as_unix_millis() as f64;
                millis
            }),
            group: group_view(file.group),
        })
        .collect()
}

/// The result of a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LargeFilesResult {
    /// Names this scan when moving files; empty when the scan was cancelled.
    pub scan_id: String,
    /// Whether the scan was cancelled (nothing is listed then).
    pub cancelled: bool,
    /// The folder that was scanned.
    pub root: String,
    /// The biggest files, biggest first.
    pub files: Vec<LargeFileView>,
    /// Files looked at in all.
    pub files_seen: f64,
    /// Entries that could not be read and were skipped.
    pub unreadable: f64,
    /// Links and junctions left alone.
    pub links: f64,
    /// System folders and files left out on purpose.
    pub excluded: f64,
    /// When the scan started, in milliseconds since the Unix epoch (the age filter counts from it).
    pub scanned_at_ms: f64,
}

/// How far a scan is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct LargeFilesProgress {
    /// Files looked at so far.
    pub files_seen: f64,
    /// Folders read so far.
    pub folders_seen: f64,
}

/// What moving files to the Recycle Bin did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct LargeFilesTrashResult {
    /// Files moved to the Recycle Bin.
    pub moved_files: u32,
    /// Their size in bytes (still used until the Recycle Bin is emptied).
    pub moved_bytes: f64,
    /// Files left in place.
    pub skipped_total: u32,
    /// Files left in place, by reason.
    pub skipped: Vec<SkipGroup>,
    /// The list that is left, renumbered.
    pub files: Vec<LargeFileView>,
    /// Whether the activity record could not be written (the move itself is unaffected).
    pub history_incomplete: bool,
}

#[derive(Debug)]
struct StoredScan {
    id: String,
    scan: LargeFilesScan,
}

/// The scan state of the shell: the last scan, whether one runs and whether it was asked to stop.
#[derive(Debug, Default)]
pub struct LargeFilesSession {
    scan: Mutex<Option<StoredScan>>,
    scanning: AtomicBool,
    cancel: AtomicBool,
    last_id: AtomicU64,
}

struct ScanGuard<'a>(&'a LargeFilesSession);

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.scanning.store(false, Ordering::SeqCst);
    }
}

impl LargeFilesSession {
    fn stored(&self) -> MutexGuard<'_, Option<StoredScan>> {
        // The guard protects a plain value; a poisoned lock only means another call panicked.
        self.scan.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(&self) -> Result<ScanGuard<'_>, IpcError> {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return Err(IpcError::InvalidInput {
                message: "a scan is already running".to_owned(),
            });
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok(ScanGuard(self))
    }

    /// Asks the running scan to stop. Does nothing when none runs.
    pub fn cancel(&self) {
        if self.scanning.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }
}

struct Observer<'a> {
    session: &'a LargeFilesSession,
    on_progress: &'a dyn Fn(LargeFilesProgress),
}

impl LargeFilesObserver for Observer<'_> {
    fn progress(&self, files_seen: u64, folders_seen: u64) {
        (self.on_progress)(LargeFilesProgress {
            files_seen: count64(files_seen),
            folders_seen: count64(folders_seen),
        });
    }

    fn is_cancelled(&self) -> bool {
        self.session.cancel.load(Ordering::SeqCst)
    }
}

fn finder(core: &AppCore) -> Result<&LargeFilesFinder, IpcError> {
    core.large_files().ok_or_else(|| IpcError::Unsupported {
        message: "the large files finder is not available in this build".to_owned(),
    })
}

fn scan_error(error: &LargeFilesError) -> IpcError {
    let message = error.to_string();
    match error {
        LargeFilesError::NotFound => IpcError::NotFound { message },
        LargeFilesError::SystemFoldersUnavailable(_) => IpcError::Unsupported { message },
        LargeFilesError::Unreadable(_) => IpcError::Other { message },
        // Not absolute, not a folder, a system folder (and anything added later).
        _ => IpcError::InvalidInput { message },
    }
}

/// The folder a scan starts from when the user did not choose one: the system drive.
fn default_root() -> PathBuf {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
    PathBuf::from(format!("{}\\", drive.trim_end_matches(['\\', '/'])))
}

/// Scans `root` (the system drive when `None`) read-only, keeps the scan under a new id and
/// returns the biggest files. `on_progress` is called every so often.
///
/// # Errors
/// `invalidInput` when a scan already runs, the folder is not a plain absolute folder or is a
/// system folder; `notFound`; `unsupported` without a finder.
pub fn scan_with(
    core: &AppCore,
    root: Option<&str>,
    on_progress: &dyn Fn(LargeFilesProgress),
) -> Result<LargeFilesResult, IpcError> {
    let finder = finder(core)?;
    let session = core.large_files_session();
    let _running = session.begin()?;
    let root = root.map_or_else(default_root, PathBuf::from);
    let observer = Observer {
        session,
        on_progress,
    };
    let scan = match finder.scan(&root, DEFAULT_MAX_FILES, &observer) {
        Ok(scan) => scan,
        Err(LargeFilesError::Cancelled) => {
            return Ok(LargeFilesResult {
                scan_id: String::new(),
                cancelled: true,
                root: root.display().to_string(),
                files: Vec::new(),
                files_seen: 0.0,
                unreadable: 0.0,
                links: 0.0,
                excluded: 0.0,
                scanned_at_ms: 0.0,
            });
        }
        Err(error) => return Err(scan_error(&error)),
    };
    let id = format!(
        "large-{}",
        session.last_id.fetch_add(1, Ordering::SeqCst) + 1
    );
    #[allow(clippy::cast_precision_loss, reason = "milliseconds fit 2^53")]
    let scanned_at_ms = scan.scanned_at.as_unix_millis() as f64;
    let result = LargeFilesResult {
        scan_id: id.clone(),
        cancelled: false,
        root: scan.root.display().to_string(),
        files: file_views(&scan.files),
        files_seen: count64(scan.files_seen),
        unreadable: count64(scan.unreadable),
        links: count64(scan.links),
        excluded: count64(scan.excluded),
        scanned_at_ms,
    };
    *session.stored() = Some(StoredScan { id, scan });
    Ok(result)
}

/// Moves the files at `indices` of the scan `scan_id` to the Recycle Bin and returns what was
/// moved, what was left (with reasons) and the list that remains.
///
/// # Errors
/// `notFound` when the scan is not the latest one; `invalidInput` when a scan runs, nothing is
/// chosen or a row is not in the scan; `unsupported` without a finder.
pub fn trash_with(
    core: &AppCore,
    scan_id: &str,
    indices: &[u32],
) -> Result<LargeFilesTrashResult, IpcError> {
    let finder = finder(core)?;
    let session = core.large_files_session();
    if session.scanning.load(Ordering::SeqCst) {
        return Err(IpcError::InvalidInput {
            message: "a scan is running; wait for it to finish or cancel it".to_owned(),
        });
    }
    let mut stored = session.stored();
    let Some(current) = stored.as_mut().filter(|s| s.id == scan_id) else {
        return Err(IpcError::NotFound {
            message: "that scan is out of date; scan again".to_owned(),
        });
    };
    if indices.is_empty() {
        return Err(IpcError::InvalidInput {
            message: "no file is chosen".to_owned(),
        });
    }
    let rows: Vec<usize> = indices.iter().map(|&i| i as usize).collect();
    if rows.iter().any(|&row| row >= current.scan.files.len()) {
        return Err(IpcError::InvalidInput {
            message: "a chosen file is not part of this scan".to_owned(),
        });
    }
    let report = finder.trash(&current.scan, &rows, Source::Manual);
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    let moved: Vec<&Path> = report.moved.iter().map(|f| f.path.as_path()).collect();
    current
        .scan
        .files
        .retain(|file| !moved.contains(&file.path.as_path()));
    let mut skipped: Vec<SkipGroup> = Vec::new();
    for (_, reason) in &report.skipped {
        let reason = reason.to_string();
        match skipped.iter_mut().find(|group| group.reason == reason) {
            Some(group) => group.count += 1,
            None => skipped.push(SkipGroup { reason, count: 1 }),
        }
    }
    Ok(LargeFilesTrashResult {
        moved_files: count32(report.moved.len()),
        moved_bytes: count64(report.moved_bytes()),
        skipped_total: count32(report.skipped.len()),
        skipped,
        files: file_views(&current.scan.files),
        history_incomplete: report.activity_write_failed,
    })
}

/// The arguments that make Explorer open a file's folder with the file selected.
fn explorer_select_argument(path: &Path) -> String {
    format!("/select,\"{}\"", path.display())
}

/// Opens the folder of the file at `index` of the scan `scan_id` in Explorer, file selected.
///
/// # Errors
/// `notFound` when the scan is not the latest one; `invalidInput` when the row is not in it;
/// `unsupported` off Windows; `other` when Explorer cannot be started.
pub fn reveal_with(core: &AppCore, scan_id: &str, index: u32) -> Result<(), IpcError> {
    let path = {
        let stored = core.large_files_session().stored();
        let Some(current) = stored.as_ref().filter(|s| s.id == scan_id) else {
            return Err(IpcError::NotFound {
                message: "that scan is out of date; scan again".to_owned(),
            });
        };
        current
            .scan
            .files
            .get(index as usize)
            .map(|file| file.path.clone())
            .ok_or_else(|| IpcError::InvalidInput {
                message: "that file is not part of this scan".to_owned(),
            })?
    };
    open_in_explorer(&path)
}

#[cfg(windows)]
fn open_in_explorer(path: &Path) -> Result<(), IpcError> {
    use std::os::windows::process::CommandExt as _;
    std::process::Command::new("explorer.exe")
        .raw_arg(explorer_select_argument(path))
        .spawn()
        .map(drop)
        .map_err(|error| IpcError::Other {
            message: format!("Explorer could not be started: {error}"),
        })
}

#[cfg(not(windows))]
fn open_in_explorer(path: &Path) -> Result<(), IpcError> {
    let _ = explorer_select_argument(path);
    Err(IpcError::Unsupported {
        message: "opening a folder is only available on Windows".to_owned(),
    })
}

/// Tauri command: scans a folder (the system drive when no folder is given) for its biggest files.
/// Read-only and off the main thread; emits `largeFilesProgress`; `cancelLargeFilesScan` stops it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn scan_large_files(
    app: AppHandle,
    state: State<'_, AppState>,
    root: Option<String>,
) -> Result<LargeFilesResult, IpcError> {
    blocking(&state, move |core| {
        scan_with(core, root.as_deref(), &|progress| {
            // A missed progress event only means the counter does not move.
            let _ = progress.emit(&app);
        })
    })
    .await
}

/// Tauri command: asks the running large files scan to stop. Does nothing when none runs.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands managed state to a command by value"
)]
pub(crate) fn cancel_large_files_scan(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.core()?.large_files_session().cancel();
    Ok(())
}

/// Tauri command: moves the chosen files of the scan `scan_id` to the Recycle Bin. Never a
/// permanent delete.
#[tauri::command]
#[specta::specta]
pub(crate) async fn trash_large_files(
    state: State<'_, AppState>,
    scan_id: String,
    indices: Vec<u32>,
) -> Result<LargeFilesTrashResult, IpcError> {
    blocking(&state, move |core| trash_with(core, &scan_id, &indices)).await
}

/// Tauri command: opens the folder of a listed file in Explorer with the file selected.
#[tauri::command]
#[specta::specta]
pub(crate) async fn reveal_large_file(
    state: State<'_, AppState>,
    scan_id: String,
    index: u32,
) -> Result<(), IpcError> {
    blocking(&state, move |core| reveal_with(core, &scan_id, index)).await
}

/// Tauri command: shows the native folder dialog and returns the chosen folder, or `null` when it
/// was closed. The UI is granted no dialog permission; only Rust opens it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn pick_scan_folder(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, IpcError> {
    blocking(&state, move |_core| {
        let mut dialog = app.dialog().file().set_title("Choose a drive or folder");
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(chosen) = dialog.blocking_pick_folder() else {
            return Ok(None);
        };
        let path = chosen.into_path().map_err(|error| IpcError::Other {
            message: format!("the chosen location cannot be used: {error}"),
        })?;
        Ok(Some(path.display().to_string()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_selects_the_file_with_the_path_quoted() {
        assert_eq!(
            explorer_select_argument(Path::new(r"C:\My Files\a.iso")),
            r#"/select,"C:\My Files\a.iso""#
        );
    }

    #[test]
    fn the_default_root_is_a_drive_root() {
        let root = default_root().display().to_string();
        assert!(root.ends_with('\\'), "{root}");
    }
}
