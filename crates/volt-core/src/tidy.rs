//! The tidy-up finder: duplicate files, empty folders and shortcuts whose target is missing, in
//! folders the user chose, and moving chosen ones to the Recycle Bin.
//!
//! One read-only walk of the chosen folders feeds three lists.
//!
//! # Duplicates
//!
//! Files of 1 MiB or more are grouped by size; only files that share a size are looked at
//! again. Of those, a file counts only when:
//!
//! - its data is on disk. A cloud placeholder (or any file Windows would have to download) is
//!   left alone and never read ([`FileIdentity::on_disk`]);
//! - it is not another name of a file already in the group. Two paths with the same volume and
//!   file index are hard links to one file: they share their data, so removing one frees
//!   nothing and they are not duplicates. They are recognised through [`FileSystem::identity`].
//!
//! The survivors are compared by a hash of their first 64 KiB, and only the files that still
//! agree are hashed in full (SHA-256, in 1 MiB steps, so memory stays flat). A group is the set
//! of files whose full hashes are equal. The scan can be cancelled between steps.
//!
//! Left out on purpose: links and junctions (never followed), the system folders
//! ([`KnownFolders::system_folders`]: Windows, Program Files, `ProgramData`), the Recycle Bin,
//! `System Volume Information`, files and folders with the system attribute, and anything that
//! cannot be read (counted, not listed). Because system and program folders are never walked, no
//! copy listed is in one.
//!
//! The walk holds at most [`MAX_CANDIDATES`] files and [`MAX_FOLDERS`] folders; past that the
//! rest is ignored and the scan says it is incomplete.
//!
//! # Empty folders
//!
//! A folder is empty when it holds nothing at all, or only other empty folders. Only the topmost
//! empty folder of such a tree is listed, with how many empty folders are inside it. Never
//! listed: a folder with any file in it (hidden, read-only or zero bytes too), a folder that
//! holds a link, a system folder, a folder with the system attribute, a folder that could not be
//! read, and the folders the user chose to scan.
//!
//! # Shortcuts
//!
//! A `.lnk` file is broken when it names a local target on a fixed disk and that target does not
//! exist while the disk is connected. Its target is read from the file's own bytes (nothing is
//! resolved or searched for). Not called broken, and counted as unchecked: a shortcut whose
//! target is on a drive that is not connected, a removable, optical or network drive, a share, or
//! that names no local path at all (an app, a Control Panel item).
//!
//! # Removing
//!
//! Every removal goes through [`FileSystem::move_to_trash`], never deletes for good, and looks
//! at the item again right before. For duplicates the user picks the copies to remove and at least
//! one copy of a group must stay; before any copy of a group is removed, a kept copy is read
//! again and must still be there with the same size, time and content hash. One activity record
//! with counts only is written per removal.

mod shortcut;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest as _, Sha256};

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::clean::{CleanSkipReason, KnownFolders, same_path_ci, starts_with_ci};
use crate::error::PortError;
use crate::journal::Source;
use crate::large_files::{LargeFilesError, SYSTEM_DIRS, SYSTEM_FILES};
use crate::phrase::Phrase;
use crate::ports::{EntryKind, FileSystem};
use crate::time::{Clock, Timestamp};

/// Files smaller than this are not compared unless the options say otherwise.
pub const DEFAULT_MIN_DUPLICATE_BYTES: u64 = 1 << 20;

/// How many duplicate groups a scan keeps, the ones that waste the most space first.
pub const MAX_GROUPS: usize = 500;

/// The most files a scan holds for comparing; later ones are ignored.
pub const MAX_CANDIDATES: usize = 300_000;

/// The most folders a scan holds; folders past it are not entered.
pub const MAX_FOLDERS: usize = 500_000;

/// The most shortcuts a scan checks.
const MAX_SHORTCUTS: usize = 20_000;

/// How many bytes of a file the first comparison hashes.
const PARTIAL_BYTES: u64 = 64 * 1024;

/// The size of one read while hashing.
const CHUNK: usize = 1 << 20;

/// How often (in folders read) the walk reports progress.
const PROGRESS_EVERY: u64 = 256;

/// How many bytes are hashed between two progress reports.
const PROGRESS_BYTES: u64 = 64 << 20;

/// Shortcuts bigger than this are not read.
const MAX_SHORTCUT_BYTES: u64 = 64 * 1024;

/// A SHA-256 hash.
pub type ContentHash = [u8; 32];

/// What a scan keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TidyOptions {
    /// Files smaller than this many bytes are not compared (empty files never are).
    pub min_duplicate_bytes: u64,
}

impl Default for TidyOptions {
    fn default() -> Self {
        Self {
            min_duplicate_bytes: DEFAULT_MIN_DUPLICATE_BYTES,
        }
    }
}

/// One copy of a duplicated file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateFile {
    /// Full path.
    pub path: PathBuf,
    /// The scanned folder it was found in.
    pub root: PathBuf,
    /// Last modification time, when the file system reports one.
    pub modified: Option<Timestamp>,
}

/// Files with the same content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    /// Size of each copy, in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the content.
    pub hash: ContentHash,
    /// The copies (at least two), oldest first.
    pub files: Vec<DuplicateFile>,
    /// Other names of copies in this group (hard links) that were folded away, because they
    /// share their data with a listed copy.
    pub hard_links: u32,
}

impl DuplicateGroup {
    /// Space the extra copies take: the size times all copies but one.
    #[must_use]
    pub fn wasted_bytes(&self) -> u64 {
        self.size_bytes
            .saturating_mul(self.files.len().saturating_sub(1) as u64)
    }
}

/// A folder that holds nothing, or only other empty folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyFolder {
    /// Full path.
    pub path: PathBuf,
    /// The scanned folder it was found in.
    pub root: PathBuf,
    /// How many empty folders are inside it (they go with it).
    pub nested: u32,
}

/// A shortcut whose target does not exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrokenShortcut {
    /// Full path of the `.lnk` file.
    pub path: PathBuf,
    /// The scanned folder it was found in.
    pub root: PathBuf,
    /// The path it points to.
    pub target: String,
    /// Size of the shortcut file in bytes.
    pub size_bytes: u64,
    /// Last modification time of the shortcut file.
    pub modified: Option<Timestamp>,
}

/// What a scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TidyScan {
    /// The folders that were scanned (a folder inside another chosen one is not repeated).
    pub roots: Vec<PathBuf>,
    /// When the scan started.
    pub scanned_at: Timestamp,
    /// Duplicate groups, the ones that waste the most space first.
    pub duplicates: Vec<DuplicateGroup>,
    /// Groups found beyond [`MAX_GROUPS`], not listed.
    pub groups_omitted: u64,
    /// Empty folders, by path.
    pub empty_folders: Vec<EmptyFolder>,
    /// Shortcuts with a missing target, by path.
    pub broken_shortcuts: Vec<BrokenShortcut>,
    /// Files looked at in all.
    pub files_seen: u64,
    /// Folders read in all.
    pub folders_seen: u64,
    /// Entries that could not be read or compared, skipped.
    pub unreadable: u64,
    /// Links and junctions left alone.
    pub links: u64,
    /// System folders and files left out on purpose.
    pub excluded: u64,
    /// Files of the same size that were left out of the comparison because their data is not
    /// on disk (cloud placeholders). They were not read.
    pub cloud_skipped: u64,
    /// Names of the same file (hard links) folded into one, in all.
    pub hard_links_folded: u64,
    /// Shortcuts looked at.
    pub shortcuts_seen: u64,
    /// Shortcuts that could not be judged (a share, a drive that is not connected, no local
    /// target), not listed.
    pub shortcuts_unchecked: u64,
    /// Whether a limit was reached and part of the folders or files was not looked at.
    pub truncated: bool,
}

/// Which part of a scan is running.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TidyStage {
    /// Reading folders.
    #[default]
    Walking,
    /// Checking shortcuts.
    Shortcuts,
    /// Comparing files of the same size.
    Comparing,
}

/// How far a scan is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TidyProgress {
    /// The part that is running.
    pub stage: TidyStage,
    /// Files looked at so far.
    pub files_seen: u64,
    /// Folders read so far.
    pub folders_seen: u64,
    /// Files compared so far (of those that share a size with another).
    pub compared: u64,
    /// Files that share a size with another, to be compared in all (known once walking is
    /// done).
    pub to_compare: u64,
    /// Bytes read for hashing so far.
    pub bytes_hashed: u64,
    reported_bytes: u64,
}

/// Watches a scan and may stop it.
pub trait TidyObserver {
    /// Called every so often.
    fn progress(&self, progress: &TidyProgress);

    /// Asked before each folder and each read; `true` stops the scan.
    fn is_cancelled(&self) -> bool;
}

struct Unobserved;

impl TidyObserver for Unobserved {
    fn progress(&self, _: &TidyProgress) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// An item that was moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TidyMoved {
    /// Where it was.
    pub path: PathBuf,
    /// Its size in bytes (0 for a folder).
    pub size_bytes: u64,
}

/// What moving items to the Recycle Bin did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TidyTrash {
    /// Items moved, in order.
    pub moved: Vec<TidyMoved>,
    /// Items left where they are, with the reason.
    pub skipped: Vec<(PathBuf, CleanSkipReason)>,
    /// Whether the activity record could not be written (the move itself is unaffected).
    pub activity_write_failed: bool,
}

impl TidyTrash {
    fn new() -> Self {
        Self {
            moved: Vec::new(),
            skipped: Vec::new(),
            activity_write_failed: false,
        }
    }

    /// Size of the items moved, in bytes. Not freed space: it stays in the Recycle Bin until
    /// that is emptied.
    #[must_use]
    pub fn moved_bytes(&self) -> u64 {
        self.moved
            .iter()
            .fold(0, |sum, item| sum.saturating_add(item.size_bytes))
    }
}

/// The copies of one group the user chose to remove: positions in [`DuplicateGroup::files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicatePick {
    /// Position of the group in [`TidyScan::duplicates`].
    pub group: usize,
    /// Positions of the copies to remove.
    pub files: Vec<usize>,
}

/// What the activity record of a removal is about.
#[derive(Debug, Clone, Copy)]
enum Kind {
    Duplicates,
    Folders,
    Shortcuts,
}

/// A directory the walk read or will read.
#[derive(Debug)]
struct Node {
    parent: Option<usize>,
    root: usize,
    name: OsString,
    /// Whether the folder is, or may be, not empty: unread, unreadable, or holding anything but
    /// empty folders.
    blocked: bool,
}

/// A file the walk found, with the scanned folder it is in.
struct FileFound {
    path: PathBuf,
    size: u64,
    modified: Option<Timestamp>,
    root: usize,
}

/// The files the walk keeps: those to compare, and the shortcuts to check.
#[derive(Default)]
struct Files {
    candidates: Vec<FileFound>,
    shortcuts: Vec<FileFound>,
}

/// What a walk leaves.
struct Walked {
    files: Files,
    nodes: Vec<Node>,
}

/// Keeps a file for comparing or checking, within the limits.
fn note_file(
    files: &mut Files,
    scan: &mut TidyScan,
    file: FileFound,
    is_shortcut: bool,
    min_bytes: u64,
) {
    let (list, limit) = if is_shortcut {
        (&mut files.shortcuts, MAX_SHORTCUTS)
    } else if file.size >= min_bytes {
        (&mut files.candidates, MAX_CANDIDATES)
    } else {
        return;
    };
    if list.len() < limit {
        list.push(file);
    } else {
        scan.truncated = true;
    }
}

/// Why hashing a file stopped.
enum HashStop {
    Cancelled,
    /// The file could not be read, or is not what the listing said.
    Unusable,
}

enum Verdict {
    Fine,
    Broken(String),
    Unchecked,
}

/// Finds duplicates, empty folders and broken shortcuts, and moves chosen ones to the Recycle Bin.
pub struct TidyFinder {
    file_system: Arc<dyn FileSystem>,
    folders: Arc<dyn KnownFolders>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
}

impl std::fmt::Debug for TidyFinder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TidyFinder").finish_non_exhaustive()
    }
}

/// The path of a node: its root, then the names below it.
fn node_path(nodes: &[Node], roots: &[PathBuf], mut at: usize) -> PathBuf {
    let mut names: Vec<&OsString> = Vec::new();
    while let Some(parent) = nodes[at].parent {
        names.push(&nodes[at].name);
        at = parent;
    }
    let mut path = roots[nodes[at].root].clone();
    path.extend(names.into_iter().rev());
    path
}

/// The folders to scan without the ones inside another chosen folder (or repeated).
fn distinct_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    roots
        .iter()
        .enumerate()
        .filter(|(at, root)| {
            !roots.iter().enumerate().any(|(other, base)| {
                other != *at
                    && starts_with_ci(root, base)
                    && (!same_path_ci(root, base) || other < *at)
            })
        })
        .map(|(_, root)| root.clone())
        .collect()
}

impl TidyFinder {
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

    /// Also writes an activity record for every removal.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// Scans `roots` read-only.
    ///
    /// # Errors
    /// See [`LargeFilesError`] (the same checks apply to every folder); nothing is changed in
    /// any case.
    pub fn scan(
        &self,
        roots: &[PathBuf],
        options: &TidyOptions,
        observer: &dyn TidyObserver,
    ) -> Result<TidyScan, LargeFilesError> {
        let system = self
            .folders
            .system_folders()
            .map_err(|e| LargeFilesError::SystemFoldersUnavailable(error_class(&e)))?;
        for root in roots {
            if !root.is_absolute() {
                return Err(LargeFilesError::NotAbsolute);
            }
            if system.iter().any(|folder| starts_with_ci(root, folder)) {
                return Err(LargeFilesError::SystemFolder);
            }
            match self.file_system.inspect(root) {
                Ok(Some(info)) if info.kind == EntryKind::Directory => {}
                Ok(Some(_)) => return Err(LargeFilesError::NotDirectory),
                Ok(None) => return Err(LargeFilesError::NotFound),
                Err(error) => return Err(LargeFilesError::Unreadable(error_class(&error))),
            }
        }
        let roots = distinct_roots(roots);
        if roots.is_empty() {
            return Err(LargeFilesError::NotFound);
        }

        let mut scan = TidyScan {
            roots,
            scanned_at: self.clock.now(),
            duplicates: Vec::new(),
            groups_omitted: 0,
            empty_folders: Vec::new(),
            broken_shortcuts: Vec::new(),
            files_seen: 0,
            folders_seen: 0,
            unreadable: 0,
            links: 0,
            excluded: 0,
            cloud_skipped: 0,
            hard_links_folded: 0,
            shortcuts_seen: 0,
            shortcuts_unchecked: 0,
            truncated: false,
        };
        let mut progress = TidyProgress::default();
        let Walked { files, mut nodes } = self.walk(
            &mut scan,
            options.min_duplicate_bytes,
            &system,
            observer,
            &mut progress,
        )?;
        scan.empty_folders = empty_folders(&mut nodes, &scan.roots);
        self.check_shortcuts(&files.shortcuts, &mut scan, observer, &mut progress)?;
        self.find_duplicates(files.candidates, &mut scan, observer, &mut progress)?;
        Ok(scan)
    }

    fn walk(
        &self,
        scan: &mut TidyScan,
        min_duplicate_bytes: u64,
        system: &[PathBuf],
        observer: &dyn TidyObserver,
        progress: &mut TidyProgress,
    ) -> Result<Walked, LargeFilesError> {
        let min_bytes = min_duplicate_bytes.max(1);
        let mut files = Files::default();
        let mut nodes: Vec<Node> = Vec::new();
        let mut pending: Vec<(PathBuf, usize)> = Vec::new();
        for (root, path) in scan.roots.clone().into_iter().enumerate() {
            nodes.push(Node {
                parent: None,
                root,
                name: path.as_os_str().to_owned(),
                blocked: true,
            });
            pending.push((path, nodes.len() - 1));
        }
        while let Some((dir, at)) = pending.pop() {
            if observer.is_cancelled() {
                return Err(LargeFilesError::Cancelled);
            }
            let Ok(entries) = self.file_system.read_dir(&dir) else {
                scan.unreadable += 1;
                continue;
            };
            scan.folders_seen += 1;
            if scan.folders_seen % PROGRESS_EVERY == 0 {
                progress.files_seen = scan.files_seen;
                progress.folders_seen = scan.folders_seen;
                observer.progress(progress);
            }
            let root = nodes[at].root;
            let mut blocked = false;
            for entry in entries {
                let Ok(info) = entry.info else {
                    scan.unreadable += 1;
                    blocked = true;
                    continue;
                };
                let name = entry.name.to_string_lossy().to_lowercase();
                match info.kind {
                    EntryKind::File => {
                        blocked = true;
                        if info.system || SYSTEM_FILES.contains(&name.as_str()) {
                            scan.excluded += 1;
                            continue;
                        }
                        scan.files_seen += 1;
                        let is_shortcut = Path::new(&entry.name)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"));
                        let file = FileFound {
                            path: dir.join(&entry.name),
                            size: info.size_bytes,
                            modified: info.modified,
                            root,
                        };
                        note_file(&mut files, scan, file, is_shortcut, min_bytes);
                    }
                    EntryKind::Directory => {
                        let path = dir.join(&entry.name);
                        if SYSTEM_DIRS.contains(&name.as_str())
                            || info.system
                            || system.iter().any(|folder| same_path_ci(&path, folder))
                        {
                            scan.excluded += 1;
                            blocked = true;
                        } else if nodes.len() >= MAX_FOLDERS {
                            scan.truncated = true;
                            blocked = true;
                        } else {
                            nodes.push(Node {
                                parent: Some(at),
                                root,
                                name: entry.name.clone(),
                                blocked: true,
                            });
                            pending.push((path, nodes.len() - 1));
                        }
                    }
                    // Links are never followed; anything new is left alone too.
                    _ => {
                        scan.links += 1;
                        blocked = true;
                    }
                }
            }
            nodes[at].blocked = blocked;
        }
        progress.files_seen = scan.files_seen;
        progress.folders_seen = scan.folders_seen;
        observer.progress(progress);
        Ok(Walked { files, nodes })
    }

    fn check_shortcuts(
        &self,
        shortcuts: &[FileFound],
        scan: &mut TidyScan,
        observer: &dyn TidyObserver,
        progress: &mut TidyProgress,
    ) -> Result<(), LargeFilesError> {
        progress.stage = TidyStage::Shortcuts;
        observer.progress(progress);
        for file in shortcuts {
            if observer.is_cancelled() {
                return Err(LargeFilesError::Cancelled);
            }
            scan.shortcuts_seen += 1;
            match self.verdict(&file.path, file.size) {
                Verdict::Fine => {}
                Verdict::Unchecked => scan.shortcuts_unchecked += 1,
                Verdict::Broken(target) => scan.broken_shortcuts.push(BrokenShortcut {
                    path: file.path.clone(),
                    root: scan.roots[file.root].clone(),
                    target,
                    size_bytes: file.size,
                    modified: file.modified,
                }),
            }
        }
        scan.broken_shortcuts.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(())
    }

    /// Reads a shortcut and decides whether its target is missing.
    fn verdict(&self, path: &Path, size: u64) -> Verdict {
        if size == 0 || size > MAX_SHORTCUT_BYTES {
            return Verdict::Unchecked;
        }
        let Ok(length) = usize::try_from(size) else {
            return Verdict::Unchecked;
        };
        let mut bytes = vec![0_u8; length];
        let mut filled = 0;
        while filled < length {
            match self
                .file_system
                .read_at(path, filled as u64, &mut bytes[filled..])
            {
                Ok(0) | Err(_) => break,
                Ok(read) => filled += read,
            }
        }
        bytes.truncate(filled);
        let shortcut::Target::Local {
            path: target,
            drive_type,
        } = shortcut::parse(&bytes)
        else {
            return Verdict::Unchecked;
        };
        // A removable, optical or network drive comes and goes: its shortcuts are never judged.
        if drive_type != shortcut::DRIVE_FIXED {
            return Verdict::Unchecked;
        }
        let target_path = PathBuf::from(&target);
        if !target_path.is_absolute() {
            return Verdict::Unchecked;
        }
        match self.file_system.inspect(&target_path) {
            Ok(Some(_)) => Verdict::Fine,
            Ok(None) => {
                // Missing: broken only when the disk it was on is connected.
                let root = target_path
                    .ancestors()
                    .filter(|a| !a.as_os_str().is_empty())
                    .last()
                    .map(Path::to_path_buf);
                match root.map(|root| self.file_system.inspect(&root)) {
                    Some(Ok(Some(_))) => Verdict::Broken(target),
                    _ => Verdict::Unchecked,
                }
            }
            Err(_) => Verdict::Unchecked,
        }
    }

    fn find_duplicates(
        &self,
        mut candidates: Vec<FileFound>,
        scan: &mut TidyScan,
        observer: &dyn TidyObserver,
        progress: &mut TidyProgress,
    ) -> Result<(), LargeFilesError> {
        candidates.sort_by_key(|candidate| candidate.size);
        let runs: Vec<(usize, usize)> = {
            let mut runs = Vec::new();
            let mut start = 0;
            while start < candidates.len() {
                let mut end = start + 1;
                while end < candidates.len() && candidates[end].size == candidates[start].size {
                    end += 1;
                }
                if end - start >= 2 {
                    runs.push((start, end));
                }
                start = end;
            }
            runs
        };
        progress.stage = TidyStage::Comparing;
        progress.to_compare = runs.iter().map(|(start, end)| (end - start) as u64).sum();
        observer.progress(progress);
        let mut buffer = vec![0_u8; CHUNK];
        let mut groups: Vec<DuplicateGroup> = Vec::new();
        for (start, end) in runs {
            if observer.is_cancelled() {
                return Err(LargeFilesError::Cancelled);
            }
            self.compare_run(
                &candidates[start..end],
                &mut buffer,
                scan,
                &mut groups,
                observer,
                progress,
            )?;
            progress.compared += (end - start) as u64;
            observer.progress(progress);
        }
        groups.sort_by(|a, b| {
            b.wasted_bytes()
                .cmp(&a.wasted_bytes())
                .then_with(|| a.files[0].path.cmp(&b.files[0].path))
        });
        if groups.len() > MAX_GROUPS {
            scan.groups_omitted = (groups.len() - MAX_GROUPS) as u64;
            groups.truncate(MAX_GROUPS);
        }
        scan.duplicates = groups;
        Ok(())
    }

    /// Compares files of one size and adds the groups of equal ones.
    fn compare_run(
        &self,
        run: &[FileFound],
        buffer: &mut [u8],
        scan: &mut TidyScan,
        groups: &mut Vec<DuplicateGroup>,
        observer: &dyn TidyObserver,
        progress: &mut TidyProgress,
    ) -> Result<(), LargeFilesError> {
        let size = run[0].size;
        // Who is who: files with data on disk, one name per physical file.
        let mut by_identity: HashMap<(u64, u64), u32> = HashMap::new();
        let mut files: Vec<(&FileFound, (u64, u64))> = Vec::new();
        for candidate in run {
            match self.file_system.identity(&candidate.path) {
                Ok(identity) if !identity.on_disk => scan.cloud_skipped += 1,
                Ok(identity) => {
                    let key = (identity.volume, identity.file_id);
                    if let Some(extra) = by_identity.get_mut(&key) {
                        *extra += 1;
                        scan.hard_links_folded += 1;
                    } else {
                        by_identity.insert(key, 0);
                        files.push((candidate, key));
                    }
                }
                Err(_) => scan.unreadable += 1,
            }
        }
        if files.len() < 2 {
            return Ok(());
        }

        let mut by_partial: HashMap<ContentHash, Vec<usize>> = HashMap::new();
        for (at, (candidate, _)) in files.iter().enumerate() {
            match self.hash(
                &candidate.path,
                size.min(PARTIAL_BYTES),
                buffer,
                observer,
                progress,
            ) {
                Ok(hash) => by_partial.entry(hash).or_default().push(at),
                Err(HashStop::Cancelled) => return Err(LargeFilesError::Cancelled),
                Err(HashStop::Unusable) => scan.unreadable += 1,
            }
        }
        for (partial, members) in by_partial {
            if members.len() < 2 {
                continue;
            }
            // A file no bigger than the partial read has been read in full already.
            let mut by_full: HashMap<ContentHash, Vec<usize>> = HashMap::new();
            if size <= PARTIAL_BYTES {
                by_full.insert(partial, members);
            } else {
                for at in members {
                    match self.hash(&files[at].0.path, size, buffer, observer, progress) {
                        Ok(hash) => by_full.entry(hash).or_default().push(at),
                        Err(HashStop::Cancelled) => return Err(LargeFilesError::Cancelled),
                        Err(HashStop::Unusable) => scan.unreadable += 1,
                    }
                }
            }
            for (hash, members) in by_full {
                if members.len() < 2 {
                    continue;
                }
                let mut copies: Vec<DuplicateFile> = members
                    .iter()
                    .map(|&at| DuplicateFile {
                        path: files[at].0.path.clone(),
                        root: scan.roots[files[at].0.root].clone(),
                        modified: files[at].0.modified,
                    })
                    .collect();
                copies.sort_by(|a, b| {
                    // Oldest first; a file with no known time last.
                    a.modified
                        .is_none()
                        .cmp(&b.modified.is_none())
                        .then(a.modified.cmp(&b.modified))
                        .then_with(|| a.path.cmp(&b.path))
                });
                groups.push(DuplicateGroup {
                    size_bytes: size,
                    hash,
                    files: copies,
                    hard_links: members
                        .iter()
                        .map(|&at| by_identity.get(&files[at].1).copied().unwrap_or(0))
                        .sum(),
                });
            }
        }
        Ok(())
    }

    /// SHA-256 of the first `len` bytes of a file, read in steps. The file must hold at least
    /// that much. Never called for a file whose data is not on disk.
    fn hash(
        &self,
        path: &Path,
        len: u64,
        buffer: &mut [u8],
        observer: &dyn TidyObserver,
        progress: &mut TidyProgress,
    ) -> Result<ContentHash, HashStop> {
        let mut hasher = Sha256::new();
        let mut offset = 0_u64;
        while offset < len {
            if observer.is_cancelled() {
                return Err(HashStop::Cancelled);
            }
            let want =
                usize::try_from(len - offset).map_or(buffer.len(), |left| left.min(buffer.len()));
            let read = self
                .file_system
                .read_at(path, offset, &mut buffer[..want])
                .map_err(|_| HashStop::Unusable)?;
            if read == 0 {
                // Shorter than the listing said: it changed.
                return Err(HashStop::Unusable);
            }
            hasher.update(&buffer[..read]);
            offset += read as u64;
            progress.bytes_hashed += read as u64;
            if progress.bytes_hashed - progress.reported_bytes >= PROGRESS_BYTES {
                progress.reported_bytes = progress.bytes_hashed;
                observer.progress(progress);
            }
        }
        Ok(hasher.finalize().into())
    }

    /// Moves the chosen copies of `scan` to the Recycle Bin.
    ///
    /// For every group: at least one copy must be left out of the choice (otherwise nothing of
    /// that group moves), and one of those copies is read again and must still have the size,
    /// time and content hash of the scan; if none does, nothing of the group moves. Each chosen
    /// copy is looked at again right before and must still be a plain file with the size and
    /// time the scan saw. The port refuses anything behind a link. A locked file is reported,
    /// never retried. Positions outside the scan are ignored.
    #[must_use]
    pub fn trash_duplicates(
        &self,
        scan: &TidyScan,
        picks: &[DuplicatePick],
        source: Source,
    ) -> TidyTrash {
        let mut chosen: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for pick in picks {
            chosen
                .entry(pick.group)
                .or_default()
                .extend(pick.files.iter().copied());
        }
        let mut result = TidyTrash::new();
        let mut buffer = vec![0_u8; CHUNK];
        for (at, remove) in chosen {
            let Some(group) = scan.duplicates.get(at) else {
                continue;
            };
            let remove: Vec<&DuplicateFile> = remove
                .into_iter()
                .filter_map(|file| group.files.get(file))
                .collect();
            let keep: Vec<&DuplicateFile> = group
                .files
                .iter()
                .filter(|file| !remove.iter().any(|gone| gone.path == file.path))
                .collect();
            if remove.is_empty() {
                continue;
            }
            let refusal = if keep.is_empty() {
                Some(CleanSkipReason::NoCopyKept)
            } else if keep
                .iter()
                .any(|file| self.still_matches(file, group, &mut buffer))
            {
                None
            } else {
                Some(CleanSkipReason::KeptCopyLost)
            };
            for file in remove {
                let outcome = match &refusal {
                    Some(reason) => Err(*reason),
                    None => {
                        self.trash_file(&file.root, &file.path, group.size_bytes, file.modified)
                    }
                };
                match outcome {
                    Ok(()) => result.moved.push(TidyMoved {
                        path: file.path.clone(),
                        size_bytes: group.size_bytes,
                    }),
                    Err(reason) => result.skipped.push((file.path.clone(), reason)),
                }
            }
        }
        result.activity_write_failed = !self.record(&result, Kind::Duplicates, source);
        result
    }

    /// Whether a copy to keep is still there, unchanged, with the content of its group.
    fn still_matches(
        &self,
        file: &DuplicateFile,
        group: &DuplicateGroup,
        buffer: &mut [u8],
    ) -> bool {
        let still_listed = matches!(
            self.file_system.inspect(&file.path),
            Ok(Some(info)) if info.kind == EntryKind::File
                && info.size_bytes == group.size_bytes
                && info.modified == file.modified
        );
        still_listed
            && self
                .hash(
                    &file.path,
                    group.size_bytes,
                    buffer,
                    &Unobserved,
                    &mut TidyProgress::default(),
                )
                .is_ok_and(|hash| hash == group.hash)
    }

    /// Moves the empty folders of `scan` at `indices` to the Recycle Bin. Each is looked at
    /// again right before: it must still be a plain folder holding nothing but empty folders.
    #[must_use]
    pub fn trash_empty_folders(
        &self,
        scan: &TidyScan,
        indices: &[usize],
        source: Source,
    ) -> TidyTrash {
        let chosen: BTreeSet<usize> = indices.iter().copied().collect();
        let mut result = TidyTrash::new();
        for folder in chosen
            .into_iter()
            .filter_map(|at| scan.empty_folders.get(at))
        {
            let outcome = self
                .still_empty(&folder.path)
                .and_then(|()| self.move_to_trash(&folder.root, &folder.path));
            match outcome {
                Ok(()) => result.moved.push(TidyMoved {
                    path: folder.path.clone(),
                    size_bytes: 0,
                }),
                Err(reason) => result.skipped.push((folder.path.clone(), reason)),
            }
        }
        result.activity_write_failed = !self.record(&result, Kind::Folders, source);
        result
    }

    /// Whether `path` is a plain folder with nothing in it but empty folders.
    fn still_empty(&self, path: &Path) -> Result<(), CleanSkipReason> {
        match self.file_system.inspect(path) {
            Ok(Some(info)) => match info.kind {
                EntryKind::Directory => {}
                EntryKind::ReparsePoint => return Err(CleanSkipReason::Link),
                _ => return Err(CleanSkipReason::Changed),
            },
            Ok(None) => return Err(CleanSkipReason::Gone),
            Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
        }
        let mut pending = vec![path.to_path_buf()];
        while let Some(dir) = pending.pop() {
            let entries = self
                .file_system
                .read_dir(&dir)
                .map_err(|error| match error {
                    PortError::NotFound(_) => CleanSkipReason::Gone,
                    PortError::InvalidInput(_) => CleanSkipReason::Link,
                    other => CleanSkipReason::Failed(error_class(&other)),
                })?;
            for entry in entries {
                match entry.info {
                    Ok(info) if info.kind == EntryKind::Directory && !info.system => {
                        pending.push(dir.join(&entry.name));
                    }
                    _ => return Err(CleanSkipReason::NotEmpty),
                }
            }
        }
        Ok(())
    }

    /// Moves the shortcuts of `scan` at `indices` to the Recycle Bin. Each is looked at again
    /// right before: it must be unchanged and its target must still be missing.
    #[must_use]
    pub fn trash_shortcuts(&self, scan: &TidyScan, indices: &[usize], source: Source) -> TidyTrash {
        let chosen: BTreeSet<usize> = indices.iter().copied().collect();
        let mut result = TidyTrash::new();
        for shortcut in chosen
            .into_iter()
            .filter_map(|at| scan.broken_shortcuts.get(at))
        {
            let outcome = match self.verdict(&shortcut.path, shortcut.size_bytes) {
                Verdict::Broken(_) => self.trash_file(
                    &shortcut.root,
                    &shortcut.path,
                    shortcut.size_bytes,
                    shortcut.modified,
                ),
                // The target is back, or cannot be judged any more.
                Verdict::Fine | Verdict::Unchecked => Err(CleanSkipReason::Changed),
            };
            match outcome {
                Ok(()) => result.moved.push(TidyMoved {
                    path: shortcut.path.clone(),
                    size_bytes: shortcut.size_bytes,
                }),
                Err(reason) => result.skipped.push((shortcut.path.clone(), reason)),
            }
        }
        result.activity_write_failed = !self.record(&result, Kind::Shortcuts, source);
        result
    }

    /// Moves one file after checking it is still the file the scan saw.
    fn trash_file(
        &self,
        root: &Path,
        path: &Path,
        size_bytes: u64,
        modified: Option<Timestamp>,
    ) -> Result<(), CleanSkipReason> {
        match self.file_system.inspect(path) {
            Ok(Some(info)) => match info.kind {
                EntryKind::File => {
                    if info.size_bytes != size_bytes || info.modified != modified {
                        return Err(CleanSkipReason::Changed);
                    }
                }
                EntryKind::ReparsePoint => return Err(CleanSkipReason::Link),
                _ => return Err(CleanSkipReason::Changed),
            },
            Ok(None) => return Err(CleanSkipReason::Gone),
            Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
        }
        self.move_to_trash(root, path)
    }

    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), CleanSkipReason> {
        self.file_system
            .move_to_trash(root, path)
            .map_err(|error| match error {
                PortError::NotFound(_) => CleanSkipReason::Gone,
                PortError::AccessDenied(_) => CleanSkipReason::LockedOrDenied,
                PortError::InvalidInput(_) => CleanSkipReason::Refused,
                other => CleanSkipReason::Failed(error_class(&other)),
            })
    }

    /// Writes the activity record. `true` when written or nothing needed writing.
    fn record(&self, result: &TidyTrash, kind: Kind, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        if result.moved.is_empty() && result.skipped.is_empty() {
            return true;
        }
        let (code, label) = match kind {
            Kind::Duplicates => ("summary.tidyDuplicatesMoved", "tidy_duplicates"),
            Kind::Folders => ("summary.tidyFoldersMoved", "tidy_folders"),
            Kind::Shortcuts => ("summary.tidyShortcutsMoved", "tidy_shortcuts"),
        };
        let mut phrase = Phrase::new(code).count("count", result.moved.len());
        if !matches!(kind, Kind::Folders) {
            phrase = phrase.size("size", result.moved_bytes());
        }
        let phrase = phrase.maybe(
            "left",
            (!result.skipped.is_empty())
                .then(|| Phrase::new("summary.tidyLeft").count("count", result.skipped.len())),
        );
        let outcome = if result.moved.is_empty() {
            ActivityOutcome::Failed
        } else {
            ActivityOutcome::Applied
        };
        let activity = NewActivity::said(
            self.clock.now(),
            source,
            outcome,
            phrase,
            Some(label.to_owned()),
        );
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for {label} ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

/// The topmost empty folder of every empty tree, by path.
fn empty_folders(nodes: &mut [Node], roots: &[PathBuf]) -> Vec<EmptyFolder> {
    // A folder with anything in it, or that may have, makes every folder above it not empty.
    // Children come after their parents, so one pass from the end reaches the roots.
    for at in (0..nodes.len()).rev() {
        if nodes[at].blocked
            && let Some(parent) = nodes[at].parent
        {
            nodes[parent].blocked = true;
        }
    }
    let mut inside = vec![0_u32; nodes.len()];
    for at in (0..nodes.len()).rev() {
        if !nodes[at].blocked
            && let Some(parent) = nodes[at].parent
        {
            inside[parent] = inside[parent].saturating_add(inside[at]).saturating_add(1);
        }
    }
    let mut found: Vec<EmptyFolder> = (0..nodes.len())
        .filter(|&at| !nodes[at].blocked)
        .filter_map(|at| {
            // A scanned folder is never offered; below it, the topmost empty folder is.
            let parent = nodes[at].parent?;
            (nodes[parent].parent.is_none() || nodes[parent].blocked).then(|| EmptyFolder {
                path: node_path(nodes, roots, at),
                root: roots[nodes[at].root].clone(),
                nested: inside[at],
            })
        })
        .collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found
}

#[cfg(test)]
mod tests;
