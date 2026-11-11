//! Port traits for the outside world (except the registry and the journal,
//! which have their own modules) and the [`Ports`] bundle.
//!
//! All ports are synchronous, object-safe and `Send + Sync`. File system,
//! process and memory ports are deliberately minimal; later tasks extend them.

use std::ffi::OsString;
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::command::Commands;
use crate::error::PortError;
use crate::exclusions::ExclusionList;
use crate::registry::Registry;
use crate::service::ServiceStartType;
use crate::time::Timestamp;

/// Port: Windows services.
pub trait Services: Send + Sync {
    /// Current start type of a service. Unknown service is [`PortError::NotFound`].
    fn start_type(&self, service: &str) -> Result<ServiceStartType, PortError>;

    /// Sets the start type of a service. Single and atomic.
    fn set_start_type(&self, service: &str, start_type: ServiceStartType) -> Result<(), PortError>;
}

/// Metadata of a file or directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStat {
    /// Size in bytes (0 for directories).
    pub size_bytes: u64,
    /// Whether the path is a directory.
    pub is_dir: bool,
}

/// What a directory entry is. A reparse point (junction, symbolic link, mount
/// point, cloud placeholder) is never reported as a file or a directory, so a
/// caller cannot treat one as the thing it points to by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EntryKind {
    /// A plain file.
    File,
    /// A plain directory.
    Directory,
    /// A reparse point of any kind. It is described as itself, never followed.
    ReparsePoint,
}

/// What a file system entry looks like right now, without following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryInfo {
    /// What the entry is.
    pub kind: EntryKind,
    /// Size in bytes; 0 for anything but a plain file.
    pub size_bytes: u64,
    /// Last modification time, when the file system reports one.
    pub modified: Option<Timestamp>,
    /// Whether the entry has the system attribute (hidden operating system files).
    pub system: bool,
}

/// Who a file is on its volume, and whether its data is on disk.
///
/// Two paths with the same `volume` and `file_id` are two names of one file (hard links): they
/// share their data and are not duplicates of each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// The serial number of the volume the file is on.
    pub volume: u64,
    /// The file's index on that volume.
    pub file_id: u64,
    /// How many names (hard links) the file has.
    pub links: u32,
    /// `false` when the data is not on this disk (a cloud placeholder or an offline file):
    /// reading it would make Windows download it, so it is never read.
    pub on_disk: bool,
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// The entry's name: one path component, never `.` or `..`. Kept as an
    /// [`OsString`] so a name that is not valid Unicode still joins back to the
    /// real path.
    pub name: OsString,
    /// The entry's metadata, or why it could not be read. One unreadable entry
    /// does not fail the whole listing.
    pub info: Result<EntryInfo, PortError>,
}

/// Port: file system. File deletion is not an undoable [`Operation`](crate::Operation);
/// cleaning shows a preview and prefers the Recycle Bin.
pub trait FileSystem: Send + Sync {
    /// Metadata for a path. A missing path is `Ok(None)`.
    fn stat(&self, path: &Path) -> Result<Option<FileStat>, PortError>;

    /// Describes `path` itself without following a link: its kind (a reparse
    /// point is [`EntryKind::ReparsePoint`]), size and modification time. A
    /// missing path is `Ok(None)`.
    fn inspect(&self, path: &Path) -> Result<Option<EntryInfo>, PortError>;

    /// Lists the entries of the directory `dir`, sorted by name, without `.`
    /// and `..` and without descending. Read-only.
    ///
    /// Implementations must refuse with [`PortError::InvalidInput`] a `dir`
    /// that is a reparse point or not a directory, so a scan can never be led
    /// out of the folder it was asked to read by a junction or symlink. A
    /// missing directory is [`PortError::NotFound`].
    fn read_dir(&self, dir: &Path) -> Result<Vec<DirEntry>, PortError>;

    /// Moves `path` (a file or directory) to the Recycle Bin, provided it lies
    /// inside `root`.
    ///
    /// Implementations must refuse with [`PortError::InvalidInput`] a `path`
    /// whose resolved location is outside `root`, and one that reaches its
    /// target through a junction or symlink, so a cleaner can never be tricked
    /// into deleting outside the folder it was asked to clean. A missing path
    /// is [`PortError::NotFound`].
    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), PortError>;

    /// The identity of the plain file at `path`, read without reading its data and without
    /// following a link. The default is unsupported: an adapter that cannot tell makes the
    /// duplicate finder compare nothing.
    fn identity(&self, _path: &Path) -> Result<FileIdentity, PortError> {
        Err(PortError::Unsupported("file identity".to_owned()))
    }

    /// Reads up to `buf.len()` bytes of the plain file at `path` starting at `offset`, and
    /// returns how many were read (0 at the end). Read-only and shared: it never locks the file.
    ///
    /// Implementations must refuse with [`PortError::InvalidInput`] a link or other reparse
    /// point, and with [`PortError::Unsupported`] a file that is not on disk (see
    /// [`FileIdentity::on_disk`]), so reading can never download a cloud file or leave the
    /// folder through a link. The default is unsupported.
    fn read_at(&self, _path: &Path, _offset: u64, _buf: &mut [u8]) -> Result<usize, PortError> {
        Err(PortError::Unsupported("reading file data".to_owned()))
    }
}

/// A running process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Id of the process that started it. It can name a process that has ended (and the id may
    /// even have been reused), so it is a hint for grouping, never proof of ancestry.
    pub parent_pid: u32,
    /// Executable name, such as `chrome.exe`.
    pub name: String,
    /// Private working set in bytes: the memory only this process holds in RAM, which is the
    /// figure Task Manager shows. 0 when the adapter cannot read it.
    pub working_set_bytes: u64,
    /// CPU time the process has used since it started (kernel plus user), in 100 ns ticks. It only
    /// grows, so CPU load is the difference of two readings over the time between them.
    pub cpu_time_100ns: u64,
    /// When the process started, in 100 ns ticks (a Windows file time). A process id can be reused
    /// once its process has exited, so the pair (pid, creation time) is what identifies one
    /// process for good.
    pub created_at_100ns: u64,
    /// Full path of the executable, when it can be read without administrator rights. `None` for
    /// the protected processes a normal user cannot look into.
    pub path: Option<String>,
    /// The Windows session it runs in. Session 0 holds services and system processes, never a
    /// user's apps.
    pub session_id: u32,
}

/// Port: processes.
pub trait Processes: Send + Sync {
    /// Lists running processes, without the idle pseudo-process (pid 0).
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError>;

    /// Ends a process at once, without letting it save anything. `created_at_100ns` is the
    /// creation time the listing gave for `pid`; if the process now behind that id started at
    /// another time, the id was reused and nothing is ended.
    ///
    /// An unknown pid, or one whose creation time differs, is [`PortError::NotFound`] and a process
    /// the user may not end is [`PortError::AccessDenied`].
    fn terminate(&self, pid: u32, created_at_100ns: u64) -> Result<(), PortError>;

    /// Reads the settings of a process that Volt can change: priority class, CPU affinity and
    /// power throttling. `created_at_100ns` is the creation time the listing gave; another one
    /// means the id was reused and is [`PortError::NotFound`], like an unknown id.
    ///
    /// A process the unelevated user may not change (it runs elevated, belongs to another user or
    /// is protected) is [`PortError::AccessDenied`]: the adapter asks for the right to change it,
    /// so "readable" means "changeable". A priority class Volt does not offer (Realtime) is
    /// [`PortError::Unsupported`].
    fn settings(
        &self,
        pid: u32,
        created_at_100ns: u64,
    ) -> Result<crate::tuning::ProcessSettings, PortError>;

    /// Changes the settings a [`ProcessChange`](crate::tuning::ProcessChange) names and leaves
    /// the others alone. All or nothing: when one field cannot be set, the ones already set are
    /// put back and the error is returned. Errors as for [`Processes::settings`].
    fn apply_settings(
        &self,
        pid: u32,
        created_at_100ns: u64,
        change: &crate::tuning::ProcessChange,
    ) -> Result<(), PortError>;
}

/// Physical memory totals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStatus {
    /// Installed physical memory in bytes.
    pub total_physical_bytes: u64,
    /// Physical memory available for use in bytes.
    pub available_physical_bytes: u64,
}

/// A memory area that can be purged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MemoryArea {
    /// Trim the working sets of the processes Volt may open.
    WorkingSets,
    /// Flush the standby list.
    StandbyList,
    /// Flush only the low-priority part of the standby list (priority 0).
    StandbyListLowPriority,
    /// Flush the modified page list: write its pages to disk so they become standby pages.
    ModifiedList,
    /// Trim the system file cache.
    FileCache,
}

impl MemoryArea {
    /// Every area, in the order a run performs them. Trimming and flushing move pages onto the
    /// standby list, so the standby purges come last and see everything the earlier steps moved.
    pub const RUN_ORDER: [Self; 5] = [
        Self::WorkingSets,
        Self::FileCache,
        Self::ModifiedList,
        Self::StandbyListLowPriority,
        Self::StandbyList,
    ];

    /// What the area is called in a sentence ("the standby list").
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::WorkingSets => "working sets",
            Self::StandbyList => "standby list",
            Self::StandbyListLowPriority => "low-priority standby list",
            Self::ModifiedList => "modified list",
            Self::FileCache => "system file cache",
        }
    }

    /// [`MemoryArea::name`] as a phrase, for the UI to write in its own language.
    #[must_use]
    pub fn phrase(self) -> crate::phrase::Phrase {
        crate::phrase::Phrase::new(match self {
            Self::WorkingSets => "area.workingSets",
            Self::StandbyList => "area.standbyList",
            Self::StandbyListLowPriority => "area.standbyListLowPriority",
            Self::ModifiedList => "area.modifiedList",
            Self::FileCache => "area.fileCache",
        })
    }
}

/// Physical memory split by the lists Windows keeps pages on, in bytes. The four parts add up to
/// the total (`in_use` is what is left after the other three).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLists {
    /// Installed physical memory.
    pub total_bytes: u64,
    /// Pages in working sets, the kernel and drivers: memory that is not reclaimable for free.
    pub in_use_bytes: u64,
    /// Standby list: cached pages that still hold data and that Windows gives back on demand.
    pub standby_bytes: u64,
    /// Modified list: pages that must be written to disk before they can be reused.
    pub modified_bytes: u64,
    /// Free and zeroed pages.
    pub free_bytes: u64,
}

impl MemoryLists {
    /// Builds the split from the three lists a system reports; `in_use` is the remainder.
    #[must_use]
    pub fn from_lists(
        total_bytes: u64,
        standby_bytes: u64,
        modified_bytes: u64,
        free_bytes: u64,
    ) -> Self {
        let listed = standby_bytes
            .saturating_add(modified_bytes)
            .saturating_add(free_bytes);
        Self {
            total_bytes,
            in_use_bytes: total_bytes.saturating_sub(listed),
            standby_bytes,
            modified_bytes,
            free_bytes,
        }
    }
}

/// Whether an area can be purged right now, without trying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Availability {
    /// The purge can run.
    Available,
    /// The purge needs rights only an elevated (administrator) process has.
    NeedsAdministrator,
    /// This Windows build or adapter cannot do it.
    Unsupported,
}

/// What a purge did, beyond "it worked". Only trimming working sets has counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PurgeOutcome {
    /// Processes whose working set was trimmed.
    pub processes_trimmed: u32,
    /// Processes that could not be opened (protected or owned by another user) and were left.
    pub processes_skipped: u32,
    /// Processes that were left alone because they are on the exclusion list.
    pub processes_excluded: u32,
}

/// Port: memory. Purging is not an undoable operation: it only releases reclaimable memory.
pub trait Memory: Send + Sync {
    /// Current physical memory totals.
    fn status(&self) -> Result<MemoryStatus, PortError>;

    /// Physical memory by list. Reading changes nothing.
    fn lists(&self) -> Result<MemoryLists, PortError>;

    /// Whether `area` can be purged now. Reads only.
    fn availability(&self, area: MemoryArea) -> Availability;

    /// Purges a memory area. A missing privilege is [`PortError::AccessDenied`] and a call this
    /// Windows build does not have is [`PortError::Unsupported`]; neither changes anything.
    ///
    /// For [`MemoryArea::WorkingSets`] this trims every process, with nothing protected. The
    /// [`MemoryCleaner`](crate::MemoryCleaner) never calls it for that area: it calls
    /// [`Memory::trim_working_sets`] with the user's exclusion list.
    fn purge(&self, area: MemoryArea) -> Result<PurgeOutcome, PortError>;

    /// Trims the working set of every process the user can open, except the processes `protected`
    /// names (matched by image name). A protected process is never opened and is counted in
    /// [`PurgeOutcome::processes_excluded`].
    fn trim_working_sets(&self, protected: &ExclusionList) -> Result<PurgeOutcome, PortError>;
}

/// The ports an operation may touch, bundled for the executor.
///
/// Composition roots build this from real adapters; tests build it from fakes.
#[derive(Clone)]
pub struct Ports {
    /// Registry port.
    pub registry: Arc<dyn Registry>,
    /// File system port.
    pub file_system: Arc<dyn FileSystem>,
    /// Services port.
    pub services: Arc<dyn Services>,
    /// Processes port.
    pub processes: Arc<dyn Processes>,
    /// Memory port.
    pub memory: Arc<dyn Memory>,
    /// External commands port.
    pub commands: Arc<dyn Commands>,
}

impl fmt::Debug for Ports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}
