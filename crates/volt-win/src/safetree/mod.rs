//! The tree walker of machine-wide cleaning: a handle-based scan and delete that cannot be steered
//! out of its root.
//!
//! It serves the elevated helper's `clean_scan` and `clean_apply` (see the decision on the helper).
//! The rules it keeps, each one tested in `tests/safetree*.rs`:
//!
//! - **Every open is relative to a held parent handle** (`NtCreateFile` with `RootDirectory` and one
//!   name). A path is never resolved from the top twice, so swapping a folder for a junction, a
//!   mount point or another folder between two steps cannot redirect a later step.
//! - **The root is walked down from the volume**, which is opened by its volume GUID, one component
//!   at a time, every ancestor held (shared for read and write, not for delete) for the whole run, so
//!   no ancestor can be renamed or removed while the walk is in it.
//! - **A reparse point is never followed**: junctions, symbolic links, mount points and cloud
//!   placeholders are opened as themselves, counted and left alone. A root that is one, or has one on
//!   the way to it, is refused.
//! - **Identity is checked on the opened handle**: every folder and file records its 128-bit file id
//!   at scan, and apply deletes an item only when the handle it just opened has the same id, is the
//!   same kind of entry, has one name only (a file with another hard link is left alone), and still
//!   has the size and time the scan saw. The delete itself goes through that handle (POSIX
//!   semantics), not through a path.
//! - **Folders a non-administrator owns or can write to** are skipped when the rule is silent (see
//!   [`Policy::skip_user_writable`]); the access list is read through the held handle, at scan and
//!   again at apply.
//! - **Everything is capped and stoppable**: depth, items, entries and time ([`Limits`],
//!   [`Control`]); [`run_bounded`] cancels a call stuck in the kernel.
//!
//! No name ever leaves this module: [`TreeScan`] and [`ApplyResult`] hold counts, sizes and skip
//! reasons only.

mod bounded;
mod nt;

use std::collections::HashMap;
use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use volt_core::clean::MAX_ITEMS_PER_RULE;
use volt_core::clean::age_allows;
use volt_core::clean::machine::{ApplySkips, MACHINE_MAX_ENTRIES, ScanSkips};
use volt_core::{CleaningRule, Location, SkipReason, Timestamp};
use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows::Win32::UI::Shell::FOLDERID_ProgramData;

use crate::folders::shell_folder;
use windows::Win32::Storage::FileSystem::{GetVolumeNameForVolumeMountPointW, GetVolumePathNameW};
use windows::core::PCWSTR;

pub use bounded::{Stuck, run_bounded};
use nt::{FileId, Meta, Status, Want};

use crate::helper::OwnedHandle;
use crate::helper::acl::of_file_handle;
use crate::helper::install::check_security;

/// How far a walk goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Folder levels below the root (a rule's own depth is applied too, and is never deeper).
    pub max_depth: u32,
    /// Items one scan may plan.
    pub max_items: usize,
    /// Entries one scan may look at.
    pub max_entries: usize,
}

impl Default for Limits {
    /// The decision's caps: depth 32, 25,000 items, 200,000 entries.
    fn default() -> Self {
        Self {
            max_depth: volt_core::clean::MAX_DEPTH_LIMIT,
            max_items: MAX_ITEMS_PER_RULE,
            max_entries: MACHINE_MAX_ENTRIES,
        }
    }
}

/// Why a walk stopped before it finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Somebody asked it to stop.
    Cancelled,
    /// Its time ran out.
    TimedOut,
}

/// A cancel flag and a deadline. Checked before every entry.
#[derive(Debug, Clone)]
pub struct Control {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
}

impl Control {
    /// A control that stops the walk when `cancel` is set or `budget` has passed.
    #[must_use]
    pub fn new(cancel: Arc<AtomicBool>, budget: Duration) -> Self {
        Self {
            cancel,
            deadline: Instant::now() + budget,
        }
    }

    /// A control with no one to cancel it and a generous budget, for tests.
    #[must_use]
    pub fn unbounded() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)), Duration::from_secs(3600))
    }

    /// The flag a caller sets to stop the walk.
    #[must_use]
    pub fn cancel_flag(&self) -> &Arc<AtomicBool> {
        &self.cancel
    }

    /// The time the walk must end by.
    #[must_use]
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    fn stop(&self) -> Option<Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Some(Stop::Cancelled)
        } else if Instant::now() >= self.deadline {
            Some(Stop::TimedOut)
        } else {
            None
        }
    }
}

/// What a walk takes and when.
#[derive(Debug, Clone)]
pub struct Policy {
    /// The rule: include and exclude patterns, depth, whether empty folders go too.
    pub rule: CleaningRule,
    /// The minimum age in days (the catalogue's, or a higher one).
    pub min_age_days: u32,
    /// The time ages are measured against.
    pub now: Timestamp,
    /// Whether a folder that a non-administrator owns or can write to is left alone, with
    /// everything in it. `true` for a rule that runs without a confirmation: its folder is meant
    /// to be one only administrators can change. A rule that a person confirms every time does not
    /// need it.
    pub skip_user_writable: bool,
    /// Paths (lower case, plain `C:\...` form, see `volt_core::clean::machine::pending_paths`) that
    /// Windows has queued to rename or delete at the next restart. A file on the list, and a
    /// folder that is on it or holds one, is left for Windows.
    pub pending_restart: Vec<String>,
}

/// Why a root could not be walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeError {
    /// It, or a folder on the way to it, does not exist.
    RootMissing,
    /// It, or a folder on the way to it, is a link or not a folder; or the path is not one this walker
    /// takes; or it is not the folder the scan was made for.
    RootRefused,
    /// Windows refused access.
    AccessDenied,
    /// It could not be opened for another reason.
    Unreadable,
    /// The rule is not one this walker serves (it takes whole entries, or is picked by item).
    UnsupportedRule,
}

/// A folder the scan went into.
#[derive(Debug, Clone)]
struct DirRec {
    /// Index of the parent (`usize::MAX` for the root).
    parent: usize,
    name: Vec<u16>,
    id: FileId,
    /// Whether the folder itself is planned for deletion.
    is_item: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    File,
    EmptyFolder,
}

/// One planned deletion.
#[derive(Debug, Clone)]
struct ItemRec {
    kind: ItemKind,
    /// For a file, the folder it is in; for a folder, the folder itself.
    dir: usize,
    name: Vec<u16>,
    id: FileId,
    size: u64,
    modified_ms: Option<i64>,
}

/// What a scan found: counts and sizes, and the identity of everything it planned, which apply
/// checks again. Holds no name that could be shown: [`TreeScan::files`] and the other getters are
/// all a caller gets.
#[derive(Debug, Clone)]
pub struct TreeScan {
    root_id: FileId,
    dirs: Vec<DirRec>,
    items: Vec<ItemRec>,
    files: u64,
    folders: u64,
    bytes: u64,
    skips: ScanSkips,
    truncated: bool,
    stopped: Option<Stop>,
    root_writable_by_users: bool,
}

impl TreeScan {
    /// Files planned for deletion.
    #[must_use]
    pub fn files(&self) -> u64 {
        self.files
    }

    /// Empty folders planned for deletion.
    #[must_use]
    pub fn folders(&self) -> u64 {
        self.folders
    }

    /// Size of the planned files, in bytes.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// What was left out.
    #[must_use]
    pub fn skips(&self) -> ScanSkips {
        self.skips
    }

    /// Whether a limit ended the scan: there is more than what is planned.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Why the scan stopped early, when it did.
    #[must_use]
    pub fn stopped(&self) -> Option<Stop> {
        self.stopped
    }

    /// Whether the root's access list lets a non-administrator write to it (the folder a normal
    /// user can plant files in), so a clean needs a person's confirmation.
    #[must_use]
    pub fn root_writable_by_users(&self) -> bool {
        self.root_writable_by_users
    }
}

/// What an apply did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ApplyResult {
    /// Files deleted.
    pub files: u64,
    /// Empty folders deleted.
    pub folders: u64,
    /// Size of the deleted files, in bytes.
    pub bytes: u64,
    /// What was left in place.
    pub skipped: ApplySkips,
    /// Why the apply stopped before it had looked at every item, when it did.
    pub stopped: Option<Stop>,
}

/// A hook the tests of the races use: it is called at the points where an attacker would swap
/// something, so a test can swap it there, every time, instead of hoping to win a timing. The
/// product passes [`NoProbe`].
pub trait Probe: Send + Sync {
    /// Whether the callbacks are wanted (the relative text is built only then).
    fn enabled(&self) -> bool {
        false
    }

    /// The folders above the item are held and the item has not been opened yet. `item` is its path
    /// below the root, with `/` between the components.
    fn before_open(&self, _item: &str) {}

    /// The item's identity was checked on the handle that is held, and the delete is next.
    fn before_delete(&self, _item: &str) {}
}

/// The probe of the product: does nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoProbe;

impl Probe for NoProbe {}

// ---------------------------------------------------------------------------------------------
// Roots

/// A held folder: its handle and what it is.
#[derive(Debug)]
struct Dir {
    handle: OwnedHandle,
    id: FileId,
    /// Whether the handle has the right to delete the folder.
    deletable: bool,
}

/// The root and every ancestor above it down from the volume, all held until this is dropped.
#[derive(Debug)]
struct Held {
    _ancestors: Vec<OwnedHandle>,
    root: Dir,
}

fn valid_component(name: &[u16]) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != [u16::from(b'.')]
        && name != [u16::from(b'.'), u16::from(b'.')]
        && name.iter().all(|&unit| {
            unit != 0
                && unit != u16::from(b'\\')
                && unit != u16::from(b'/')
                && unit != u16::from(b':')
        })
}

fn status_error(status: Status) -> TreeError {
    match status {
        nt::STATUS_OBJECT_NAME_NOT_FOUND
        | nt::STATUS_OBJECT_PATH_NOT_FOUND
        | nt::STATUS_NO_SUCH_FILE => TreeError::RootMissing,
        nt::STATUS_ACCESS_DENIED => TreeError::AccessDenied,
        nt::STATUS_NOT_A_DIRECTORY => TreeError::RootRefused,
        _ => TreeError::Unreadable,
    }
}

/// The `\??\Volume{guid}\` name of the volume `path` is on, and the components below its mount
/// point.
fn volume_of(path: &Path) -> Result<(Vec<u16>, Vec<Vec<u16>>), TreeError> {
    if !path.is_absolute() {
        return Err(TreeError::RootRefused);
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut mount = [0_u16; 520];
    // SAFETY: `wide` is NUL-terminated and outlives the call; `mount` is a valid buffer.
    unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut mount) }
        .map_err(|_| TreeError::RootRefused)?;
    let mut guid = [0_u16; 64];
    // SAFETY: `mount` holds the NUL-terminated volume path the call above wrote; `guid` is a valid
    // buffer of the length passed.
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid) }
        .map_err(|_| TreeError::RootRefused)?;
    let guid_len = guid
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(guid.len());
    let mount_len = mount
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(mount.len());
    let mut nt_path: Vec<u16> = guid[..guid_len].to_vec();
    // `\\?\Volume{guid}\` becomes `\??\Volume{guid}\`, the form the native calls take.
    if nt_path.len() < 4 || nt_path[..4] != [92, 92, 63, 92] {
        return Err(TreeError::RootRefused);
    }
    nt_path[1] = u16::from(b'?');

    // The path below the mount point, compared without regard to case.
    let mount_text = OsString::from_wide(&mount[..mount_len])
        .to_string_lossy()
        .to_lowercase();
    let mount_text = mount_text.trim_start_matches(r"\\?\").to_owned();
    let full = path.to_string_lossy().to_lowercase();
    let full = full.trim_start_matches(r"\\?\").to_owned();
    if !full.starts_with(&mount_text) {
        return Err(TreeError::RootRefused);
    }
    let below_len = path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .chars()
        .skip(mount_text.chars().count())
        .collect::<String>();
    let mut parts = Vec::new();
    for component in Path::new(&below_len).components() {
        match component {
            Component::Normal(name) => {
                let name: Vec<u16> = name.encode_wide().collect();
                if !valid_component(&name) {
                    return Err(TreeError::RootRefused);
                }
                parts.push(name);
            }
            Component::RootDir | Component::CurDir => {}
            _ => return Err(TreeError::RootRefused),
        }
    }
    Ok((nt_path, parts))
}

/// The Windows folder as `GetSystemWindowsDirectory` says, which no environment variable of a user
/// can change.
fn system_windows_directory() -> Result<PathBuf, TreeError> {
    let mut buffer = [0_u16; 520];
    // SAFETY: `buffer` is a valid buffer of the length the slice carries.
    let length = unsafe { GetSystemWindowsDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(TreeError::Unreadable);
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

/// The folder a machine-wide rule works in: the Windows folder or `ProgramData`, asked of the
/// operating system (never the environment, which a user controls), plus the rule's relative path.
///
/// # Errors
/// [`TreeError::RootRefused`] for any other location, an empty relative path (a rule never cleans a
/// whole location) or a component that is not a plain name; [`TreeError::Unreadable`] when the
/// system cannot say where the folder is.
pub fn machine_root(location: Location, relative: &[String]) -> Result<PathBuf, TreeError> {
    let base = match location {
        Location::Windows => system_windows_directory(),
        Location::ProgramData => {
            shell_folder(&FOLDERID_ProgramData, "ProgramData").map_err(|_| TreeError::Unreadable)
        }
        _ => return Err(TreeError::RootRefused),
    }?;
    if relative.is_empty() {
        return Err(TreeError::RootRefused);
    }
    let mut root = base;
    for part in relative {
        let wide: Vec<u16> = part.encode_utf16().collect();
        if !valid_component(&wide) {
            return Err(TreeError::RootRefused);
        }
        root.push(part);
    }
    Ok(root)
}

/// Opens `name` below `parent` as a folder and checks that it is one and is not a link.
fn open_checked(parent: Option<&OwnedHandle>, name: &[u16], want: Want) -> Result<Dir, Status> {
    let handle = nt::open_at(parent.map(OwnedHandle::raw), name, want)?;
    let meta = nt::meta(&handle)?;
    if meta.is_reparse_point() {
        return Err(nt::STATUS_NOT_A_DIRECTORY);
    }
    if !meta.is_directory {
        return Err(nt::STATUS_NOT_A_DIRECTORY);
    }
    Ok(Dir {
        handle,
        id: meta.id,
        deletable: matches!(want, Want::DirectoryToDelete),
    })
}

/// Walks down from the volume to `path`, holding every folder on the way.
fn open_root(path: &Path) -> Result<Held, TreeError> {
    let (volume, parts) = volume_of(path)?;
    let mut ancestors = Vec::new();
    let mut current = open_checked(None, &volume, Want::Directory).map_err(status_error)?;
    for part in &parts {
        let next =
            open_checked(Some(&current.handle), part, Want::Directory).map_err(status_error)?;
        ancestors.push(std::mem::replace(&mut current, next).handle);
    }
    Ok(Held {
        _ancestors: ancestors,
        root: current,
    })
}

/// Whether the folder's owner or access list lets a non-administrator change it, read through the
/// handle. A list that cannot be read counts as writable: unknown is never trusted.
fn writable_by_users(handle: &OwnedHandle) -> bool {
    match of_file_handle(handle) {
        Ok(info) => check_security(&info).is_err(),
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------------------------
// Scan

/// Scans `root` for what `policy` would delete. Read-only.
///
/// # Errors
/// [`TreeError`] when the root cannot be used; a scan that is cut short by a limit or a stop is not
/// an error (see [`TreeScan::truncated`] and [`TreeScan::stopped`]).
pub fn scan(
    root: &Path,
    policy: &Policy,
    limits: &Limits,
    control: &Control,
) -> Result<TreeScan, TreeError> {
    check_rule(&policy.rule)?;
    let held = open_root(root)?;
    let root_writable_by_users = writable_by_users(&held.root.handle);
    let mut scanner = Scanner {
        policy,
        limits,
        control,
        dirs: vec![DirRec {
            parent: usize::MAX,
            name: Vec::new(),
            id: held.root.id,
            is_item: false,
        }],
        items: Vec::new(),
        skips: ScanSkips::default(),
        entries: 0,
        truncated: false,
        stopped: None,
        relative: Vec::new(),
        volume: held.root.id.volume,
        root_text: plain_lower(root),
        max_depth: policy
            .rule
            .constraints
            .max_depth
            .min(limits.max_depth)
            .min(volt_core::clean::MAX_DEPTH_LIMIT),
    };
    if let Err(status) = scanner.walk(&held.root, 0, 0) {
        // The root itself could not be listed.
        return Err(status_error(status));
    }
    let (mut files, mut folders, mut bytes) = (0_u64, 0_u64, 0_u64);
    for item in &scanner.items {
        match item.kind {
            ItemKind::File => {
                files += 1;
                bytes = bytes.saturating_add(item.size);
            }
            ItemKind::EmptyFolder => folders += 1,
        }
    }
    Ok(TreeScan {
        root_id: held.root.id,
        dirs: scanner.dirs,
        items: scanner.items,
        files,
        folders,
        bytes,
        skips: scanner.skips,
        truncated: scanner.truncated,
        stopped: scanner.stopped,
        root_writable_by_users,
    })
}

fn check_rule(rule: &CleaningRule) -> Result<(), TreeError> {
    if rule.unit == volt_core::ItemUnit::Files && !rule.pick {
        Ok(())
    } else {
        Err(TreeError::UnsupportedRule)
    }
}

struct Scanner<'a> {
    policy: &'a Policy,
    limits: &'a Limits,
    control: &'a Control,
    dirs: Vec<DirRec>,
    items: Vec<ItemRec>,
    skips: ScanSkips,
    entries: usize,
    truncated: bool,
    stopped: Option<Stop>,
    relative: Vec<String>,
    volume: u64,
    max_depth: u32,
    /// The root as a lower-case plain path, for the pending list.
    root_text: String,
}

/// A path as the pending list spells it: lower case, no `\\?\` prefix, no trailing separator.
fn plain_lower(path: &Path) -> String {
    let text = path.to_string_lossy().to_lowercase();
    text.trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .to_owned()
}

/// `root` and the components below it, lower case, joined with `\\`.
fn join_lower(root: &str, parts: &[String]) -> String {
    let mut text = root.to_owned();
    for part in parts {
        text.push('\\');
        text.push_str(&part.to_lowercase());
    }
    text
}

/// Whether Windows has `path` queued, or something inside it when `folder` is true.
fn pending_blocks(pending: &[String], path: &str, folder: bool) -> bool {
    pending.iter().any(|queued| {
        queued == path
            || (folder && queued.starts_with(path) && queued[path.len()..].starts_with('\\'))
    })
}

impl Scanner<'_> {
    fn halted(&mut self) -> bool {
        if self.truncated || self.stopped.is_some() {
            return true;
        }
        if let Some(stop) = self.control.stop() {
            self.stopped = Some(stop);
            self.truncated = true;
            return true;
        }
        false
    }

    fn at_cap(&mut self) -> bool {
        if self.items.len() >= self.limits.max_items {
            self.truncated = true;
        }
        self.truncated
    }

    fn age_ok(&mut self, modified_ms: Option<i64>) -> bool {
        match age_allows(
            self.policy.min_age_days,
            modified_ms.map(Timestamp::from_unix_millis),
            self.policy.now,
        ) {
            Ok(()) => true,
            Err(SkipReason::AgeUnknown) => {
                self.skips.age_unknown += 1;
                false
            }
            Err(_) => {
                self.skips.too_new += 1;
                false
            }
        }
    }

    /// Lists the held folder `dir` (record `index`, `depth` levels below the root) and plans what
    /// the rule takes. Returns whether everything in it is planned, so it will be empty after a
    /// clean. The root's own listing failing is the one error.
    fn walk(&mut self, dir: &Dir, index: usize, depth: u32) -> Result<bool, Status> {
        let entries = match nt::list(&dir.handle) {
            Ok(entries) => entries,
            Err(status) if depth == 0 => return Err(status),
            Err(status) => {
                if status == nt::STATUS_ACCESS_DENIED {
                    self.skips.access_denied += 1;
                } else {
                    self.skips.unreadable += 1;
                }
                return Ok(false);
            }
        };
        let mut all_planned = true;
        for entry in entries {
            if self.halted() {
                return Ok(false);
            }
            self.entries += 1;
            if self.entries > self.limits.max_entries {
                self.truncated = true;
                return Ok(false);
            }
            let Ok(text) = String::from_utf16(&entry.name) else {
                self.skips.invalid_name += 1;
                all_planned = false;
                continue;
            };
            if !valid_component(&entry.name) {
                self.skips.invalid_name += 1;
                all_planned = false;
                continue;
            }
            self.relative.push(text);
            let planned = if entry.is_reparse_point() {
                // A link, junction, mount point or placeholder: described as itself, never opened
                // as what it points at.
                self.skips.links += 1;
                false
            } else if entry.is_directory() {
                self.directory(dir, index, depth, &entry)
            } else {
                self.file(index, &entry)
            };
            self.relative.pop();
            all_planned &= planned;
        }
        Ok(all_planned)
    }

    fn file(&mut self, index: usize, entry: &nt::Entry) -> bool {
        if self.policy.rule.excludes(&self.relative) || !self.policy.rule.includes(&self.relative) {
            return false;
        }
        if !self.policy.pending_restart.is_empty()
            && pending_blocks(
                &self.policy.pending_restart,
                &join_lower(&self.root_text, &self.relative),
                false,
            )
        {
            self.skips.pending_restart += 1;
            return false;
        }
        if !self.age_ok(entry.modified_ms) || self.at_cap() {
            return false;
        }
        self.items.push(ItemRec {
            kind: ItemKind::File,
            dir: index,
            name: entry.name.clone(),
            id: FileId {
                volume: self.volume,
                id: entry.id_bytes,
            },
            size: entry.size,
            modified_ms: entry.modified_ms,
        });
        true
    }

    fn directory(
        &mut self,
        parent: &Dir,
        parent_index: usize,
        depth: u32,
        entry: &nt::Entry,
    ) -> bool {
        if self.policy.rule.excludes(&self.relative) {
            return false;
        }
        if depth >= self.max_depth {
            self.skips.too_deep += 1;
            return false;
        }
        let child = match nt::open_at(Some(parent.handle.raw()), &entry.name, Want::Directory) {
            Ok(handle) => handle,
            Err(status) => {
                if status == nt::STATUS_ACCESS_DENIED {
                    self.skips.access_denied += 1;
                } else {
                    self.skips.unreadable += 1;
                }
                return false;
            }
        };
        // What the handle says, not what the listing said a moment ago.
        let Ok(meta) = nt::meta(&child) else {
            self.skips.unreadable += 1;
            return false;
        };
        let listed = FileId {
            volume: self.volume,
            id: entry.id_bytes,
        };
        if meta.is_reparse_point() {
            self.skips.links += 1;
            return false;
        }
        if !meta.is_directory || meta.id != listed {
            self.skips.unreadable += 1;
            return false;
        }
        if self.policy.skip_user_writable && writable_by_users(&child) {
            self.skips.user_writable += 1;
            return false;
        }
        let index = self.dirs.len();
        self.dirs.push(DirRec {
            parent: parent_index,
            name: entry.name.clone(),
            id: meta.id,
            is_item: false,
        });
        let held = Dir {
            handle: child,
            id: meta.id,
            deletable: false,
        };
        let emptied = self.walk(&held, index, depth + 1).unwrap_or(false);
        if !emptied || !self.policy.rule.constraints.remove_empty_dirs {
            return false;
        }
        if !self.policy.pending_restart.is_empty() {
            let path = join_lower(&self.root_text, &self.relative);
            if pending_blocks(&self.policy.pending_restart, &path, true) {
                // Counted when it is the folder itself that is queued; a queued file inside was
                // counted where it was found.
                if self.policy.pending_restart.contains(&path) {
                    self.skips.pending_restart += 1;
                }
                return false;
            }
        }
        if !self.age_ok(entry.modified_ms) || self.at_cap() {
            return false;
        }
        self.dirs[index].is_item = true;
        self.items.push(ItemRec {
            kind: ItemKind::EmptyFolder,
            dir: index,
            name: entry.name.clone(),
            id: meta.id,
            size: 0,
            modified_ms: entry.modified_ms,
        });
        true
    }
}

// ---------------------------------------------------------------------------------------------
// Apply

/// Why a folder on the way to an item could not be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Skip {
    Gone,
    Changed,
    Link,
    HardLink,
    NotEmpty,
    Locked,
    UserWritable,
    PendingRestart,
    Failed,
}

fn skip_of(status: Status) -> Skip {
    match status {
        nt::STATUS_OBJECT_NAME_NOT_FOUND
        | nt::STATUS_OBJECT_PATH_NOT_FOUND
        | nt::STATUS_NO_SUCH_FILE => Skip::Gone,
        nt::STATUS_ACCESS_DENIED
        | nt::STATUS_SHARING_VIOLATION
        | nt::STATUS_DELETE_PENDING
        | nt::STATUS_CANNOT_DELETE => Skip::Locked,
        nt::STATUS_NOT_A_DIRECTORY | nt::STATUS_FILE_IS_A_DIRECTORY => Skip::Changed,
        nt::STATUS_DIRECTORY_NOT_EMPTY => Skip::NotEmpty,
        _ => Skip::Failed,
    }
}

struct Applier<'a> {
    scan: &'a TreeScan,
    policy: &'a Policy,
    probe: &'a dyn Probe,
    /// Folders held so far, by record index (0 is the root).
    open: HashMap<usize, Dir>,
    /// Folders that could not be held, and why.
    refused: HashMap<usize, Skip>,
    result: ApplyResult,
    /// The root as a lower-case plain path, for the pending list.
    root_text: String,
}

/// Deletes what `scan` planned that is still the same, one item at a time, each through a handle
/// opened relative to its held parent (see the module documentation).
///
/// # Errors
/// [`TreeError`] when the root cannot be opened, or is not the folder the scan was made for.
pub fn apply(
    root: &Path,
    scan: &TreeScan,
    policy: &Policy,
    control: &Control,
    probe: &dyn Probe,
) -> Result<ApplyResult, TreeError> {
    check_rule(&policy.rule)?;
    let held = open_root(root)?;
    if held.root.id != scan.root_id {
        return Err(TreeError::RootRefused);
    }
    let mut applier = Applier {
        scan,
        policy,
        probe,
        open: HashMap::new(),
        refused: HashMap::new(),
        result: ApplyResult::default(),
        root_text: plain_lower(root),
    };
    applier.open.insert(0, held.root);
    for item in &scan.items {
        if let Some(stop) = control.stop() {
            applier.result.stopped = Some(stop);
            break;
        }
        match applier.item(item) {
            Ok(()) => match item.kind {
                ItemKind::File => {
                    applier.result.files += 1;
                    applier.result.bytes = applier.result.bytes.saturating_add(item.size);
                }
                ItemKind::EmptyFolder => applier.result.folders += 1,
            },
            Err(skip) => applier.count(skip),
        }
    }
    Ok(applier.result)
}

impl Applier<'_> {
    fn count(&mut self, skip: Skip) {
        let counts = &mut self.result.skipped;
        match skip {
            Skip::Gone => counts.gone += 1,
            Skip::Changed => counts.changed += 1,
            Skip::Link => counts.links += 1,
            Skip::HardLink => counts.hard_links += 1,
            Skip::NotEmpty => counts.not_empty += 1,
            Skip::Locked => counts.locked += 1,
            Skip::UserWritable => counts.user_writable += 1,
            Skip::PendingRestart => counts.pending_restart += 1,
            Skip::Failed => counts.failed += 1,
        }
    }

    /// The path of an item below the root, for a probe. Built only when a probe wants it.
    fn relative(&self, dir: usize, name: &[u16]) -> String {
        if !self.probe.enabled() {
            return String::new();
        }
        let mut parts = Vec::new();
        let mut at = dir;
        while at != 0 && at != usize::MAX {
            let rec = &self.scan.dirs[at];
            parts.push(String::from_utf16_lossy(&rec.name));
            at = rec.parent;
        }
        parts.reverse();
        if !name.is_empty() {
            parts.push(String::from_utf16_lossy(name));
        }
        parts.join("/")
    }

    /// The path of an item as the pending list spells it (lower case), or `None` when the policy
    /// has no pending list.
    fn pending_path(&self, dir: usize, name: &[u16]) -> Option<String> {
        if self.policy.pending_restart.is_empty() {
            return None;
        }
        let mut parts = Vec::new();
        let mut at = dir;
        while at != 0 && at != usize::MAX {
            let rec = &self.scan.dirs[at];
            parts.push(String::from_utf16_lossy(&rec.name));
            at = rec.parent;
        }
        parts.reverse();
        if !name.is_empty() {
            parts.push(String::from_utf16_lossy(name));
        }
        Some(join_lower(&self.root_text, &parts))
    }

    /// The held folder for record `index`, opened below its own held parent the first time and
    /// checked like the scan checked it: not a link, a folder, the same object, and (for a silent
    /// rule) not writable by a non-administrator.
    fn dir(&mut self, index: usize) -> Result<(), Skip> {
        if self.open.contains_key(&index) {
            return Ok(());
        }
        if let Some(skip) = self.refused.get(&index) {
            return Err(*skip);
        }
        let outcome = self.open_dir(index);
        if let Err(skip) = outcome {
            self.refused.insert(index, skip);
        }
        outcome
    }

    fn open_dir(&mut self, index: usize) -> Result<(), Skip> {
        let rec = &self.scan.dirs[index];
        let (parent, name, id, is_item) = (rec.parent, rec.name.clone(), rec.id, rec.is_item);
        self.dir(parent)?;
        let parent_handle = self
            .open
            .get(&parent)
            .map(|dir| dir.handle.raw())
            .ok_or(Skip::Failed)?;
        // A folder that is itself planned is opened with the right to delete it. When the access
        // list does not give that, it is still held to work below, and its own delete is skipped.
        let first = if is_item {
            Want::DirectoryToDelete
        } else {
            Want::Directory
        };
        let handle = match nt::open_at(Some(parent_handle), &name, first) {
            Err(nt::STATUS_ACCESS_DENIED) if is_item => {
                nt::open_at(Some(parent_handle), &name, Want::Directory)
            }
            other => other,
        }
        .map_err(skip_of)?;
        let meta = nt::meta(&handle).map_err(skip_of)?;
        if meta.is_reparse_point() {
            return Err(Skip::Link);
        }
        if !meta.is_directory || meta.id != id {
            return Err(Skip::Changed);
        }
        if self.policy.skip_user_writable && writable_by_users(&handle) {
            return Err(Skip::UserWritable);
        }
        self.open.insert(
            index,
            Dir {
                handle,
                id: meta.id,
                deletable: is_item,
            },
        );
        Ok(())
    }

    fn item(&mut self, item: &ItemRec) -> Result<(), Skip> {
        match item.kind {
            ItemKind::File => self.delete_file(item),
            ItemKind::EmptyFolder => self.delete_folder(item),
        }
    }

    fn delete_file(&mut self, item: &ItemRec) -> Result<(), Skip> {
        if let Some(path) = self.pending_path(item.dir, &item.name)
            && pending_blocks(&self.policy.pending_restart, &path, false)
        {
            return Err(Skip::PendingRestart);
        }
        self.dir(item.dir)?;
        let shown = self.relative(item.dir, &item.name);
        self.probe.before_open(&shown);
        let parent = self
            .open
            .get(&item.dir)
            .map(|dir| dir.handle.raw())
            .ok_or(Skip::Failed)?;
        let handle = nt::open_at(Some(parent), &item.name, Want::FileToDelete).map_err(skip_of)?;
        let meta = nt::meta(&handle).map_err(skip_of)?;
        self.check_file(item, &meta)?;
        self.probe.before_delete(&shown);
        nt::delete(&handle).map_err(skip_of)
    }

    /// Whether the file that was opened is the one the scan planned.
    fn check_file(&self, item: &ItemRec, meta: &Meta) -> Result<(), Skip> {
        if meta.is_reparse_point() {
            return Err(Skip::Link);
        }
        if meta.is_directory || meta.id != item.id {
            return Err(Skip::Changed);
        }
        if meta.links != 1 {
            return Err(Skip::HardLink);
        }
        if meta.size != item.size || meta.modified_ms != item.modified_ms {
            return Err(Skip::Changed);
        }
        age_allows(
            self.policy.min_age_days,
            meta.modified_ms.map(Timestamp::from_unix_millis),
            self.policy.now,
        )
        .map_err(|_| Skip::Changed)
    }

    fn delete_folder(&mut self, item: &ItemRec) -> Result<(), Skip> {
        let rec = &self.scan.dirs[item.dir];
        let (name, parent) = (rec.name.clone(), rec.parent);
        if let Some(path) = self.pending_path(parent, &name)
            && pending_blocks(&self.policy.pending_restart, &path, true)
        {
            self.open.remove(&item.dir);
            return Err(Skip::PendingRestart);
        }
        let shown = self.relative(parent, &name);
        self.probe.before_open(&shown);
        let outcome = self.delete_held_folder(item, &shown);
        // The handle goes whatever happened: a folder is never held longer than its own turn.
        self.open.remove(&item.dir);
        outcome
    }

    fn delete_held_folder(&mut self, item: &ItemRec, shown: &str) -> Result<(), Skip> {
        self.dir(item.dir)?;
        let dir = self.open.get(&item.dir).ok_or(Skip::Failed)?;
        if !dir.deletable {
            return Err(Skip::Locked);
        }
        let meta = nt::meta(&dir.handle).map_err(skip_of)?;
        if meta.is_reparse_point() {
            return Err(Skip::Link);
        }
        if !meta.is_directory || meta.id != item.id {
            return Err(Skip::Changed);
        }
        self.probe.before_delete(shown);
        nt::delete(&dir.handle).map_err(skip_of)
    }
}

/// Whether the cancel flag or the deadline of `control` has ended the work.
#[must_use]
pub fn stopped(control: &Control) -> Option<Stop> {
    control.stop()
}
