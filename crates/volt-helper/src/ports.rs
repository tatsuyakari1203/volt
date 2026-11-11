//! The ports the helper's tweak engine works through.
//!
//! The helper has no shell, no child processes and no file operations of the user's (see the
//! decision on the helper), so only three of the six ports do anything:
//!
//! - the registry is [`AdminRegistry`]: it reads like the real one and writes only `HKLM`, only
//!   after every key on the path has been checked (owner, ACL, no symbolic link);
//! - services go through [`AllowedServices`]: only the services the helper's own catalogue names
//!   in an administrator tweak can be read or changed;
//! - memory is the real memory port (a purge, which is not a tweak).
//!
//! The file system, processes and commands ports answer "unsupported" to everything.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use volt_core::{
    Catalogue, Command, CommandOutput, Commands, DirEntry, EntryInfo, FileStat, FileSystem, Memory,
    PortError, Ports, ProcessChange, ProcessInfo, ProcessSettings, Processes, ServiceStartType,
    Services, TweakStep,
};
use volt_win::WinServices;
use volt_win::helper::registry_guard::AdminRegistry;

fn none(what: &str) -> PortError {
    PortError::Unsupported(format!("the helper has no {what}"))
}

/// A file system port that refuses everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoFileSystem;

impl FileSystem for NoFileSystem {
    fn stat(&self, _path: &Path) -> Result<Option<FileStat>, PortError> {
        Err(none("file system access"))
    }

    fn inspect(&self, _path: &Path) -> Result<Option<EntryInfo>, PortError> {
        Err(none("file system access"))
    }

    fn read_dir(&self, _dir: &Path) -> Result<Vec<DirEntry>, PortError> {
        Err(none("file system access"))
    }

    fn move_to_trash(&self, _root: &Path, _path: &Path) -> Result<(), PortError> {
        Err(none("file system access"))
    }
}

/// A processes port that refuses everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProcesses;

impl Processes for NoProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        Err(none("process access"))
    }

    fn terminate(&self, _pid: u32, _created_at_100ns: u64) -> Result<(), PortError> {
        Err(none("process access"))
    }

    fn settings(&self, _pid: u32, _created_at_100ns: u64) -> Result<ProcessSettings, PortError> {
        Err(none("process access"))
    }

    fn apply_settings(
        &self,
        _pid: u32,
        _created_at_100ns: u64,
        _change: &ProcessChange,
    ) -> Result<(), PortError> {
        Err(none("process access"))
    }
}

/// A commands port that runs nothing: the helper starts no program.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoCommands;

impl Commands for NoCommands {
    fn run(&self, _command: &Command) -> Result<CommandOutput, PortError> {
        Err(none("way to run a program"))
    }
}

/// A services port that serves only an allow list of service names (compared ignoring case).
pub struct AllowedServices {
    inner: Arc<dyn Services>,
    allowed: HashSet<String>,
}

impl std::fmt::Debug for AllowedServices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AllowedServices")
            .field("allowed", &self.allowed.len())
            .finish_non_exhaustive()
    }
}

impl AllowedServices {
    /// Serves `names` through `inner` and refuses every other service.
    pub fn new<I, S>(inner: Arc<dyn Services>, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            inner,
            allowed: names
                .into_iter()
                .map(|name| name.as_ref().to_lowercase())
                .collect(),
        }
    }

    fn check(&self, service: &str) -> Result<(), PortError> {
        if self.allowed.contains(&service.to_lowercase()) {
            Ok(())
        } else {
            Err(PortError::AccessDenied(format!(
                "the helper does not manage the service `{service}`"
            )))
        }
    }
}

impl Services for AllowedServices {
    fn start_type(&self, service: &str) -> Result<ServiceStartType, PortError> {
        self.check(service)?;
        self.inner.start_type(service)
    }

    fn set_start_type(&self, service: &str, start_type: ServiceStartType) -> Result<(), PortError> {
        self.check(service)?;
        self.inner.set_start_type(service, start_type)
    }
}

/// The services the administrator tweaks of `catalogue` change.
#[must_use]
pub fn catalogue_services(catalogue: &Catalogue) -> Vec<String> {
    catalogue
        .tweaks()
        .iter()
        .filter(|tweak| tweak.requires_admin)
        .flat_map(|tweak| &tweak.steps)
        .filter_map(|step| match step {
            TweakStep::Service { service, .. } => Some(service.clone()),
            _ => None,
        })
        .collect()
}

/// The ports of the real helper for `catalogue`.
#[must_use]
pub fn helper_ports(catalogue: &Catalogue, memory: Arc<dyn Memory>) -> Ports {
    let registry: Arc<dyn volt_core::Registry> = Arc::new(AdminRegistry::new());
    // The harness build ends the helper at the write of its crash tweak; no other build has this.
    #[cfg(feature = "elevated-ci")]
    let registry: Arc<dyn volt_core::Registry> =
        Arc::new(crate::failpoint::FailpointRegistry::new(registry));
    Ports {
        registry,
        file_system: Arc::new(NoFileSystem),
        services: Arc::new(AllowedServices::new(
            Arc::new(WinServices::new()),
            catalogue_services(catalogue),
        )),
        processes: Arc::new(NoProcesses),
        memory,
        commands: Arc::new(NoCommands),
    }
}

#[cfg(test)]
mod tests {
    use volt_core::fakes::FakeServices;

    use super::*;

    #[test]
    fn only_the_services_of_administrator_tweaks_are_served() {
        let catalogue = Catalogue::from_toml(
            r#"
[[tweak]]
id = "stop-search"
title = "Stop search"
description = "d"
category = "performance"
risk = "caution"
evidence = "measured"
evidence_note = "n"
requires_admin = true

[[tweak.steps]]
kind = "service"
service = "WSearch"
start_type = "disabled"

[[tweak]]
id = "user-service"
title = "Not administrator"
description = "d"
category = "performance"
risk = "caution"
evidence = "measured"
evidence_note = "n"

[[tweak.steps]]
kind = "service"
service = "Other"
start_type = "disabled"
"#,
        )
        .expect("catalogue");
        let names = catalogue_services(&catalogue);
        assert_eq!(names, ["WSearch"]);

        let inner = Arc::new(FakeServices::new());
        inner.add("WSearch", ServiceStartType::Automatic);
        inner.add("Spooler", ServiceStartType::Automatic);
        let services = AllowedServices::new(inner.clone(), names);
        assert_eq!(
            services.start_type("WSearch"),
            Ok(ServiceStartType::Automatic)
        );
        assert_eq!(
            services.set_start_type("WSearch", ServiceStartType::Disabled),
            Ok(())
        );
        // The allow list ignores case, as Windows does (this fake service store does not, so the
        // answer is "not found", not "denied").
        assert!(matches!(
            services.start_type("wsearch"),
            Err(PortError::NotFound(_))
        ));
        for name in ["Spooler", "Other", "", "WSearch2"] {
            assert!(
                matches!(services.start_type(name), Err(PortError::AccessDenied(_))),
                "{name:?}"
            );
            assert!(
                matches!(
                    services.set_start_type(name, ServiceStartType::Disabled),
                    Err(PortError::AccessDenied(_))
                ),
                "{name:?}"
            );
        }
        assert_eq!(inner.start_type("Spooler"), Ok(ServiceStartType::Automatic));
    }

    #[test]
    fn the_helper_runs_no_program_and_touches_no_file_or_process() {
        assert!(
            NoCommands
                .run(&Command::new("cmd", ["/c", "echo"]))
                .is_err()
        );
        assert!(NoFileSystem.stat(Path::new("C:\\")).is_err());
        assert!(NoFileSystem.read_dir(Path::new("C:\\")).is_err());
        assert!(NoFileSystem.inspect(Path::new("C:\\")).is_err());
        assert!(
            NoFileSystem
                .move_to_trash(Path::new("C:\\"), Path::new("C:\\x"))
                .is_err()
        );
        assert!(NoProcesses.list().is_err());
        assert!(NoProcesses.terminate(4, 0).is_err());
    }
}
