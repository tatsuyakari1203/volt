//! Where startup entries live, as data the composition root can change.
//!
//! The registry keys and folders the lister reads and the toggle writes are not
//! hard-coded in the manager: [`StartupSources`] carries them. The app uses
//! [`StartupSources::windows`]; tests use [`StartupSources::sandboxed_in_hkcu`]
//! to point every one of them at a throwaway key and temp folders.

use std::fmt;
use std::path::PathBuf;

use crate::clean::{KnownFolders, Location};
use crate::error::PortError;
use crate::registry::{RegistryHive, RegistryKeyRef, RegistryView};

/// The Run key, below a hive.
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Windows keeps the approval records, below a hive.
const APPROVED_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";
/// The per-user Startup folder, below the roaming application data folder.
const USER_STARTUP_FOLDER: [&str; 5] =
    ["Microsoft", "Windows", "Start Menu", "Programs", "Startup"];
/// The common Startup folder, below `ProgramData`.
const COMMON_STARTUP_FOLDER: [&str; 5] =
    ["Microsoft", "Windows", "Start Menu", "Programs", "StartUp"];

/// Whether a source belongs to the current user or to every user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StartupScope {
    /// Applies to the signed-in user. Changing it needs no special rights.
    User,
    /// Applies to every user. Changing it needs administrator rights.
    Machine,
}

/// Which place a startup entry comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StartupOrigin {
    /// `HKCU\...\Run`.
    UserRun,
    /// `HKLM\...\Run`, the 64-bit view.
    MachineRun,
    /// `HKLM\...\Run`, the 32-bit view (`WOW6432Node`).
    MachineRun32,
    /// The current user's Startup folder.
    UserFolder,
    /// The Startup folder shared by all users.
    CommonFolder,
}

impl StartupOrigin {
    /// Every origin, in the order the sources are read.
    pub const ALL: [Self; 5] = [
        Self::UserRun,
        Self::MachineRun,
        Self::MachineRun32,
        Self::UserFolder,
        Self::CommonFolder,
    ];

    /// A stable id, used inside entry ids and by the UI.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::UserRun => "user-run",
            Self::MachineRun => "machine-run",
            Self::MachineRun32 => "machine-run-32",
            Self::UserFolder => "user-folder",
            Self::CommonFolder => "common-folder",
        }
    }

    /// The origin with this [`id`](Self::id).
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|origin| origin.id() == id)
    }

    /// Whose entries these are.
    #[must_use]
    pub const fn scope(self) -> StartupScope {
        match self {
            Self::UserRun | Self::UserFolder => StartupScope::User,
            Self::MachineRun | Self::MachineRun32 | Self::CommonFolder => StartupScope::Machine,
        }
    }

    /// Whether entries of this origin are files in a Startup folder (rather
    /// than values in a Run key).
    #[must_use]
    pub const fn is_folder(self) -> bool {
        matches!(self, Self::UserFolder | Self::CommonFolder)
    }

    /// A short plain-words name for a badge.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::UserRun => "Run (you)",
            Self::MachineRun => "Run (all users)",
            Self::MachineRun32 => "Run, 32-bit (all users)",
            Self::UserFolder => "Startup folder (you)",
            Self::CommonFolder => "Startup folder (all users)",
        }
    }
}

impl fmt::Display for StartupOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// A Run key and the approval key that holds the state of its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSource {
    /// Where the entries come from.
    pub origin: StartupOrigin,
    /// The Run key.
    pub key: RegistryKeyRef,
    /// The `StartupApproved` key whose values share the entries' names.
    pub approval: RegistryKeyRef,
}

/// A Startup folder and the approval key that holds the state of its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderSource {
    /// Where the entries come from.
    pub origin: StartupOrigin,
    /// The folder, or why it could not be resolved.
    pub path: Result<PathBuf, PortError>,
    /// The `StartupApproved` key whose values are the files' names.
    pub approval: RegistryKeyRef,
}

/// Every place startup entries are read from and approved in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupSources {
    /// Run keys.
    pub runs: Vec<RunSource>,
    /// Startup folders.
    pub folders: Vec<FolderSource>,
}

impl StartupSources {
    /// The real sources of Windows: the per-user Run key, the machine-wide Run
    /// key in the 64-bit and the 32-bit view and the two Startup folders, with
    /// the approval keys Task Manager uses (`Run`, `Run32`, `StartupFolder`).
    ///
    /// `HKCU\...\Run` is shared between the 64-bit and the 32-bit view (there
    /// is no `WOW6432Node` copy of it), so it is listed once.
    #[must_use]
    pub fn windows(folders: &dyn KnownFolders) -> Self {
        let approval = |hive, kind: &str| {
            RegistryKeyRef::new(hive, format!(r"{APPROVED_KEY}\{kind}"), RegistryView::Bit64)
        };
        let startup_folder = |location, parts: &[&str]| {
            folders.location(location).map(|mut path| {
                path.extend(parts);
                path
            })
        };
        Self {
            runs: vec![
                RunSource {
                    origin: StartupOrigin::UserRun,
                    key: RegistryKeyRef::new(
                        RegistryHive::CurrentUser,
                        RUN_KEY,
                        RegistryView::Bit64,
                    ),
                    approval: approval(RegistryHive::CurrentUser, "Run"),
                },
                RunSource {
                    origin: StartupOrigin::MachineRun,
                    key: RegistryKeyRef::new(
                        RegistryHive::LocalMachine,
                        RUN_KEY,
                        RegistryView::Bit64,
                    ),
                    approval: approval(RegistryHive::LocalMachine, "Run"),
                },
                RunSource {
                    origin: StartupOrigin::MachineRun32,
                    key: RegistryKeyRef::new(
                        RegistryHive::LocalMachine,
                        RUN_KEY,
                        RegistryView::Bit32,
                    ),
                    approval: approval(RegistryHive::LocalMachine, "Run32"),
                },
            ],
            folders: vec![
                FolderSource {
                    origin: StartupOrigin::UserFolder,
                    path: startup_folder(Location::RoamingAppData, &USER_STARTUP_FOLDER),
                    approval: approval(RegistryHive::CurrentUser, "StartupFolder"),
                },
                FolderSource {
                    origin: StartupOrigin::CommonFolder,
                    path: startup_folder(Location::ProgramData, &COMMON_STARTUP_FOLDER),
                    approval: approval(RegistryHive::LocalMachine, "StartupFolder"),
                },
            ],
        }
    }

    /// Sources that sit entirely below one throwaway key in `HKCU` and in two
    /// given folders, with the same layout as the real ones
    /// (`<root>\Run`, `<root>\RunMachine`, `<root>\RunMachine32`, approvals under
    /// `<root>\Explorer\StartupApproved\{Run,Run32,StartupFolder}` for the user
    /// and `<root>\Explorer\StartupApprovedMachine\...` for the machine).
    ///
    /// For tests of the adapters and the composition root: it lets them list
    /// and change entries without touching the real Run or `StartupApproved`
    /// keys. The machine-wide origins keep their scope, so changing them is
    /// still refused.
    #[must_use]
    pub fn sandboxed_in_hkcu(root: &str, user_folder: PathBuf, common_folder: PathBuf) -> Self {
        let key = |subkey: String| {
            RegistryKeyRef::new(RegistryHive::CurrentUser, subkey, RegistryView::Bit64)
        };
        let user_approval = |kind: &str| key(format!(r"{root}\Explorer\StartupApproved\{kind}"));
        let machine_approval =
            |kind: &str| key(format!(r"{root}\Explorer\StartupApprovedMachine\{kind}"));
        Self {
            runs: vec![
                RunSource {
                    origin: StartupOrigin::UserRun,
                    key: key(format!(r"{root}\Run")),
                    approval: user_approval("Run"),
                },
                RunSource {
                    origin: StartupOrigin::MachineRun,
                    key: key(format!(r"{root}\RunMachine")),
                    approval: machine_approval("Run"),
                },
                RunSource {
                    origin: StartupOrigin::MachineRun32,
                    key: key(format!(r"{root}\RunMachine32")),
                    approval: machine_approval("Run32"),
                },
            ],
            folders: vec![
                FolderSource {
                    origin: StartupOrigin::UserFolder,
                    path: Ok(user_folder),
                    approval: user_approval("StartupFolder"),
                },
                FolderSource {
                    origin: StartupOrigin::CommonFolder,
                    path: Ok(common_folder),
                    approval: machine_approval("StartupFolder"),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clean::fakes::FakeKnownFolders;

    #[test]
    fn origin_ids_round_trip_and_are_unique() {
        for origin in StartupOrigin::ALL {
            assert_eq!(StartupOrigin::from_id(origin.id()), Some(origin));
        }
        assert_eq!(StartupOrigin::from_id("nope"), None);
        let mut ids: Vec<_> = StartupOrigin::ALL.iter().map(|o| o.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), StartupOrigin::ALL.len());
    }

    #[test]
    fn only_the_current_user_origins_are_user_scope() {
        let user: Vec<_> = StartupOrigin::ALL
            .into_iter()
            .filter(|o| o.scope() == StartupScope::User)
            .collect();
        assert_eq!(user, [StartupOrigin::UserRun, StartupOrigin::UserFolder]);
    }

    #[test]
    fn the_windows_sources_name_the_keys_task_manager_uses() {
        let folders = FakeKnownFolders::new();
        folders.set(Location::RoamingAppData, r"C:\Users\Sample\AppData\Roaming");
        folders.set(Location::ProgramData, r"C:\ProgramData");
        let sources = StartupSources::windows(&folders);

        let run = |origin| {
            sources
                .runs
                .iter()
                .find(|s| s.origin == origin)
                .expect("source")
        };
        assert_eq!(
            run(StartupOrigin::UserRun).key.hive,
            RegistryHive::CurrentUser
        );
        assert_eq!(
            run(StartupOrigin::MachineRun32).key.view,
            RegistryView::Bit32
        );
        assert_eq!(
            run(StartupOrigin::MachineRun).approval.subkey,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run"
        );
        assert!(
            run(StartupOrigin::MachineRun32)
                .approval
                .subkey
                .ends_with(r"StartupApproved\Run32")
        );
        let folder = |origin| {
            sources
                .folders
                .iter()
                .find(|s| s.origin == origin)
                .expect("source")
        };
        assert_eq!(
            folder(StartupOrigin::UserFolder).path,
            Ok(PathBuf::from(
                r"C:\Users\Sample\AppData\Roaming\Microsoft\Windows\Start Menu\Programs\Startup"
            ))
        );
        assert_eq!(
            folder(StartupOrigin::CommonFolder).path,
            Ok(PathBuf::from(
                r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\StartUp"
            ))
        );
        assert!(
            folder(StartupOrigin::CommonFolder)
                .approval
                .subkey
                .ends_with(r"StartupApproved\StartupFolder")
        );
        assert_eq!(
            folder(StartupOrigin::CommonFolder).approval.hive,
            RegistryHive::LocalMachine
        );
    }

    #[test]
    fn an_unresolvable_folder_is_kept_as_an_error_not_dropped() {
        let sources = StartupSources::windows(&FakeKnownFolders::new());
        assert_eq!(sources.folders.len(), 2);
        assert!(sources.folders.iter().all(|f| f.path.is_err()));
    }

    #[test]
    fn the_sandbox_keeps_every_key_below_its_root() {
        let sources = StartupSources::sandboxed_in_hkcu(
            r"Software\VoltTest\x",
            PathBuf::from("a"),
            PathBuf::from("b"),
        );
        let keys = sources
            .runs
            .iter()
            .flat_map(|s| [&s.key, &s.approval])
            .chain(sources.folders.iter().map(|f| &f.approval));
        for key in keys {
            assert_eq!(key.hive, RegistryHive::CurrentUser);
            assert!(key.subkey.starts_with(r"Software\VoltTest\x\"), "{key}");
        }
        assert_eq!(sources.runs.len(), 3);
        assert_eq!(sources.folders.len(), 2);
    }
}
