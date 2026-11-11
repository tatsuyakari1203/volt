//! In-memory doubles of the app ports, for tests of the core and of the shell.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::{
    PackagedApp, PackagedApps, StopWaiting, UninstallCommand, Uninstaller, UninstallerExit, Waited,
};
use crate::error::PortError;
use crate::fakes::FakeRegistry;
use crate::registry::{Registry, RegistryKeyRef};
use crate::time::Timestamp;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A [`Uninstaller`] that starts nothing. It records what it was asked to run and can pretend the
/// vendor's uninstaller removed the app (by deleting its registry key), exited without removing
/// it, or could not be started.
#[derive(Debug, Default)]
pub struct FakeUninstaller {
    runs: Mutex<Vec<UninstallCommand>>,
    removes: Mutex<HashMap<String, (Arc<FakeRegistry>, RegistryKeyRef)>>,
    failures: Mutex<HashMap<String, PortError>>,
    exit_codes: Mutex<HashMap<String, i32>>,
    waited: Mutex<HashMap<String, Waited>>,
    until_stopped: Mutex<Vec<String>>,
}

impl FakeUninstaller {
    /// An uninstaller that succeeds without changing anything.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// When `program` is run, deletes `key` from `registry`, as a finished uninstaller does.
    pub fn removes_key_when_run(
        &self,
        program: &str,
        registry: Arc<FakeRegistry>,
        key: RegistryKeyRef,
    ) {
        lock(&self.removes).insert(program.to_lowercase(), (registry, key));
    }

    /// Makes running `program` fail with `error` (nothing ran).
    pub fn fail_on(&self, program: &str, error: PortError) {
        lock(&self.failures).insert(program.to_lowercase(), error);
    }

    /// Makes `program` exit with `code`.
    pub fn exit_with(&self, program: &str, code: i32) {
        lock(&self.exit_codes).insert(program.to_lowercase(), code);
    }

    /// Makes `program` report that the wait reached only as far as `waited`.
    pub fn waited_as(&self, program: &str, waited: Waited) {
        lock(&self.waited).insert(program.to_lowercase(), waited);
    }

    /// Makes `program` keep running until the wait is stopped, as an uninstaller the user gives up
    /// on does.
    pub fn runs_until_stopped(&self, program: &str) {
        lock(&self.until_stopped).push(program.to_lowercase());
    }

    /// Every command that was run, in order.
    #[must_use]
    pub fn runs(&self) -> Vec<UninstallCommand> {
        lock(&self.runs).clone()
    }
}

impl Uninstaller for FakeUninstaller {
    fn run(
        &self,
        command: &UninstallCommand,
        stop: &StopWaiting,
    ) -> Result<UninstallerExit, PortError> {
        let program = command.program().to_lowercase();
        if let Some(error) = lock(&self.failures).get(&program) {
            return Err(error.clone());
        }
        lock(&self.runs).push(command.clone());
        if let Some((registry, key)) = lock(&self.removes).get(&program) {
            let _ = registry.delete_key(key);
        }
        if lock(&self.until_stopped).contains(&program) {
            while !stop.is_requested() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            return Ok(UninstallerExit {
                exit_code: None,
                waited: Waited::Stopped,
            });
        }
        Ok(UninstallerExit {
            exit_code: Some(lock(&self.exit_codes).get(&program).copied().unwrap_or(0)),
            waited: lock(&self.waited)
                .get(&program)
                .copied()
                .unwrap_or(Waited::Everything),
        })
    }
}

/// A [`PackagedApps`] over a list in memory.
#[derive(Debug, Default)]
pub struct FakePackages {
    packages: Mutex<Vec<PackagedApp>>,
    removed: Mutex<Vec<String>>,
    list_failure: Mutex<Option<PortError>>,
    remove_failures: Mutex<HashMap<String, PortError>>,
    keep_after_remove: Mutex<Vec<String>>,
}

impl FakePackages {
    /// No packages.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an ordinary packaged app.
    pub fn add(&self, full_name: &str, name: &str) {
        self.add_package(PackagedApp {
            full_name: full_name.to_owned(),
            name: name.to_owned(),
            publisher: Some("Contoso".to_owned()),
            version: Some("1.2.3.4".to_owned()),
            installed: Some(Timestamp::from_unix_millis(1_700_000_000_000)),
            size_bytes: None,
            framework: false,
            resource: false,
            system: false,
        });
    }

    /// Adds a package exactly as given.
    pub fn add_package(&self, package: PackagedApp) {
        lock(&self.packages).push(package);
    }

    /// Makes every listing fail with `error`.
    pub fn fail_list(&self, error: PortError) {
        *lock(&self.list_failure) = Some(error);
    }

    /// Makes removing `full_name` fail with `error`.
    pub fn fail_remove(&self, full_name: &str, error: PortError) {
        lock(&self.remove_failures).insert(full_name.to_owned(), error);
    }

    /// Removing `full_name` reports success but leaves the package listed.
    pub fn keep_after_remove(&self, full_name: &str) {
        lock(&self.keep_after_remove).push(full_name.to_owned());
    }

    /// The full names that were asked to be removed, in order.
    #[must_use]
    pub fn removed(&self) -> Vec<String> {
        lock(&self.removed).clone()
    }
}

impl PackagedApps for FakePackages {
    fn list(&self) -> Result<Vec<PackagedApp>, PortError> {
        if let Some(error) = lock(&self.list_failure).clone() {
            return Err(error);
        }
        Ok(lock(&self.packages).clone())
    }

    fn remove(&self, full_name: &str) -> Result<(), PortError> {
        if let Some(error) = lock(&self.remove_failures).get(full_name) {
            return Err(error.clone());
        }
        lock(&self.removed).push(full_name.to_owned());
        if !lock(&self.keep_after_remove)
            .iter()
            .any(|kept| kept == full_name)
        {
            lock(&self.packages).retain(|package| package.full_name != full_name);
        }
        Ok(())
    }
}

/// Writes an app entry the way an installer does, into `registry` at `subkey` (the key of the
/// app, below the Uninstall key of its place). For arranging a test.
pub fn add_entry(
    registry: &FakeRegistry,
    key: &RegistryKeyRef,
    values: &[(&str, crate::registry::RegistryValue)],
) {
    registry.add_key(key.hive, &key.subkey, key.view);
    for (name, value) in values {
        registry.set(&key.value(*name), value.clone());
    }
}

/// A [`Winget`](super::Winget) that runs nothing. It answers the listing and each upgrade with
/// what a test set, and records every command it was asked to run.
#[derive(Debug)]
pub struct FakeWinget {
    list: Mutex<Result<super::WingetRun, PortError>>,
    upgrades: Mutex<HashMap<String, Result<super::WingetRun, PortError>>>,
    commands: Mutex<Vec<super::WingetCommand>>,
}

impl Default for FakeWinget {
    fn default() -> Self {
        Self {
            list: Mutex::new(Err(PortError::NotFound("winget".into()))),
            upgrades: Mutex::new(HashMap::new()),
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl FakeWinget {
    /// A winget that is not installed.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The listing prints `output` and exits with `exit_code`.
    pub fn lists(&self, exit_code: i32, output: &str) {
        *lock(&self.list) = Ok(super::WingetRun {
            exit_code,
            output: output.to_owned(),
        });
    }

    /// The listing cannot be run.
    pub fn list_fails(&self, error: PortError) {
        *lock(&self.list) = Err(error);
    }

    /// Upgrading `id` exits with `exit_code`. An id without an answer exits with 0.
    pub fn upgrade_exits(&self, id: &str, exit_code: i32) {
        lock(&self.upgrades).insert(
            id.to_owned(),
            Ok(super::WingetRun {
                exit_code,
                output: String::new(),
            }),
        );
    }

    /// Upgrading `id` cannot be run.
    pub fn upgrade_fails(&self, id: &str, error: PortError) {
        lock(&self.upgrades).insert(id.to_owned(), Err(error));
    }

    /// Every command run so far, in order.
    #[must_use]
    pub fn commands(&self) -> Vec<super::WingetCommand> {
        lock(&self.commands).clone()
    }
}

impl super::Winget for FakeWinget {
    fn run(&self, command: &super::WingetCommand) -> Result<super::WingetRun, PortError> {
        lock(&self.commands).push(command.clone());
        match command {
            super::WingetCommand::List => lock(&self.list).clone(),
            super::WingetCommand::Upgrade { id, .. } => lock(&self.upgrades)
                .get(id.as_str())
                .cloned()
                .unwrap_or_else(|| {
                    Ok(super::WingetRun {
                        exit_code: 0,
                        output: String::new(),
                    })
                }),
        }
    }
}
