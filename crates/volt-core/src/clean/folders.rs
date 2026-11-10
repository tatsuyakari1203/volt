//! Named locations a cleaning rule may start from, and the port that resolves
//! them.
//!
//! A rule never carries an absolute path. It names one of a small, closed set
//! of [`Location`]s and a relative path below it, and the [`KnownFolders`] port
//! turns the location into a real folder. The core never reads the environment
//! itself, so tests resolve locations to fake folders and the composition root
//! resolves them with the Windows shell.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::PortError;

/// A named place on disk. The set is closed on purpose: adding a location is a
/// code change that gets reviewed, not something a rule file can invent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Location {
    /// The user's temporary directory (`%TEMP%`). The only location whose root
    /// may be the folder itself.
    UserTemp,
    /// The user's local application data (`%LOCALAPPDATA%`).
    LocalAppData,
    /// The user's roaming application data (`%APPDATA%`).
    RoamingAppData,
    /// Machine-wide application data (`%ProgramData%`).
    ProgramData,
    /// The Windows directory (`%SystemRoot%`).
    Windows,
}

impl Location {
    /// The name used in rule files.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::UserTemp => "user_temp",
            Self::LocalAppData => "local_app_data",
            Self::RoamingAppData => "roaming_app_data",
            Self::ProgramData => "program_data",
            Self::Windows => "windows",
        }
    }

    /// Whether a rule may use the location folder itself as its root, with no
    /// relative path. Only the temporary directory qualifies: it holds nothing
    /// but disposable files, unlike the other locations.
    #[must_use]
    pub const fn allows_empty_path(self) -> bool {
        matches!(self, Self::UserTemp)
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Port: where the named locations are on this machine.
pub trait KnownFolders: Send + Sync {
    /// The absolute folder of a location. Failure is a [`PortError`]; a rule
    /// that needs the location is then not scanned.
    fn location(&self, location: Location) -> Result<PathBuf, PortError>;

    /// Absolute folders that no cleaning root may be, or contain: the user
    /// profile, the Windows directory, Program Files, the user's documents and
    /// similar. A rule whose resolved root equals one of them, or is a folder
    /// above one of them, is refused. The comparison ignores case.
    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError>;
}
