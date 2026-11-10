//! Port traits for the outside world (except the registry and the journal,
//! which have their own modules) and the [`Ports`] bundle.
//!
//! All ports are synchronous, object-safe and `Send + Sync`. File system,
//! process and memory ports are deliberately minimal; later tasks extend them.

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::registry::Registry;
use crate::service::ServiceStartType;

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

/// Port: file system. File deletion is not an undoable [`Operation`](crate::Operation);
/// cleaning shows a preview and prefers the Recycle Bin.
pub trait FileSystem: Send + Sync {
    /// Metadata for a path. A missing path is `Ok(None)`.
    fn stat(&self, path: &Path) -> Result<Option<FileStat>, PortError>;

    /// Moves `path` (a file or directory) to the Recycle Bin, provided it lies
    /// inside `root`.
    ///
    /// Implementations must refuse with [`PortError::InvalidInput`] a `path`
    /// whose resolved location is outside `root`, and one that reaches its
    /// target through a junction or symlink, so a cleaner can never be tricked
    /// into deleting outside the folder it was asked to clean. A missing path
    /// is [`PortError::NotFound`].
    fn move_to_trash(&self, root: &Path, path: &Path) -> Result<(), PortError>;
}

/// A running process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Process id.
    pub pid: u32,
    /// Executable name.
    pub name: String,
    /// Working set in bytes.
    pub working_set_bytes: u64,
}

/// Port: processes.
pub trait Processes: Send + Sync {
    /// Lists running processes.
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError>;

    /// Ends a process. Unknown pid is [`PortError::NotFound`].
    fn terminate(&self, pid: u32) -> Result<(), PortError>;
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
    /// Trim the working sets of processes.
    WorkingSets,
    /// Flush the standby list.
    StandbyList,
    /// Flush the modified page list.
    ModifiedList,
}

/// Port: memory. Purging is not an undoable operation: it only releases
/// reclaimable memory.
pub trait Memory: Send + Sync {
    /// Current physical memory totals.
    fn status(&self) -> Result<MemoryStatus, PortError>;

    /// Purges a memory area.
    fn purge(&self, area: MemoryArea) -> Result<(), PortError>;
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
}

impl fmt::Debug for Ports {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ports").finish_non_exhaustive()
    }
}
