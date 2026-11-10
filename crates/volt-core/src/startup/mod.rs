//! The startup apps manager: what starts with Windows, and switching entries
//! on and off the way Windows does.
//!
//! # Reading
//!
//! [`StartupManager::list`] is read-only. It reads the Run keys (per user, and
//! machine-wide in the 64-bit and the 32-bit view) and the per-user and common
//! Startup folders. Each [`StartupEntry`] has a name, the command as written,
//! where it comes from, whether it is enabled and, when it can be read, the
//! publisher of the program (an optional [`FilePublisher`] port: a missing or
//! unreadable publisher is `None`, never an error). A source that cannot be
//! read is reported as a [`SourceProblem`]; it does not fail the list.
//!
//! # Switching
//!
//! Windows switches an entry off by writing a small binary value under
//! `StartupApproved` (see [`approval`]), not by deleting it. Volt does the same,
//! so Task Manager and Settings show the same state. Every change is one
//! [`Operation`](crate::Operation) (a registry value write) applied through the
//! [`Executor`], so it is journaled with the previous value (including "there
//! was none"), labelled [`STARTUP_LABEL`], undoable, and recorded in the
//! activity log with a summary in plain words (`Disabled startup entry: Name`).
//!
//! # Shortcuts and scheduled tasks
//!
//! A Startup folder shortcut (`.lnk`) is resolved through the optional
//! [`ShortcutResolver`] port, so the entry shows the command it really starts
//! (target and arguments) and the publisher of the target. Scheduled tasks with
//! a logon trigger come from the optional [`LogonTasks`] port and are listed in
//! [`StartupListing::tasks`], read-only: they are not entries, have no state
//! the manager can change and never count towards the switchable entries.
//!
//! Entries that apply to every user (the machine-wide Run keys and the common
//! Startup folder) can be listed but not changed: that needs administrator
//! rights and the elevation model is not decided, so
//! [`StartupError::NeedsAdministrator`] is returned and nothing is touched.

pub mod approval;
mod command;
mod sources;

#[cfg(any(test, feature = "fakes"))]
pub mod fakes;

use std::fmt;
use std::path::Path;
use std::sync::Arc;

use thiserror::Error;

pub use approval::{StartupState, approved_entry_name, decode_approval, encode_approval};
pub use command::{CommandLine, parse_command_line};
pub use sources::{FolderSource, RunSource, StartupOrigin, StartupScope, StartupSources};

use crate::activity::error_class;
use crate::error::{ExecError, PlanError, PortError};
use crate::executor::{Executor, ItemStatus};
use crate::journal::{BatchId, EntryId, Source};
use crate::label::{Evidence, Risk};
use crate::operation::Operation;
use crate::plan::{OperationRequest, Plan};
use crate::ports::{EntryKind, FileSystem};
use crate::registry::{Registry, RegistryKeyRef, RegistryValue};
use crate::time::Clock;

/// The label of the journal batches the startup manager writes.
pub const STARTUP_LABEL: &str = "startup";

/// Port: who published a program.
///
/// Read from the file's version information. Optional by type: the caller
/// never needs an answer.
pub trait FilePublisher: Send + Sync {
    /// The publisher (the company name in the file's version information) of
    /// the program at `executable`, or `Ok(None)` when the file has none or
    /// does not exist.
    ///
    /// `executable` is a path as written in a startup command, so it may hold
    /// unexpanded environment variable references such as `%ProgramFiles%`;
    /// the adapter expands them. Read-only.
    fn publisher(&self, executable: &str) -> Result<Option<String>, PortError>;
}

/// A [`FilePublisher`] that knows nothing: every publisher is unknown.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPublisher;

impl FilePublisher for NoPublisher {
    fn publisher(&self, _executable: &str) -> Result<Option<String>, PortError> {
        Ok(None)
    }
}

/// What a shortcut points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutTarget {
    /// The program the shortcut starts, as stored (environment variable
    /// references are not expanded).
    pub target: String,
    /// The arguments, possibly empty.
    pub arguments: String,
}

/// Port: reads where a `.lnk` shortcut points. Read-only; the shortcut is never
/// followed or run.
pub trait ShortcutResolver: Send + Sync {
    /// The target of the shortcut file at `shortcut`, or `Ok(None)` when it has
    /// no file target (for example a shortcut to a folder or a special item).
    fn resolve(&self, shortcut: &Path) -> Result<Option<ShortcutTarget>, PortError>;
}

/// A scheduled task that runs when a user signs in, as a port reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogonTask {
    /// The full path of the task in the Task Scheduler library, like
    /// `\Microsoft\Windows\Foo\Bar`.
    pub path: String,
    /// The program of the first program action, as written, or empty.
    pub program: String,
    /// The arguments of that action.
    pub arguments: String,
    /// Whether the task and one of its logon triggers are enabled.
    pub enabled: bool,
}

/// Port: lists scheduled tasks that have a logon trigger. Read-only; tasks the
/// user cannot read are left out, not reported as errors.
pub trait LogonTasks: Send + Sync {
    /// The logon tasks the current user can read.
    fn logon_tasks(&self) -> Result<Vec<LogonTask>, PortError>;
}

/// A logon scheduled task, read-only: it has no switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledTaskEntry {
    /// The task's name (the last part of its path).
    pub name: String,
    /// The folder of the task in the library, like `\Microsoft\Windows`.
    pub folder: String,
    /// The command (program and arguments); empty when the task runs no program.
    pub command: String,
    /// The program, when there is one.
    pub program: Option<String>,
    /// Who published the program, when its version information says.
    pub publisher: Option<String>,
    /// Whether the task will run at logon.
    pub enabled: bool,
    /// Whether the task is part of Windows (it lives under `\Microsoft\`).
    pub microsoft: bool,
}

/// One thing that starts with Windows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupEntry {
    /// Stable id (`<origin id>/<name>`); pass it to [`StartupManager::set_enabled`].
    pub id: String,
    /// The name shown to the user: the Run value name, or a Startup folder
    /// file's name without a `.lnk` extension.
    pub name: String,
    /// Where it comes from.
    pub origin: StartupOrigin,
    /// The command as written (a Startup folder item shows its file path).
    /// Environment variable references are not expanded.
    pub command: String,
    /// The program of the command, as written, when it could be told.
    pub program: Option<String>,
    /// Who published the program, when its version information says.
    pub publisher: Option<String>,
    /// Whether it starts with Windows.
    pub state: StartupState,
}

/// A source that could not be read. The rest of the list is still valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceProblem {
    /// The source.
    pub origin: StartupOrigin,
    /// The class of the failure ("access denied", "not found", ...), never the
    /// port's message.
    pub reason: &'static str,
}

/// The result of [`StartupManager::list`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartupListing {
    /// Entries sorted by name (ignoring case), then by source.
    pub entries: Vec<StartupEntry>,
    /// Sources that could not be read.
    pub problems: Vec<SourceProblem>,
    /// Logon scheduled tasks, read-only, sorted by name. Empty when there is no
    /// task source.
    pub tasks: Vec<ScheduledTaskEntry>,
    /// Why the scheduled tasks could not be read (a failure class), if so.
    /// Kept apart from `problems` because tasks are not switchable entries.
    pub tasks_problem: Option<&'static str>,
}

/// What [`StartupManager::set_enabled`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChangeOutcome {
    /// The state was changed and journaled.
    Changed {
        /// The journal batch.
        batch: BatchId,
        /// The journal entry holding the previous value.
        entry: EntryId,
    },
    /// The entry was already in the requested state: nothing was changed or
    /// journaled.
    AlreadyInState,
}

/// Precise result of [`StartupManager::set_enabled`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupChange {
    /// The entry id.
    pub id: String,
    /// The entry's name.
    pub name: String,
    /// Where the entry comes from.
    pub origin: StartupOrigin,
    /// What was done.
    pub outcome: ChangeOutcome,
    /// The state read again afterwards; `None` when it could not be read.
    pub state: Option<StartupState>,
}

/// Why an entry could not be listed or changed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum StartupError {
    /// The id is not one this manager hands out.
    #[error("`{0}` is not a startup entry id")]
    InvalidId(String),
    /// The entry no longer exists (it was removed, or the id is stale).
    #[error("the startup entry `{0}` was not found")]
    NotFound(String),
    /// The entry applies to every user and changing it needs administrator
    /// rights. Nothing was read for the change or changed.
    #[error("changing the startup entry `{name}` needs administrator rights")]
    NeedsAdministrator {
        /// The entry's name.
        name: String,
    },
    /// The current state could not be read. Nothing was changed.
    #[error("cannot read the startup entry: {0}")]
    Read(PortError),
    /// The change failed. [`ExecError::system_changed`] says whether the system
    /// may have been changed.
    #[error("the startup entry could not be changed: {0}")]
    Change(ExecError),
}

/// Lists startup entries and switches them on and off through the executor.
///
/// It owns its ports (the registry and file system views it reads, the
/// publisher port) and a handle on the [`Executor`] that applies the changes.
pub struct StartupManager {
    registry: Arc<dyn Registry>,
    file_system: Arc<dyn FileSystem>,
    publisher: Arc<dyn FilePublisher>,
    executor: Executor,
    clock: Arc<dyn Clock>,
    sources: StartupSources,
    shortcuts: Option<Arc<dyn ShortcutResolver>>,
    tasks: Option<Arc<dyn LogonTasks>>,
}

impl fmt::Debug for StartupManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StartupManager")
            .field("runs", &self.sources.runs.len())
            .field("folders", &self.sources.folders.len())
            .finish_non_exhaustive()
    }
}

/// An entry found again, with what is needed to read and change its state.
struct Located {
    name: String,
    origin: StartupOrigin,
    approval: RegistryKeyRef,
}

impl StartupManager {
    /// Creates a manager over its ports, the executor that applies changes,
    /// the clock (for the time a disabled record carries) and the sources.
    #[must_use]
    pub fn new(
        registry: Arc<dyn Registry>,
        file_system: Arc<dyn FileSystem>,
        publisher: Arc<dyn FilePublisher>,
        executor: Executor,
        clock: Arc<dyn Clock>,
        sources: StartupSources,
    ) -> Self {
        Self {
            registry,
            file_system,
            publisher,
            executor,
            clock,
            sources,
            shortcuts: None,
            tasks: None,
        }
    }

    /// Resolves Startup folder shortcuts through `resolver`, so they show the
    /// command they start and its publisher.
    #[must_use]
    pub fn with_shortcut_resolver(mut self, resolver: Arc<dyn ShortcutResolver>) -> Self {
        self.shortcuts = Some(resolver);
        self
    }

    /// Lists logon scheduled tasks (read-only) from `tasks`.
    #[must_use]
    pub fn with_logon_tasks(mut self, tasks: Arc<dyn LogonTasks>) -> Self {
        self.tasks = Some(tasks);
        self
    }

    /// Lists everything that starts with Windows. Read-only.
    #[must_use]
    pub fn list(&self) -> StartupListing {
        let mut listing = StartupListing::default();
        for source in &self.sources.runs {
            match self.read_run_source(source) {
                Ok(entries) => listing.entries.extend(entries),
                Err(error) => listing.problems.push(problem(source.origin, &error)),
            }
        }
        for source in &self.sources.folders {
            match self.read_folder_source(source) {
                Ok(entries) => listing.entries.extend(entries),
                Err(error) => listing.problems.push(problem(source.origin, &error)),
            }
        }
        listing
            .entries
            .sort_by_cached_key(|entry| (entry.name.to_lowercase(), entry.origin.id()));
        if let Some(tasks) = &self.tasks {
            match tasks.logon_tasks() {
                Ok(found) => {
                    listing.tasks = found.iter().map(|task| self.task_entry(task)).collect();
                    listing
                        .tasks
                        .sort_by_cached_key(|task| (task.name.to_lowercase(), task.folder.clone()));
                }
                Err(error) => listing.tasks_problem = Some(error_class(&error)),
            }
        }
        listing
    }

    fn task_entry(&self, task: &LogonTask) -> ScheduledTaskEntry {
        let (folder, name) = task
            .path
            .rsplit_once('\\')
            .unwrap_or(("", task.path.as_str()));
        let program = Some(task.program.trim().to_owned()).filter(|text| !text.is_empty());
        let command = program
            .as_deref()
            .map(|program| join_command(program, &task.arguments))
            .unwrap_or_default();
        let publisher = program
            .as_deref()
            .and_then(|program| self.publisher_of(program));
        ScheduledTaskEntry {
            name: name.to_owned(),
            folder: if folder.is_empty() { "\\" } else { folder }.to_owned(),
            command,
            program,
            publisher,
            enabled: task.enabled,
            microsoft: task.path.to_lowercase().starts_with("\\microsoft\\"),
        }
    }

    /// The dry run of [`StartupManager::set_enabled`]: the plan it would
    /// apply, or `None` when the entry is already in the requested state.
    /// Reads only. Fails the same ways `set_enabled` fails before it changes
    /// anything, including [`StartupError::NeedsAdministrator`].
    pub fn plan(&self, id: &str, enabled: bool) -> Result<Option<Plan>, StartupError> {
        self.prepare(id, enabled).map(|prepared| prepared.plan)
    }

    /// Switches an entry on or off, as Windows does, through the executor: the
    /// previous value is journaled first (a batch labelled [`STARTUP_LABEL`]),
    /// the change is recorded in the activity log and can be undone from the
    /// journal.
    ///
    /// An entry already in the requested state is left alone and reported as
    /// [`ChangeOutcome::AlreadyInState`]. Machine-wide entries are refused with
    /// [`StartupError::NeedsAdministrator`] before anything is read or written.
    pub fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
        source: Source,
    ) -> Result<StartupChange, StartupError> {
        let prepared = self.prepare(id, enabled)?;
        let located = prepared.located;
        let outcome = match prepared.plan {
            None => ChangeOutcome::AlreadyInState,
            Some(plan) => {
                let report = self
                    .executor
                    .apply_or_rollback(&plan, source)
                    .map_err(StartupError::Change)?;
                if let Some(error) = report.outcomes.iter().find_map(|item| match &item.status {
                    ItemStatus::Failed { error, .. } => Some(error.clone()),
                    _ => None,
                }) {
                    return Err(StartupError::Change(error));
                }
                let entry = report.applied().find_map(|item| match item.status {
                    ItemStatus::Applied { entry } => Some(entry),
                    _ => None,
                });
                match entry {
                    Some(entry) => ChangeOutcome::Changed {
                        batch: report.batch,
                        entry,
                    },
                    // A plan of one item that neither applied nor failed cannot
                    // happen; say so instead of claiming a change.
                    None => {
                        return Err(StartupError::Read(PortError::Other(
                            "the change was neither applied nor reported as failed".to_owned(),
                        )));
                    }
                }
            }
        };
        let state = match self.state_of(&located.approval, &located.name) {
            Ok(state) => Some(state),
            Err(error) => {
                log::warn!(
                    "cannot read a startup entry's state after changing it: {}",
                    error_class(&error)
                );
                None
            }
        };
        Ok(StartupChange {
            id: entry_id(located.origin, &located.name),
            name: located.name,
            origin: located.origin,
            outcome,
            state,
        })
    }

    // ---- reading -----------------------------------------------------------------------------

    fn read_run_source(&self, source: &RunSource) -> Result<Vec<StartupEntry>, PortError> {
        let mut entries = Vec::new();
        for value in self.registry.list_values(&source.key)? {
            // Windows runs string values only; anything else is not an entry.
            let Ok(RegistryValue::Sz(command) | RegistryValue::ExpandSz(command)) = value.value
            else {
                continue;
            };
            let state = self.state_of(&source.approval, &value.name)?;
            let program = parse_command_line(&command).map(|line| line.program);
            let publisher = program
                .as_deref()
                .and_then(|program| self.publisher_of(program));
            entries.push(StartupEntry {
                id: entry_id(source.origin, &value.name),
                name: value.name,
                origin: source.origin,
                command,
                program,
                publisher,
                state,
            });
        }
        Ok(entries)
    }

    fn read_folder_source(&self, source: &FolderSource) -> Result<Vec<StartupEntry>, PortError> {
        let folder = source.path.clone()?;
        let listing = match self.file_system.read_dir(&folder) {
            Ok(listing) => listing,
            // A Startup folder that does not exist has no entries.
            Err(PortError::NotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut entries = Vec::new();
        for item in listing {
            let name = item.name.to_string_lossy().into_owned();
            // Only plain files start. `desktop.ini` is folder decoration.
            let is_file = matches!(&item.info, Ok(info) if info.kind == EntryKind::File);
            if !is_file || name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let path = folder.join(&item.name);
            let mut command = path.to_string_lossy().into_owned();
            let mut program = has_extension(&name, "exe").then(|| command.clone());
            if has_extension(&name, "lnk")
                && let Some(target) = self.shortcut_target(&path)
            {
                command = join_command(&target.target, &target.arguments);
                program = Some(target.target);
            }
            let publisher = program
                .as_deref()
                .and_then(|program| self.publisher_of(program));
            let state = self.state_of(&source.approval, &name)?;
            entries.push(StartupEntry {
                id: entry_id(source.origin, &name),
                name: display_name(&name),
                origin: source.origin,
                command,
                program,
                publisher,
                state,
            });
        }
        Ok(entries)
    }

    /// Where a shortcut points. A shortcut that cannot be read keeps showing
    /// its file path, so a failure is not an error here.
    fn shortcut_target(&self, shortcut: &Path) -> Option<ShortcutTarget> {
        let resolver = self.shortcuts.as_ref()?;
        match resolver.resolve(shortcut) {
            Ok(target) => target.filter(|target| !target.target.trim().is_empty()),
            Err(error) => {
                log::debug!("cannot read a shortcut: {}", error_class(&error));
                None
            }
        }
    }

    /// The publisher of a program, or `None` when it cannot be told. A failure
    /// is not an error here: the publisher is decoration.
    fn publisher_of(&self, program: &str) -> Option<String> {
        match self.publisher.publisher(program) {
            Ok(publisher) => publisher.filter(|text| !text.trim().is_empty()),
            Err(error) => {
                log::debug!(
                    "no publisher for a startup program: {}",
                    error_class(&error)
                );
                None
            }
        }
    }

    /// The state recorded for `name` under an approval key. No record means
    /// enabled, which is what Windows does; a record this layout does not
    /// describe is shown as enabled too (see [`approval`]).
    fn state_of(&self, approval: &RegistryKeyRef, name: &str) -> Result<StartupState, PortError> {
        Ok(self
            .registry
            .read_value(&approval.value(name))?
            .as_ref()
            .and_then(decode_approval)
            .unwrap_or(StartupState::Enabled))
    }

    // ---- changing ----------------------------------------------------------------------------

    fn prepare(&self, id: &str, enabled: bool) -> Result<Prepared, StartupError> {
        let (origin, name) = parse_entry_id(id)?;
        if origin.scope() == StartupScope::Machine {
            return Err(StartupError::NeedsAdministrator { name });
        }
        let located = self.locate(origin, name)?;
        let wanted = if enabled {
            StartupState::Enabled
        } else {
            StartupState::Disabled
        };
        let current = self
            .state_of(&located.approval, &located.name)
            .map_err(StartupError::Read)?;
        if current == wanted {
            return Ok(Prepared {
                located,
                plan: None,
            });
        }
        let request = OperationRequest::new(
            Operation::SetRegistryValue {
                target: located.approval.value(&located.name),
                value: encode_approval(wanted, self.clock.now()),
            },
            Risk::Safe,
            Evidence::Measured,
        );
        let ports = self.executor.ports();
        let plan = Plan::build(vec![request], ports)
            .map_err(|PlanError::Detect { error, .. }| StartupError::Read(error))?
            .with_label(STARTUP_LABEL);
        Ok(Prepared {
            located,
            plan: Some(plan),
        })
    }

    /// Finds the entry again right now, so a stale id is `NotFound` and a name
    /// can never address anything but a single entry of its source.
    fn locate(&self, origin: StartupOrigin, name: String) -> Result<Located, StartupError> {
        let not_found = || StartupError::NotFound(name.clone());
        if origin.is_folder() {
            let source = self
                .sources
                .folders
                .iter()
                .find(|source| source.origin == origin)
                .ok_or_else(not_found)?;
            // The name becomes a path: it must be exactly one plain component.
            if !is_single_component(&name) {
                return Err(StartupError::InvalidId(entry_id(origin, &name)));
            }
            let folder = source.path.clone().map_err(StartupError::Read)?;
            let info = self
                .file_system
                .inspect(&folder.join(&name))
                .map_err(StartupError::Read)?;
            if !matches!(info, Some(info) if info.kind == EntryKind::File) {
                return Err(not_found());
            }
            Ok(Located {
                approval: source.approval.clone(),
                origin,
                name,
            })
        } else {
            let source = self
                .sources
                .runs
                .iter()
                .find(|source| source.origin == origin)
                .ok_or_else(not_found)?;
            let value = self
                .registry
                .read_value(&source.key.value(&name))
                .map_err(StartupError::Read)?;
            if !matches!(
                value,
                Some(RegistryValue::Sz(_) | RegistryValue::ExpandSz(_))
            ) {
                return Err(not_found());
            }
            Ok(Located {
                approval: source.approval.clone(),
                origin,
                name,
            })
        }
    }
}

struct Prepared {
    located: Located,
    plan: Option<Plan>,
}

fn problem(origin: StartupOrigin, error: &PortError) -> SourceProblem {
    SourceProblem {
        origin,
        reason: error_class(error),
    }
}

/// The id of an entry: its origin and its registry value or file name.
fn entry_id(origin: StartupOrigin, name: &str) -> String {
    format!("{}/{name}", origin.id())
}

fn parse_entry_id(id: &str) -> Result<(StartupOrigin, String), StartupError> {
    id.split_once('/')
        .and_then(|(origin, name)| {
            let origin = StartupOrigin::from_id(origin)?;
            (!name.is_empty()).then(|| (origin, name.to_owned()))
        })
        .ok_or_else(|| StartupError::InvalidId(id.to_owned()))
}

/// Whether `name` is one plain path component: no separator, no `.` or `..`.
fn is_single_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::Normal(part)), None) if part == name
    )
}

/// A program and its arguments as one command line; a program path with spaces
/// is quoted.
fn join_command(program: &str, arguments: &str) -> String {
    let program = program.trim();
    let program = if program.contains(' ') && !program.starts_with('"') {
        format!("\"{program}\"")
    } else {
        program.to_owned()
    };
    let arguments = arguments.trim();
    if arguments.is_empty() {
        program
    } else {
        format!("{program} {arguments}")
    }
}

fn has_extension(name: &str, extension: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

/// A Startup folder file's name for display: shortcuts lose their `.lnk`.
fn display_name(file_name: &str) -> String {
    if has_extension(file_name, "lnk") {
        Path::new(file_name).file_stem().map_or_else(
            || file_name.to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        )
    } else {
        file_name.to_owned()
    }
}

impl fmt::Display for StartupState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        })
    }
}

#[cfg(test)]
mod tests;
