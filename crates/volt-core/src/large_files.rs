//! The large files finder: the biggest files under a folder the user chose, and
//! moving chosen ones to the Recycle Bin.
//!
//! The scan is read-only. It walks the folder through the [`FileSystem`] port,
//! never follows a junction or symbolic link, and keeps only the biggest
//! `max_files` files while it goes, so memory stays flat on any disk. Left out
//! on purpose: the system folders ([`KnownFolders::system_folders`]), the page,
//! hibernation and swap files, the Recycle Bin and `System Volume Information`,
//! and anything that cannot be read (counted, not listed).
//!
//! Trashing goes through the same port call as the clean engine
//! ([`FileSystem::move_to_trash`]), re-checks each file right before, never
//! deletes for good, and writes one activity record with counts only.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::clean::{CleanSkipReason, KnownFolders, human_bytes, same_path_ci, starts_with_ci};
use crate::error::PortError;
use crate::journal::Source;
use crate::ports::{EntryKind, FileSystem};
use crate::time::{Clock, Timestamp};

/// How many files a scan keeps unless told otherwise.
pub const DEFAULT_MAX_FILES: usize = 200;

/// How often (in folders read) a scan reports progress.
const PROGRESS_EVERY: u64 = 256;

/// Label of the activity records this module writes.
const ACTIVITY_LABEL: &str = "large_files";

/// File names that are never listed, wherever they are.
const SYSTEM_FILES: [&str; 3] = ["pagefile.sys", "hiberfil.sys", "swapfile.sys"];

/// Folder names that are never entered, wherever they are.
const SYSTEM_DIRS: [&str; 2] = ["$recycle.bin", "system volume information"];

/// What kind of file it is, guessed from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FileGroup {
    /// Videos.
    Video,
    /// Archives and compressed files.
    Archive,
    /// Disk and virtual machine images.
    DiskImage,
    /// Installer packages and setup programs.
    Installer,
    /// Everything else.
    Other,
}

impl FileGroup {
    /// The group of a file name.
    #[must_use]
    pub fn of(name: &OsStr) -> Self {
        let path = Path::new(name);
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg"
            | "m2ts" => Self::Video,
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "cab" => {
                Self::Archive
            }
            "iso" | "img" | "vhd" | "vhdx" | "vmdk" | "vdi" | "qcow2" | "wim" | "esd" | "dmg" => {
                Self::DiskImage
            }
            "msi" | "msix" | "msixbundle" | "appx" | "appxbundle" | "msp" => Self::Installer,
            "exe" => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if stem.contains("setup") || stem.contains("install") {
                    Self::Installer
                } else {
                    Self::Other
                }
            }
            _ => Self::Other,
        }
    }
}

/// One file of the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFile {
    /// Full path.
    pub path: PathBuf,
    /// Size in bytes.
    pub size_bytes: u64,
    /// Last modification time, when the file system reports one.
    pub modified: Option<Timestamp>,
    /// Its type group.
    pub group: FileGroup,
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LargeFilesScan {
    /// The folder that was scanned.
    pub root: PathBuf,
    /// When the scan started.
    pub scanned_at: Timestamp,
    /// The biggest files, biggest first (at most the requested number).
    pub files: Vec<LargeFile>,
    /// Files looked at in all.
    pub files_seen: u64,
    /// Folders read in all.
    pub folders_seen: u64,
    /// Entries that could not be read (folders and files), skipped.
    pub unreadable: u64,
    /// Links and junctions left alone.
    pub links: u64,
    /// System folders and files left out on purpose.
    pub excluded: u64,
}

/// Why a scan or a trash did not happen.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum LargeFilesError {
    /// The folder is not an absolute path.
    #[error("the folder is not an absolute path")]
    NotAbsolute,
    /// The folder does not exist.
    #[error("the folder does not exist")]
    NotFound,
    /// The folder is not a plain folder (a file, a link or a junction).
    #[error("the location is not a plain folder")]
    NotDirectory,
    /// The folder is a system folder or inside one.
    #[error("system folders are not scanned")]
    SystemFolder,
    /// The system folders could not be named, so they cannot be left out.
    #[error("the system folders could not be determined ({0})")]
    SystemFoldersUnavailable(&'static str),
    /// The folder could not be read (the text is the error class).
    #[error("the folder could not be read ({0})")]
    Unreadable(&'static str),
    /// The observer asked to stop. Nothing was changed.
    #[error("the scan was cancelled")]
    Cancelled,
}

/// Watches a scan and may stop it.
pub trait LargeFilesObserver {
    /// Called every so often with the files and folders looked at so far.
    fn progress(&self, files_seen: u64, folders_seen: u64);

    /// Asked before each folder; `true` stops the scan.
    fn is_cancelled(&self) -> bool;
}

/// What moving files to the Recycle Bin did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LargeFilesTrash {
    /// Files moved, in order.
    pub moved: Vec<LargeFile>,
    /// Files left where they are, with the reason.
    pub skipped: Vec<(PathBuf, CleanSkipReason)>,
    /// Whether the activity record could not be written (the move itself is
    /// unaffected).
    pub activity_write_failed: bool,
}

impl LargeFilesTrash {
    /// Size of the files moved, in bytes. Not freed space: it stays in the
    /// Recycle Bin until that is emptied.
    #[must_use]
    pub fn moved_bytes(&self) -> u64 {
        self.moved
            .iter()
            .fold(0, |sum, file| sum.saturating_add(file.size_bytes))
    }
}

/// Finds the biggest files and moves chosen ones to the Recycle Bin.
pub struct LargeFilesFinder {
    file_system: Arc<dyn FileSystem>,
    folders: Arc<dyn KnownFolders>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
}

impl std::fmt::Debug for LargeFilesFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LargeFilesFinder").finish_non_exhaustive()
    }
}

/// A candidate in the heap: size, path, modified. `Reverse` makes the smallest
/// the one on top, so it is the one dropped when the heap is full.
type Candidate = Reverse<(u64, PathBuf, Option<Timestamp>)>;

/// The candidates as a list, biggest first.
fn sorted_files(heap: BinaryHeap<Candidate>) -> Vec<LargeFile> {
    let mut files: Vec<LargeFile> = heap
        .into_iter()
        .map(|Reverse((size_bytes, path, modified))| LargeFile {
            group: FileGroup::of(path.file_name().unwrap_or_default()),
            path,
            size_bytes,
            modified,
        })
        .collect();
    files.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.path.cmp(&b.path))
    });
    files
}

impl LargeFilesFinder {
    /// A finder over these ports.
    #[must_use]
    pub fn new(
        file_system: Arc<dyn FileSystem>,
        folders: Arc<dyn KnownFolders>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            file_system,
            folders,
            clock,
            activity: None,
        }
    }

    /// Also writes an activity record for every trash.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// Scans `root` read-only and keeps the `max_files` biggest files.
    ///
    /// # Errors
    /// See [`LargeFilesError`]; nothing is changed in any case.
    pub fn scan(
        &self,
        root: &Path,
        max_files: usize,
        observer: &dyn LargeFilesObserver,
    ) -> Result<LargeFilesScan, LargeFilesError> {
        if !root.is_absolute() {
            return Err(LargeFilesError::NotAbsolute);
        }
        let system = self
            .folders
            .system_folders()
            .map_err(|e| LargeFilesError::SystemFoldersUnavailable(error_class(&e)))?;
        if system.iter().any(|folder| starts_with_ci(root, folder)) {
            return Err(LargeFilesError::SystemFolder);
        }
        match self.file_system.inspect(root) {
            Ok(Some(info)) if info.kind == EntryKind::Directory => {}
            Ok(Some(_)) => return Err(LargeFilesError::NotDirectory),
            Ok(None) => return Err(LargeFilesError::NotFound),
            Err(error) => return Err(LargeFilesError::Unreadable(error_class(&error))),
        }

        let mut scan = LargeFilesScan {
            root: root.to_path_buf(),
            scanned_at: self.clock.now(),
            files: Vec::new(),
            files_seen: 0,
            folders_seen: 0,
            unreadable: 0,
            links: 0,
            excluded: 0,
        };
        let mut heap: BinaryHeap<Candidate> = BinaryHeap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            if observer.is_cancelled() {
                return Err(LargeFilesError::Cancelled);
            }
            let Ok(entries) = self.file_system.read_dir(&dir) else {
                scan.unreadable += 1;
                continue;
            };
            scan.folders_seen += 1;
            if scan.folders_seen % PROGRESS_EVERY == 0 {
                observer.progress(scan.files_seen, scan.folders_seen);
            }
            for entry in entries {
                let Ok(info) = entry.info else {
                    scan.unreadable += 1;
                    continue;
                };
                let name = entry.name.to_string_lossy().to_lowercase();
                match info.kind {
                    EntryKind::File => {
                        if SYSTEM_FILES.contains(&name.as_str()) {
                            scan.excluded += 1;
                            continue;
                        }
                        scan.files_seen += 1;
                        if max_files == 0 {
                            continue;
                        }
                        if heap.len() >= max_files
                            && heap
                                .peek()
                                .is_some_and(|Reverse((size, ..))| *size >= info.size_bytes)
                        {
                            continue;
                        }
                        heap.push(Reverse((
                            info.size_bytes,
                            dir.join(&entry.name),
                            info.modified,
                        )));
                        if heap.len() > max_files {
                            heap.pop();
                        }
                    }
                    EntryKind::Directory => {
                        let path = dir.join(&entry.name);
                        if SYSTEM_DIRS.contains(&name.as_str())
                            || system.iter().any(|folder| same_path_ci(&path, folder))
                        {
                            scan.excluded += 1;
                        } else {
                            pending.push(path);
                        }
                    }
                    // Links are never followed; anything new is left alone too.
                    _ => scan.links += 1,
                }
            }
        }
        observer.progress(scan.files_seen, scan.folders_seen);
        scan.files = sorted_files(heap);
        Ok(scan)
    }

    /// Moves the files of `scan` at `indices` to the Recycle Bin.
    ///
    /// Each file is looked at again right before: it must still be a plain
    /// file with the size and modification time the scan saw, otherwise it is
    /// left where it is. The port refuses anything behind a link. A locked file
    /// is reported, never retried. Indices outside the scan are ignored.
    #[must_use]
    pub fn trash(
        &self,
        scan: &LargeFilesScan,
        indices: &[usize],
        source: Source,
    ) -> LargeFilesTrash {
        let chosen: BTreeSet<usize> = indices.iter().copied().collect();
        let mut result = LargeFilesTrash {
            moved: Vec::new(),
            skipped: Vec::new(),
            activity_write_failed: false,
        };
        for file in chosen.into_iter().filter_map(|index| scan.files.get(index)) {
            match self.trash_one(&scan.root, file) {
                Ok(()) => result.moved.push(file.clone()),
                Err(reason) => result.skipped.push((file.path.clone(), reason)),
            }
        }
        result.activity_write_failed = !self.record(&result, source);
        result
    }

    fn trash_one(&self, root: &Path, file: &LargeFile) -> Result<(), CleanSkipReason> {
        let fs: &dyn FileSystem = self.file_system.as_ref();
        let info = match fs.inspect(&file.path) {
            Ok(Some(info)) => info,
            Ok(None) => return Err(CleanSkipReason::Gone),
            Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
        };
        match info.kind {
            EntryKind::File => {
                if info.size_bytes != file.size_bytes || info.modified != file.modified {
                    return Err(CleanSkipReason::Changed);
                }
            }
            EntryKind::ReparsePoint => return Err(CleanSkipReason::Link),
            _ => return Err(CleanSkipReason::Changed),
        }
        fs.move_to_trash(root, &file.path)
            .map_err(|error| match error {
                PortError::NotFound(_) => CleanSkipReason::Gone,
                PortError::AccessDenied(_) => CleanSkipReason::LockedOrDenied,
                PortError::InvalidInput(_) => CleanSkipReason::Refused,
                other => CleanSkipReason::Failed(error_class(&other)),
            })
    }

    /// Writes the activity record. `true` when written or nothing needed
    /// writing.
    fn record(&self, result: &LargeFilesTrash, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        if result.moved.is_empty() && result.skipped.is_empty() {
            return true;
        }
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let mut summary = format!(
            "Moved {} large file{} to the Recycle Bin ({})",
            result.moved.len(),
            plural(result.moved.len()),
            human_bytes(result.moved_bytes()),
        );
        if !result.skipped.is_empty() {
            summary = format!(
                "{summary}, left {} file{} in place",
                result.skipped.len(),
                plural(result.skipped.len())
            );
        }
        summary.push_str("; space is freed only when the Recycle Bin is emptied");
        let activity = NewActivity {
            timestamp: self.clock.now(),
            source,
            outcome: if result.moved.is_empty() {
                ActivityOutcome::Failed
            } else {
                ActivityOutcome::Applied
            },
            summary,
            batch: None,
            entry: None,
            label: Some(ACTIVITY_LABEL.to_owned()),
        };
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for large files ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests;
