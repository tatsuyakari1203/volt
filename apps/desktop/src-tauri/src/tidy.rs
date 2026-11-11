//! The tidy-up finder at the IPC boundary: pick folders, scan them for duplicate files, empty
//! folders and shortcuts with a missing target, and move chosen items to the Recycle Bin.
//!
//! The rules live in `volt_core::tidy`. What is decided here:
//!
//! - **The scan stays in Rust.** The UI gets the lists (full paths: the user asked to see where
//!   things are) and sends back only the scan id and row numbers, never a path, so it cannot name
//!   an item the scan did not find. A new scan replaces the stored one.
//! - **Moved items leave the lists.** A move removes what was moved from the stored scan (a
//!   duplicate group left with one copy is dropped) and returns the lists that are left,
//!   renumbered.
//! - **Nothing is deleted for good.** The only change is the move to the Recycle Bin.
//!
//! Byte counts and times cross as `f64` (see [`crate::metrics`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, State};
use tauri_specta::Event as _;
use volt_core::{
    DuplicatePick, LargeFilesError, Phrase, Source, TidyFinder, TidyObserver, TidyOptions,
    TidyProgress, TidyScan, TidyStage, TidyTrash,
};

use crate::IpcError;
use crate::cleaner::{SkipGroup, add_skip, count32, count64};
use crate::commands::blocking;
use crate::large_files::{open_in_explorer, scan_error};
use crate::wiring::{AppCore, AppState};

/// Which part of a scan is running. Mirrors [`volt_core::TidyStage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TidyStageView {
    /// Reading folders.
    Walking,
    /// Checking shortcuts.
    Shortcuts,
    /// Comparing files of the same size.
    Comparing,
}

fn stage_view(stage: TidyStage) -> TidyStageView {
    match stage {
        TidyStage::Shortcuts => TidyStageView::Shortcuts,
        TidyStage::Comparing => TidyStageView::Comparing,
        // `TidyStage` is non-exhaustive: a stage added later reads as the first one.
        _ => TidyStageView::Walking,
    }
}

/// How far a scan is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct TidyProgressEvent {
    /// The part that is running.
    pub stage: TidyStageView,
    /// Files looked at so far.
    pub files_seen: f64,
    /// Folders read so far.
    pub folders_seen: f64,
    /// Files compared so far.
    pub compared: f64,
    /// Files that share a size with another, to be compared in all (0 until walking is done).
    pub to_compare: f64,
    /// Bytes read for hashing so far.
    pub bytes_hashed: f64,
}

/// One copy of a duplicated file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateFileView {
    /// Full path.
    pub path: String,
    /// Last modified, in milliseconds since the Unix epoch, or `null` when unknown.
    pub modified_ms: Option<f64>,
}

/// Files with the same content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroupView {
    /// Row number, sent back to move copies.
    pub index: u32,
    /// Size of each copy in bytes.
    pub size_bytes: f64,
    /// Space the extra copies take, in bytes.
    pub wasted_bytes: f64,
    /// Other names of copies in this group (hard links) that were folded away: they share data
    /// with a listed copy, so removing one frees nothing.
    pub hard_links: u32,
    /// The copies, oldest first. A copy's position is its number in a move.
    pub files: Vec<DuplicateFileView>,
}

/// A folder that holds nothing, or only other empty folders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EmptyFolderView {
    /// Row number, sent back to move it.
    pub index: u32,
    /// Full path.
    pub path: String,
    /// How many empty folders are inside it (they go with it).
    pub nested: u32,
}

/// A shortcut whose target does not exist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BrokenShortcutView {
    /// Row number, sent back to move it.
    pub index: u32,
    /// Full path of the shortcut.
    pub path: String,
    /// The path it points to.
    pub target: String,
    /// Size of the shortcut file in bytes.
    pub size_bytes: f64,
}

/// The lists a scan found (and what is left of them after a move).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TidyLists {
    /// Duplicate groups, the ones that waste the most space first.
    pub duplicates: Vec<DuplicateGroupView>,
    /// Empty folders.
    pub empty_folders: Vec<EmptyFolderView>,
    /// Shortcuts with a missing target.
    pub broken_shortcuts: Vec<BrokenShortcutView>,
}

fn millis(time: Option<volt_core::Timestamp>) -> Option<f64> {
    time.map(|t| {
        #[allow(clippy::cast_precision_loss, reason = "milliseconds fit 2^53")]
        let millis = t.as_unix_millis() as f64;
        millis
    })
}

fn lists(scan: &TidyScan) -> TidyLists {
    TidyLists {
        duplicates: scan
            .duplicates
            .iter()
            .enumerate()
            .map(|(index, group)| DuplicateGroupView {
                index: count32(index),
                size_bytes: count64(group.size_bytes),
                wasted_bytes: count64(group.wasted_bytes()),
                hard_links: group.hard_links,
                files: group
                    .files
                    .iter()
                    .map(|file| DuplicateFileView {
                        path: file.path.display().to_string(),
                        modified_ms: millis(file.modified),
                    })
                    .collect(),
            })
            .collect(),
        empty_folders: scan
            .empty_folders
            .iter()
            .enumerate()
            .map(|(index, folder)| EmptyFolderView {
                index: count32(index),
                path: folder.path.display().to_string(),
                nested: folder.nested,
            })
            .collect(),
        broken_shortcuts: scan
            .broken_shortcuts
            .iter()
            .enumerate()
            .map(|(index, shortcut)| BrokenShortcutView {
                index: count32(index),
                path: shortcut.path.display().to_string(),
                target: shortcut.target.clone(),
                size_bytes: count64(shortcut.size_bytes),
            })
            .collect(),
    }
}

/// The result of a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TidyResult {
    /// Names this scan when moving items; empty when the scan was cancelled.
    pub scan_id: String,
    /// Whether the scan was cancelled (nothing is listed then).
    pub cancelled: bool,
    /// The folders that were scanned.
    pub roots: Vec<String>,
    /// What was found.
    pub lists: TidyLists,
    /// Duplicate groups found beyond the ones listed.
    pub groups_omitted: f64,
    /// Files looked at in all.
    pub files_seen: f64,
    /// Entries that could not be read or compared and were skipped.
    pub unreadable: f64,
    /// Links and junctions left alone.
    pub links: f64,
    /// System folders and files left out on purpose.
    pub excluded: f64,
    /// Files left out of the comparison because their data is not on disk (cloud files).
    pub cloud_skipped: f64,
    /// Names of the same file (hard links) folded into one.
    pub hard_links_folded: f64,
    /// Shortcuts looked at.
    pub shortcuts_seen: f64,
    /// Shortcuts that could not be judged (a share, a drive that is not connected, no local
    /// target); they are not listed.
    pub shortcuts_unchecked: f64,
    /// Whether a limit was reached and part of what was asked for was not looked at.
    pub truncated: bool,
    /// Files smaller than this many bytes are not compared for duplicates.
    pub min_duplicate_bytes: f64,
}

/// What moving items to the Recycle Bin did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TidyTrashResult {
    /// Items moved to the Recycle Bin.
    pub moved: u32,
    /// Their size in bytes (still used until the Recycle Bin is emptied).
    pub moved_bytes: f64,
    /// Items left in place.
    pub skipped_total: u32,
    /// Items left in place, by reason.
    pub skipped: Vec<SkipGroup>,
    /// The lists that are left, renumbered.
    pub lists: TidyLists,
    /// Whether the activity record could not be written (the move itself is unaffected).
    pub history_incomplete: bool,
}

/// The copies of one group the user chose to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DuplicatePickInput {
    /// Row number of the group.
    pub group: u32,
    /// Numbers of the copies to remove (at least one copy of the group must stay).
    pub files: Vec<u32>,
}

/// Which list an item is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum TidyItemKind {
    /// A copy in a duplicate group (`file` is its number).
    Duplicate,
    /// An empty folder.
    EmptyFolder,
    /// A shortcut with a missing target.
    Shortcut,
}

#[derive(Debug)]
struct StoredScan {
    id: String,
    scan: TidyScan,
}

/// The scan state of the shell: the last scan, whether one runs and whether it was asked to stop.
#[derive(Debug, Default)]
pub struct TidySession {
    scan: Mutex<Option<StoredScan>>,
    scanning: AtomicBool,
    cancel: AtomicBool,
    last_id: AtomicU64,
}

struct ScanGuard<'a>(&'a TidySession);

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.scanning.store(false, Ordering::SeqCst);
    }
}

impl TidySession {
    fn stored(&self) -> MutexGuard<'_, Option<StoredScan>> {
        // The guard protects a plain value; a poisoned lock only means another call panicked.
        self.scan.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(&self) -> Result<ScanGuard<'_>, IpcError> {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return Err(IpcError::invalid(&Phrase::new("err.scanRunning")));
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
    session: &'a TidySession,
    on_progress: &'a dyn Fn(TidyProgressEvent),
}

impl TidyObserver for Observer<'_> {
    fn progress(&self, progress: &TidyProgress) {
        (self.on_progress)(TidyProgressEvent {
            stage: stage_view(progress.stage),
            files_seen: count64(progress.files_seen),
            folders_seen: count64(progress.folders_seen),
            compared: count64(progress.compared),
            to_compare: count64(progress.to_compare),
            bytes_hashed: count64(progress.bytes_hashed),
        });
    }

    fn is_cancelled(&self) -> bool {
        self.session.cancel.load(Ordering::SeqCst)
    }
}

fn finder(core: &AppCore) -> Result<&TidyFinder, IpcError> {
    core.tidy()
        .ok_or_else(|| IpcError::unsupported(&Phrase::new("err.tidyUnavailable")))
}

/// Scans `roots` read-only, keeps the scan under a new id and returns what it found.
/// `on_progress` is called every so often.
///
/// # Errors
/// `invalidInput` when a scan already runs, no folder is given, or a folder is not a plain
/// absolute folder or is a system folder; `notFound`; `unsupported` without a finder.
pub fn scan_with(
    core: &AppCore,
    roots: &[String],
    on_progress: &dyn Fn(TidyProgressEvent),
) -> Result<TidyResult, IpcError> {
    let finder = finder(core)?;
    let session = core.tidy_session();
    let _running = session.begin()?;
    if roots.is_empty() {
        return Err(IpcError::invalid(&Phrase::new("err.tidy.noFolder")));
    }
    let roots: Vec<PathBuf> = roots.iter().map(PathBuf::from).collect();
    let options = TidyOptions::default();
    let observer = Observer {
        session,
        on_progress,
    };
    let scan = match finder.scan(&roots, &options, &observer) {
        Ok(scan) => scan,
        Err(LargeFilesError::Cancelled) => {
            return Ok(result_of(None, String::new(), &roots, options));
        }
        Err(error) => return Err(scan_error(&error)),
    };
    let id = format!(
        "tidy-{}",
        session.last_id.fetch_add(1, Ordering::SeqCst) + 1
    );
    let result = result_of(Some(&scan), id.clone(), &roots, options);
    *session.stored() = Some(StoredScan { id, scan });
    Ok(result)
}

fn result_of(
    scan: Option<&TidyScan>,
    scan_id: String,
    roots: &[PathBuf],
    options: TidyOptions,
) -> TidyResult {
    let empty = TidyLists {
        duplicates: Vec::new(),
        empty_folders: Vec::new(),
        broken_shortcuts: Vec::new(),
    };
    let count = |value: fn(&TidyScan) -> u64| scan.map_or(0.0, |s| count64(value(s)));
    TidyResult {
        scan_id,
        cancelled: scan.is_none(),
        roots: scan.map_or_else(
            || roots.iter().map(|r| r.display().to_string()).collect(),
            |s| s.roots.iter().map(|r| r.display().to_string()).collect(),
        ),
        lists: scan.map_or(empty, lists),
        groups_omitted: count(|s| s.groups_omitted),
        files_seen: count(|s| s.files_seen),
        unreadable: count(|s| s.unreadable),
        links: count(|s| s.links),
        excluded: count(|s| s.excluded),
        cloud_skipped: count(|s| s.cloud_skipped),
        hard_links_folded: count(|s| s.hard_links_folded),
        shortcuts_seen: count(|s| s.shortcuts_seen),
        shortcuts_unchecked: count(|s| s.shortcuts_unchecked),
        truncated: scan.is_some_and(|s| s.truncated),
        min_duplicate_bytes: count64(options.min_duplicate_bytes),
    }
}

/// Runs a move on the stored scan: checks the scan id and that no scan runs, lets `check` refuse
/// a choice that is not part of the scan, lets `act` do the move and drop what moved from the
/// scan, and returns what was moved and what is left.
fn move_with<R>(
    core: &AppCore,
    scan_id: &str,
    check: impl FnOnce(&TidyScan) -> Result<R, IpcError>,
    act: impl FnOnce(&TidyFinder, &mut TidyScan, R) -> TidyTrash,
) -> Result<TidyTrashResult, IpcError> {
    let finder = finder(core)?;
    let session = core.tidy_session();
    if session.scanning.load(Ordering::SeqCst) {
        return Err(IpcError::invalid(&Phrase::new("err.scanBusy")));
    }
    let mut stored = session.stored();
    let Some(current) = stored.as_mut().filter(|s| s.id == scan_id) else {
        return Err(IpcError::not_found(&Phrase::new("err.scanStale")));
    };
    let checked = check(&current.scan)?;
    let report = act(finder, &mut current.scan, checked);
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    let mut skipped: Vec<SkipGroup> = Vec::new();
    for (_, reason) in &report.skipped {
        add_skip(&mut skipped, reason);
    }
    Ok(TidyTrashResult {
        moved: count32(report.moved.len()),
        moved_bytes: count64(report.moved_bytes()),
        skipped_total: count32(report.skipped.len()),
        skipped,
        lists: lists(&current.scan),
        history_incomplete: report.activity_write_failed,
    })
}

/// The chosen rows as positions, after checking that something is chosen and every row exists.
fn rows(indices: &[u32], len: usize) -> Result<Vec<usize>, IpcError> {
    if indices.is_empty() {
        return Err(IpcError::invalid(&Phrase::new("err.tidy.noItem")));
    }
    let rows: Vec<usize> = indices.iter().map(|&row| row as usize).collect();
    if rows.iter().any(|&row| row >= len) {
        return Err(IpcError::invalid(&Phrase::new("err.itemNotInScan")));
    }
    Ok(rows)
}

/// Moves the chosen copies of duplicate groups of the scan `scan_id` to the Recycle Bin. At least
/// one copy of every group must stay (a group where every copy is chosen is left alone), and a
/// copy that stays is read again first.
///
/// # Errors
/// `notFound` when the scan is not the latest one; `invalidInput` when a scan runs, nothing is
/// chosen or a group or copy is not in the scan; `unsupported` without a finder.
pub fn trash_duplicates_with(
    core: &AppCore,
    scan_id: &str,
    picks: &[DuplicatePickInput],
) -> Result<TidyTrashResult, IpcError> {
    if picks.iter().all(|pick| pick.files.is_empty()) {
        return Err(IpcError::invalid(&Phrase::new("err.tidy.noItem")));
    }
    move_with(
        core,
        scan_id,
        |scan| {
            let in_scan = picks.iter().all(|pick| {
                scan.duplicates
                    .get(pick.group as usize)
                    .is_some_and(|group| {
                        pick.files
                            .iter()
                            .all(|&file| (file as usize) < group.files.len())
                    })
            });
            if in_scan {
                Ok(picks
                    .iter()
                    .map(|pick| DuplicatePick {
                        group: pick.group as usize,
                        files: pick.files.iter().map(|&file| file as usize).collect(),
                    })
                    .collect::<Vec<_>>())
            } else {
                Err(IpcError::invalid(&Phrase::new("err.itemNotInScan")))
            }
        },
        |finder, scan, picks| {
            let report = finder.trash_duplicates(scan, &picks, Source::Manual);
            let moved: Vec<&Path> = report.moved.iter().map(|m| m.path.as_path()).collect();
            for group in &mut scan.duplicates {
                group
                    .files
                    .retain(|file| !moved.contains(&file.path.as_path()));
            }
            // A group with one copy left has no duplicate any more.
            scan.duplicates.retain(|group| group.files.len() >= 2);
            report
        },
    )
}

/// Moves the empty folders at `indices` of the scan `scan_id` to the Recycle Bin.
///
/// # Errors
/// As [`trash_duplicates_with`].
pub fn trash_empty_folders_with(
    core: &AppCore,
    scan_id: &str,
    indices: &[u32],
) -> Result<TidyTrashResult, IpcError> {
    move_with(
        core,
        scan_id,
        |scan| rows(indices, scan.empty_folders.len()),
        |finder, scan, chosen| {
            let report = finder.trash_empty_folders(scan, &chosen, Source::Manual);
            let moved: Vec<&Path> = report.moved.iter().map(|m| m.path.as_path()).collect();
            scan.empty_folders
                .retain(|folder| !moved.contains(&folder.path.as_path()));
            report
        },
    )
}

/// Moves the shortcuts at `indices` of the scan `scan_id` to the Recycle Bin.
///
/// # Errors
/// As [`trash_duplicates_with`].
pub fn trash_broken_shortcuts_with(
    core: &AppCore,
    scan_id: &str,
    indices: &[u32],
) -> Result<TidyTrashResult, IpcError> {
    move_with(
        core,
        scan_id,
        |scan| rows(indices, scan.broken_shortcuts.len()),
        |finder, scan, chosen| {
            let report = finder.trash_shortcuts(scan, &chosen, Source::Manual);
            let moved: Vec<&Path> = report.moved.iter().map(|m| m.path.as_path()).collect();
            scan.broken_shortcuts
                .retain(|shortcut| !moved.contains(&shortcut.path.as_path()));
            report
        },
    )
}

/// Opens the folder of an item of the scan `scan_id` in Explorer, the item selected.
///
/// # Errors
/// `notFound` when the scan is not the latest one; `invalidInput` when the item is not in it;
/// `unsupported` off Windows; `other` when Explorer cannot be started.
pub fn reveal_with(
    core: &AppCore,
    scan_id: &str,
    kind: TidyItemKind,
    index: u32,
    file: Option<u32>,
) -> Result<(), IpcError> {
    let path = {
        let stored = core.tidy_session().stored();
        let Some(current) = stored.as_ref().filter(|s| s.id == scan_id) else {
            return Err(IpcError::not_found(&Phrase::new("err.scanStale")));
        };
        let scan = &current.scan;
        let found = match kind {
            TidyItemKind::Duplicate => scan
                .duplicates
                .get(index as usize)
                .and_then(|group| group.files.get(file.unwrap_or(0) as usize))
                .map(|f| f.path.clone()),
            TidyItemKind::EmptyFolder => scan
                .empty_folders
                .get(index as usize)
                .map(|f| f.path.clone()),
            TidyItemKind::Shortcut => scan
                .broken_shortcuts
                .get(index as usize)
                .map(|s| s.path.clone()),
        };
        found.ok_or_else(|| IpcError::invalid(&Phrase::new("err.itemNotInScan")))?
    };
    open_in_explorer(&path)
}

/// Tauri command: scans the chosen folders for duplicate files, empty folders and shortcuts with
/// a missing target. Read-only and off the main thread; emits `tidyProgressEvent`;
/// `cancelTidyScan` stops it.
#[tauri::command]
#[specta::specta]
pub(crate) async fn scan_tidy(
    app: AppHandle,
    state: State<'_, AppState>,
    roots: Vec<String>,
) -> Result<TidyResult, IpcError> {
    blocking(&state, move |core| {
        scan_with(core, &roots, &|progress| {
            // A missed progress event only means the counter does not move.
            let _ = progress.emit(&app);
        })
    })
    .await
}

/// Tauri command: asks the running tidy-up scan to stop. Does nothing when none runs.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri hands managed state to a command by value"
)]
pub(crate) fn cancel_tidy_scan(state: State<'_, AppState>) -> Result<(), IpcError> {
    state.core()?.tidy_session().cancel();
    Ok(())
}

/// Tauri command: moves the chosen copies of duplicate groups to the Recycle Bin. At least one
/// copy of every group stays, and one of them is checked again first. Never a permanent delete.
#[tauri::command]
#[specta::specta]
pub(crate) async fn trash_duplicates(
    state: State<'_, AppState>,
    scan_id: String,
    picks: Vec<DuplicatePickInput>,
) -> Result<TidyTrashResult, IpcError> {
    blocking(&state, move |core| {
        trash_duplicates_with(core, &scan_id, &picks)
    })
    .await
}

/// Tauri command: moves the chosen empty folders to the Recycle Bin. Never a permanent delete.
#[tauri::command]
#[specta::specta]
pub(crate) async fn trash_empty_folders(
    state: State<'_, AppState>,
    scan_id: String,
    indices: Vec<u32>,
) -> Result<TidyTrashResult, IpcError> {
    blocking(&state, move |core| {
        trash_empty_folders_with(core, &scan_id, &indices)
    })
    .await
}

/// Tauri command: moves the chosen shortcuts with a missing target to the Recycle Bin. Never a
/// permanent delete.
#[tauri::command]
#[specta::specta]
pub(crate) async fn trash_broken_shortcuts(
    state: State<'_, AppState>,
    scan_id: String,
    indices: Vec<u32>,
) -> Result<TidyTrashResult, IpcError> {
    blocking(&state, move |core| {
        trash_broken_shortcuts_with(core, &scan_id, &indices)
    })
    .await
}

/// Tauri command: opens the folder of a listed item in Explorer with the item selected.
#[tauri::command]
#[specta::specta]
pub(crate) async fn reveal_tidy_item(
    state: State<'_, AppState>,
    scan_id: String,
    kind: TidyItemKind,
    index: u32,
    file: Option<u32>,
) -> Result<(), IpcError> {
    blocking(&state, move |core| {
        reveal_with(core, &scan_id, kind, index, file)
    })
    .await
}
