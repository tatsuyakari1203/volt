//! Priority, CPU affinity and Efficiency mode of running apps, and the rules that reapply them.
//!
//! [`ProcessTuner::tune`] changes the settings of every process of an app that the unelevated user
//! may change, as one [`Operation::TuneProcesses`] through the [`Executor`]: the settings the
//! processes have now are read first and written to the undo journal, so the change can be undone
//! while the processes live. A process that has ended by then needs no undo, and the activity
//! record says so. [`ProcessTuner::inspect`] is the dry run: it only reads.
//!
//! Which settings are offered is decided here. The priority classes are Low (idle), Below normal,
//! Normal, Above normal and High; Realtime is never offered, because a busy Realtime process can
//! starve the mouse, the keyboard and the disk of the whole machine, and Windows only grants it to
//! an administrator anyway. A CPU affinity is a set of cores that cannot be empty. Efficiency mode
//! is what Windows does for it: the lowest priority class together with `EcoQoS` (power
//! throttling). Windows-critical processes, system-owned session 0 processes and Volt itself are
//! refused, like [`ProcessMonitor::end_group`](crate::ProcessMonitor::end_group) refuses them.
//!
//! A [`ProcessRule`] is a saved [`TuneRequest`] for an app (its executable name, and its path when
//! it is known). [`ProcessTuner::apply_rules`] reapplies the rules to the processes that started
//! since it last looked. It is called by the shell on a tick of the sampler it already runs; this
//! module starts no thread and holds no timer. With no rule saved a call returns before it reads
//! anything. A rule that cannot be applied is reported once (in the activity log, and as the
//! problem of the rule) and not again on the next tick.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class, error_class_phrase};
use crate::error::{ExecError, PlanError, PortError};
use crate::executor::{Executor, ItemStatus};
use crate::journal::{BatchId, Source};
use crate::label::{Evidence, Risk};
use crate::operation::Operation;
use crate::phrase::Phrase;
use crate::plan::{OperationRequest, Plan};
use crate::ports::{Ports, ProcessInfo, Processes};
use crate::process::{
    FIRST_ENDABLE_PID, Protection, group_key, is_system_owned, is_windows_critical, volt_pids,
};
use crate::settings::{Settings, SettingsExt};
use crate::time::Clock;

/// The label of the journal batch and the activity records of a change of process settings.
pub const TUNE_LABEL: &str = "process.tune";

/// The settings key that holds the saved rules.
pub const RULES_SETTINGS_KEY: &str = "process.rules";

/// The most rules the list keeps. A limit nobody reaches by hand; it bounds what a damaged
/// settings value can make a tick loop over.
pub const MAX_RULES: usize = 100;

/// The priority classes Volt offers. Realtime is deliberately not one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityClass {
    /// Windows calls it Idle; Task Manager calls it Low.
    Idle,
    /// Below normal.
    BelowNormal,
    /// Normal: what an app gets unless it asks for more or less.
    Normal,
    /// Above normal.
    AboveNormal,
    /// High.
    High,
}

impl PriorityClass {
    /// Every class that is offered, lowest first.
    pub const OFFERED: [Self; 5] = [
        Self::Idle,
        Self::BelowNormal,
        Self::Normal,
        Self::AboveNormal,
        Self::High,
    ];

    /// The class as a phrase (`low`, `below normal`, ...).
    #[must_use]
    pub fn phrase(self) -> Phrase {
        Phrase::new(match self {
            Self::Idle => "priority.idle",
            Self::BelowNormal => "priority.belowNormal",
            Self::Normal => "priority.normal",
            Self::AboveNormal => "priority.aboveNormal",
            Self::High => "priority.high",
        })
    }
}

/// Whether Windows lowers the power use of a process (`EcoQoS`), and whether that was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerThrottling {
    /// Nobody asked: Windows decides.
    Default,
    /// Throttling is forced on (Efficiency mode).
    On,
    /// Throttling is forced off.
    Off,
}

/// The settings of one process that Volt can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSettings {
    /// Priority class.
    pub priority: PriorityClass,
    /// The cores it may run on, one bit per logical processor of its group (bit 0 is core 0).
    pub affinity_mask: u64,
    /// The cores the system lets it use. A new affinity must be a non-empty part of this.
    pub system_affinity_mask: u64,
    /// Power throttling.
    pub throttling: PowerThrottling,
}

/// A change to the settings of a process, field by field: `None` leaves a field alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessChange {
    /// The new priority class.
    pub priority: Option<PriorityClass>,
    /// The new affinity (not empty).
    pub affinity_mask: Option<u64>,
    /// The new power throttling.
    pub throttling: Option<PowerThrottling>,
}

impl ProcessChange {
    /// Whether the change leaves everything alone.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.priority.is_none() && self.affinity_mask.is_none() && self.throttling.is_none()
    }

    /// Whether `settings` already are what the change asks for.
    #[must_use]
    pub fn is_met_by(&self, settings: &ProcessSettings) -> bool {
        self.priority.is_none_or(|p| p == settings.priority)
            && self
                .affinity_mask
                .is_none_or(|m| m == settings.affinity_mask)
            && self.throttling.is_none_or(|t| t == settings.throttling)
    }

    /// The change that puts back what `before` held, for the fields this change touches.
    #[must_use]
    pub fn restoring(&self, before: &ProcessSettings) -> Self {
        Self {
            priority: self.priority.map(|_| before.priority),
            affinity_mask: self.affinity_mask.map(|_| before.affinity_mask),
            throttling: self.throttling.map(|_| before.throttling),
        }
    }

    /// The fields of the change, one phrase each.
    #[must_use]
    pub fn phrases(&self) -> Vec<Phrase> {
        let mut parts = Vec::new();
        if let Some(priority) = self.priority {
            parts.push(Phrase::new("tune.priority").phrase("class", priority.phrase()));
        }
        if let Some(mask) = self.affinity_mask {
            parts.push(Phrase::new("tune.affinity").count("count", mask.count_ones()));
        }
        if let Some(throttling) = self.throttling {
            parts.push(Phrase::new(match throttling {
                PowerThrottling::On => "tune.efficiency.on",
                PowerThrottling::Off => "tune.efficiency.off",
                PowerThrottling::Default => "tune.efficiency.default",
            }));
        }
        parts
    }
}

/// One process, as the listing named it: its id and when it started, so an id that was reused
/// since is never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTarget {
    /// Process id.
    pub pid: u32,
    /// Creation time, in 100 ns ticks.
    pub created_at_100ns: u64,
}

fn changed_to_phrase(change: &ProcessChange) -> Phrase {
    Phrase::new("tune.list").list("items", ", ", change.phrases())
}

/// The settings a prior state holds, said for the fields the operation changes: the first
/// process that was still there when they were read.
pub(crate) fn prior_phrase_of(
    change: &ProcessChange,
    prior: &[Option<ProcessSettings>],
) -> Option<Phrase> {
    let first = prior.iter().flatten().next()?;
    Some(changed_to_phrase(&change.restoring(first)))
}

/// What [`Operation::TuneProcesses`] leaves behind, said for the activity log.
pub(crate) fn applied_phrase_of(name: &str, change: &ProcessChange) -> Phrase {
    Phrase::new("summary.processTuned")
        .text("name", name)
        .phrase("change", changed_to_phrase(change))
}

fn not_found_all() -> PortError {
    PortError::NotFound("the processes have ended".to_owned())
}

/// Reads the settings of every target (`None` for one that has ended). Read-only. Fails with
/// [`PortError::NotFound`] when none of them is left.
pub(crate) fn detect_targets(
    ports: &Ports,
    targets: &[ProcessTarget],
) -> Result<Vec<Option<ProcessSettings>>, PortError> {
    let mut found = Vec::with_capacity(targets.len());
    for target in targets {
        match ports
            .processes
            .settings(target.pid, target.created_at_100ns)
        {
            Ok(settings) => found.push(Some(settings)),
            Err(PortError::NotFound(_)) => found.push(None),
            Err(error) => return Err(error),
        }
    }
    if found.iter().all(Option::is_none) {
        return Err(not_found_all());
    }
    Ok(found)
}

/// Whether the state read now is what `change` leaves behind: every process still there holds it
/// and at least one is.
pub(crate) fn targets_met(change: &ProcessChange, now: &[Option<ProcessSettings>]) -> bool {
    now.iter().any(Option::is_some) && now.iter().flatten().all(|s| change.is_met_by(s))
}

/// Applies `change` to every target that still runs. All or nothing: when one fails, the ones
/// already changed are put back and the error is returned.
pub(crate) fn apply_targets(
    ports: &Ports,
    targets: &[ProcessTarget],
    change: &ProcessChange,
) -> Result<(), PortError> {
    let mut done: Vec<(&ProcessTarget, ProcessSettings)> = Vec::new();
    let undo = |done: &[(&ProcessTarget, ProcessSettings)]| {
        for (target, before) in done.iter().rev() {
            // Best effort: the process may have ended, and the caller reports the first error.
            let _ = ports.processes.apply_settings(
                target.pid,
                target.created_at_100ns,
                &change.restoring(before),
            );
        }
    };
    for target in targets {
        let before = match ports
            .processes
            .settings(target.pid, target.created_at_100ns)
        {
            Ok(before) => before,
            Err(PortError::NotFound(_)) => continue,
            Err(error) => {
                undo(&done);
                return Err(error);
            }
        };
        match ports
            .processes
            .apply_settings(target.pid, target.created_at_100ns, change)
        {
            Ok(()) => done.push((target, before)),
            Err(PortError::NotFound(_)) => {}
            Err(error) => {
                undo(&done);
                return Err(error);
            }
        }
    }
    Ok(())
}

/// Puts back what `prior` recorded, for the fields `change` touched. A process that has ended is
/// skipped; when every one has ended there is nothing to restore and the result is
/// [`PortError::NotFound`], which the executor turns into a no-op that says so.
pub(crate) fn revert_targets(
    ports: &Ports,
    targets: &[ProcessTarget],
    change: &ProcessChange,
    prior: &[Option<ProcessSettings>],
) -> Result<(), PortError> {
    if prior.len() != targets.len() {
        return Err(PortError::InvalidInput(
            "journal prior state does not match the operation".to_owned(),
        ));
    }
    let mut restored = 0_u32;
    let mut first_error = None;
    for (target, before) in targets.iter().zip(prior) {
        let Some(before) = before else { continue };
        match ports.processes.apply_settings(
            target.pid,
            target.created_at_100ns,
            &change.restoring(before),
        ) {
            Ok(()) => restored += 1,
            Err(PortError::NotFound(_)) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None if restored == 0 => Err(not_found_all()),
        None => Ok(()),
    }
}

/// What the user asks for. `None` leaves a setting alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuneRequest {
    /// The priority class. Efficiency mode on overrides it with the lowest class.
    pub priority: Option<PriorityClass>,
    /// The cores to run on, one bit per core. Never zero.
    pub affinity_mask: Option<u64>,
    /// Efficiency mode: on is power throttling and the lowest priority, off gives the process
    /// back to the Windows default (it does not force throttling off).
    pub efficiency: Option<bool>,
}

impl TuneRequest {
    /// Whether nothing is asked for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.priority.is_none() && self.affinity_mask.is_none() && self.efficiency.is_none()
    }

    /// Checks the request: an affinity must not be empty.
    pub fn validate(&self) -> Result<(), TuneError> {
        if self.affinity_mask == Some(0) {
            return Err(TuneError::EmptyAffinity);
        }
        Ok(())
    }

    /// The change to the settings the request means. Efficiency mode on is the lowest priority
    /// class together with power throttling, as Windows does it.
    #[must_use]
    pub fn change(&self) -> ProcessChange {
        ProcessChange {
            priority: if self.efficiency == Some(true) {
                Some(PriorityClass::Idle)
            } else {
                self.priority
            },
            affinity_mask: self.affinity_mask,
            throttling: self.efficiency.map(|on| {
                if on {
                    PowerThrottling::On
                } else {
                    PowerThrottling::Default
                }
            }),
        }
    }
}

/// A saved request for an app, reapplied whenever the app starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRule {
    /// Identity of the rule: the executable path in lower case when it was known, else the
    /// executable name in lower case. The same as [`ProcessGroup::key`](crate::ProcessGroup::key).
    pub key: String,
    /// The executable name as the process list spells it.
    pub name: String,
    /// The executable path in lower case, when it was known. A rule with a path matches only that
    /// file; one without matches only processes whose path is unknown too, by name.
    pub path: Option<String>,
    /// What to apply.
    pub request: TuneRequest,
}

impl ProcessRule {
    /// Whether `process` is one the rule is about.
    #[must_use]
    pub fn matches(&self, process: &ProcessInfo) -> bool {
        if !process.name.eq_ignore_ascii_case(&self.name) {
            return false;
        }
        match &self.path {
            // The app's file was unknown when the rule was saved: only a process whose file is
            // unknown too is meant, never a same-named program that has a path.
            None => process.path.as_deref().is_none_or(str::is_empty),
            Some(path) => process
                .path
                .as_deref()
                .is_some_and(|p| p.to_lowercase() == *path),
        }
    }
}

/// Why a tune, an inspection or a rule change was refused. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TuneError {
    /// The app may never be changed.
    #[error("{name} cannot be changed: it is {protection}")]
    Protected {
        /// The executable name asked for.
        name: String,
        /// Why.
        protection: Protection,
    },
    /// No process runs that executable (it may have just closed).
    #[error("{0} is not running")]
    NotRunning(String),
    /// Nothing was asked for.
    #[error("nothing to change was chosen")]
    Nothing,
    /// An affinity with no core in it.
    #[error("at least one core must stay selected")]
    EmptyAffinity,
    /// A core the system does not have.
    #[error("a core was chosen that these processes cannot use")]
    UnknownCore,
    /// The process list could not be read.
    #[error("the process list could not be read: {0}")]
    List(PortError),
    /// The saved rules could not be read or written.
    #[error("the saved rules could not be used: {0}")]
    Rules(PortError),
    /// The rule list is full.
    #[error("at most {MAX_RULES} rules can be saved")]
    TooManyRules,
    /// Applying the change failed. See [`ExecError`].
    #[error("the change could not be made: {0}")]
    Exec(ExecError),
    /// The plan could not be built (the settings could not be read).
    #[error("the settings could not be read: {0}")]
    Read(PortError),
}

/// What an app's processes hold now. Read-only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspection {
    /// The executable name.
    pub name: String,
    /// The executable path, when known.
    pub path: Option<String>,
    /// How many processes the app has that Volt may offer to change.
    pub processes: u32,
    /// How many of them can be changed now.
    pub changeable: u32,
    /// How many Windows would not let Volt change (administrator rights, a protected process).
    pub needs_administrator: u32,
    /// How many could not be read for another reason, or have a priority class Volt does not
    /// offer (Realtime).
    pub unsupported: u32,
    /// The priority class when every changeable process has the same one.
    pub priority: Option<PriorityClass>,
    /// The affinity when every changeable process has the same one.
    pub affinity_mask: Option<u64>,
    /// The cores every changeable process may be given (the part all of them share).
    pub system_affinity_mask: u64,
    /// Whether Efficiency mode is on, when every changeable process agrees.
    pub efficiency: Option<bool>,
}

/// What a tune did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuneReport {
    /// The executable name.
    pub name: String,
    /// Processes whose settings were changed.
    pub changed: u32,
    /// Processes that already had the settings asked for.
    pub already_set: u32,
    /// Processes Windows would not let Volt change.
    pub needs_administrator: u32,
    /// Processes that had ended, or could not be read.
    pub skipped: u32,
    /// The journal batch of the change, when something was changed.
    pub batch: Option<BatchId>,
    /// Whether the activity record could not be written. The change is not affected.
    pub activity_write_failed: bool,
}

/// What a pass over the rules did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RulesRun {
    /// Processes whose settings were changed.
    pub changed: u32,
    /// Rules that could not be applied in this pass (reported once each).
    pub failed: u32,
}

/// A saved rule and how it is going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleStatus {
    /// The rule.
    pub rule: ProcessRule,
    /// The class of the error that kept it from being applied ("access denied"), until it works.
    pub problem: Option<&'static str>,
    /// How many processes of the app the rule could not change since Volt started, although it
    /// changed others (or none). Windows refused them, or they lack a core the rule asks for.
    pub left_alone: u32,
}

/// What applying a rule to new processes did.
#[derive(Default)]
struct RuleApplied {
    changed: u32,
    left_alone: u32,
    /// The class of the first refusal.
    problem: Option<&'static str>,
}

/// A process the tuner may change, with its settings.
struct Candidate {
    process: ProcessInfo,
    settings: ProcessSettings,
}

/// The processes of an app, sorted by what Volt may do with them.
struct Sorted {
    changeable: Vec<Candidate>,
    needs_administrator: u32,
    gone: u32,
    unsupported: u32,
}

#[derive(Default)]
struct RuleState {
    /// Processes already dealt with (changed, conforming or failed), so a tick looks at a process
    /// once. Pruned to the processes that still run.
    handled: HashSet<(u32, u64)>,
    /// The class of the error of each rule that failed, by rule key.
    problems: HashMap<String, &'static str>,
    /// How many processes each rule left alone since Volt started (Windows refused them), by
    /// rule key.
    left_alone: HashMap<String, u32>,
    /// Rules that already wrote their one activity line this session.
    reported: HashSet<String>,
}

/// Changes process settings and keeps the rules, over the [`Processes`] port and an [`Executor`].
pub struct ProcessTuner {
    processes: Arc<dyn Processes>,
    executor: Executor,
    settings: Arc<dyn Settings>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
    own_pid: u32,
    rules: Mutex<Option<Vec<ProcessRule>>>,
    state: Mutex<RuleState>,
}

impl std::fmt::Debug for ProcessTuner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessTuner").finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // Each guarded value is replaced or edited whole, so a poisoned lock holds nothing half-done.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The risk of a request: a high priority or a narrow affinity can starve other apps.
fn risk_of(change: &ProcessChange) -> Risk {
    if matches!(
        change.priority,
        Some(PriorityClass::High | PriorityClass::AboveNormal)
    ) {
        Risk::Caution
    } else {
        Risk::Safe
    }
}

impl ProcessTuner {
    /// A tuner. `executor` journals and records every change; `own_pid` is Volt's own process id
    /// (it and the `WebView` it started are refused).
    #[must_use]
    pub fn new(
        processes: Arc<dyn Processes>,
        executor: Executor,
        settings: Arc<dyn Settings>,
        clock: Arc<dyn Clock>,
        own_pid: u32,
    ) -> Self {
        Self {
            processes,
            executor,
            settings,
            clock,
            activity: None,
            own_pid,
            rules: Mutex::new(None),
            state: Mutex::new(RuleState::default()),
        }
    }

    /// Writes the reports of rules that failed to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// The processes of the app `key` that Volt may change, the way
    /// [`ProcessMonitor::end_group`](crate::ProcessMonitor::end_group) picks them.
    fn app(&self, key: &str) -> Result<(String, Option<String>, Vec<ProcessInfo>), TuneError> {
        let key = key.trim().to_lowercase();
        if is_windows_critical(&key) {
            return Err(TuneError::Protected {
                name: key,
                protection: Protection::Windows,
            });
        }
        let list = self.processes.list().map_err(TuneError::List)?;
        let members: Vec<&ProcessInfo> = list.iter().filter(|p| group_key(p) == key).collect();
        let Some(first) = members.first() else {
            return Err(TuneError::NotRunning(key));
        };
        let (name, path) = (
            first.name.clone(),
            first.path.clone().filter(|p| !p.is_empty()),
        );
        if is_windows_critical(&name) {
            return Err(TuneError::Protected {
                name,
                protection: Protection::Windows,
            });
        }
        let volt = volt_pids(&list, self.own_pid);
        let system = members
            .iter()
            .any(|p| p.pid < FIRST_ENDABLE_PID || is_system_owned(p));
        let targets: Vec<ProcessInfo> = members
            .into_iter()
            .filter(|p| p.pid >= FIRST_ENDABLE_PID && !is_system_owned(p) && !volt.contains(&p.pid))
            .cloned()
            .collect();
        if targets.is_empty() {
            return Err(TuneError::Protected {
                name,
                protection: if system {
                    Protection::Windows
                } else {
                    Protection::Volt
                },
            });
        }
        Ok((name, path, targets))
    }

    /// Reads the settings of each process and sorts them by whether they can be changed.
    fn sort(&self, processes: Vec<ProcessInfo>) -> Sorted {
        let mut sorted = Sorted {
            changeable: Vec::new(),
            needs_administrator: 0,
            gone: 0,
            unsupported: 0,
        };
        for process in processes {
            match self
                .processes
                .settings(process.pid, process.created_at_100ns)
            {
                Ok(settings) => sorted.changeable.push(Candidate { process, settings }),
                Err(PortError::AccessDenied(_)) => sorted.needs_administrator += 1,
                Err(PortError::NotFound(_)) => sorted.gone += 1,
                Err(_) => sorted.unsupported += 1,
            }
        }
        sorted
    }

    /// What the processes of the app `key` hold now, and how many of them Volt may change.
    /// Reads only: this is the dry run of [`ProcessTuner::tune`].
    pub fn inspect(&self, key: &str) -> Result<Inspection, TuneError> {
        let (name, path, processes) = self.app(key)?;
        let total = count(processes.len());
        let sorted = self.sort(processes);
        let common = |pick: &dyn Fn(&ProcessSettings) -> u64| {
            let mut values = sorted.changeable.iter().map(|c| pick(&c.settings));
            let first = values.next()?;
            values.all(|v| v == first).then_some(first)
        };
        let shared_cores = sorted
            .changeable
            .iter()
            .map(|c| c.settings.system_affinity_mask)
            .reduce(|a, b| a & b)
            .unwrap_or(0);
        let priority = sorted
            .changeable
            .first()
            .map(|c| c.settings.priority)
            .filter(|first| {
                sorted
                    .changeable
                    .iter()
                    .all(|c| c.settings.priority == *first)
            });
        Ok(Inspection {
            name,
            path,
            processes: total,
            changeable: count(sorted.changeable.len()),
            needs_administrator: sorted.needs_administrator,
            unsupported: sorted.unsupported,
            priority,
            affinity_mask: common(&|s| s.affinity_mask),
            system_affinity_mask: shared_cores,
            efficiency: common(&|s| u64::from(s.throttling == PowerThrottling::On)).map(|v| v == 1),
        })
    }

    /// Changes the settings of every process of the app `key` that the user may change, and
    /// records it in the undo journal as one entry. Processes that already hold the settings are
    /// left alone; processes Windows will not let Volt change are counted, not changed.
    ///
    /// Protected apps are refused with [`TuneError::Protected`] before anything is read.
    pub fn tune(
        &self,
        key: &str,
        request: &TuneRequest,
        source: Source,
    ) -> Result<TuneReport, TuneError> {
        if request.is_empty() {
            return Err(TuneError::Nothing);
        }
        request.validate()?;
        let (name, _, processes) = self.app(key)?;
        let sorted = self.sort(processes);
        let change = request.change();
        let mut report = TuneReport {
            name: name.clone(),
            changed: 0,
            already_set: 0,
            needs_administrator: sorted.needs_administrator,
            skipped: sorted.gone + sorted.unsupported,
            batch: None,
            activity_write_failed: false,
        };
        if let Some(mask) = change.affinity_mask
            && sorted
                .changeable
                .iter()
                .any(|c| mask & !c.settings.system_affinity_mask != 0)
        {
            return Err(TuneError::UnknownCore);
        }
        let (due, conforming): (Vec<_>, Vec<_>) = sorted
            .changeable
            .iter()
            .partition(|c| !change.is_met_by(&c.settings));
        report.already_set = count(conforming.len());
        if due.is_empty() {
            return Ok(report);
        }
        let targets: Vec<ProcessTarget> = due
            .iter()
            .map(|c| ProcessTarget {
                pid: c.process.pid,
                created_at_100ns: c.process.created_at_100ns,
            })
            .collect();
        let operation = Operation::TuneProcesses {
            name,
            targets,
            change,
        };
        let plan = Plan::build(
            vec![OperationRequest::new(
                operation,
                risk_of(&change),
                Evidence::Anecdotal,
            )],
            self.executor.ports(),
        )
        .map_err(|PlanError::Detect { error, .. }| TuneError::Read(error))?
        .with_label(TUNE_LABEL);
        let applied = self
            .executor
            .apply(&plan, source)
            .map_err(TuneError::Exec)?;
        if let Some(outcome) = applied.outcomes.first()
            && let ItemStatus::Failed { error, .. } = &outcome.status
        {
            return Err(TuneError::Exec(error.clone()));
        }
        report.changed = count(due.len());
        report.batch = Some(applied.batch);
        report.activity_write_failed = self.executor.activity_write_failures() > 0;
        Ok(report)
    }

    // ---- rules ----------------------------------------------------------------------------

    /// The saved rules, read once and kept until a rule is saved or removed.
    fn loaded_rules(&self) -> Result<Vec<ProcessRule>, PortError> {
        let mut cached = lock(&self.rules);
        if let Some(rules) = cached.as_ref() {
            return Ok(rules.clone());
        }
        let stored = self
            .settings
            .get_typed::<Vec<ProcessRule>>(RULES_SETTINGS_KEY)?
            .unwrap_or_default();
        *cached = Some(stored.clone());
        Ok(stored)
    }

    fn store_rules(&self, rules: Vec<ProcessRule>) -> Result<(), PortError> {
        self.settings.set_typed(RULES_SETTINGS_KEY, &rules)?;
        *lock(&self.rules) = Some(rules);
        Ok(())
    }

    /// The saved rules, sorted by name, each with the problem that kept it from being applied.
    pub fn rules(&self) -> Result<Vec<RuleStatus>, TuneError> {
        let mut rules = self.loaded_rules().map_err(TuneError::Rules)?;
        rules.sort_by(|a, b| a.key.cmp(&b.key));
        let state = lock(&self.state);
        Ok(rules
            .into_iter()
            .map(|rule| RuleStatus {
                problem: state.problems.get(&rule.key).copied(),
                left_alone: state.left_alone.get(&rule.key).copied().unwrap_or(0),
                rule,
            })
            .collect())
    }

    /// Saves `request` as the rule of the running app `key`, replacing the rule it had. The app
    /// must be running (that is where its name and path come from) and must be one Volt may
    /// change. The rule is applied to the processes of the app on the next pass of
    /// [`ProcessTuner::apply_rules`].
    pub fn save_rule(&self, key: &str, request: &TuneRequest) -> Result<ProcessRule, TuneError> {
        if request.is_empty() {
            return Err(TuneError::Nothing);
        }
        request.validate()?;
        let (name, path, processes) = self.app(key)?;
        if let Some(mask) = request.affinity_mask {
            // The cores must exist on this machine (as far as the app's processes may use them).
            let sorted = self.sort(processes);
            if sorted
                .changeable
                .iter()
                .any(|c| mask & !c.settings.system_affinity_mask != 0)
            {
                return Err(TuneError::UnknownCore);
            }
        }
        let rule = ProcessRule {
            key: path
                .as_ref()
                .map_or_else(|| name.to_lowercase(), |p| p.to_lowercase()),
            name,
            path: path.map(|p| p.to_lowercase()),
            request: *request,
        };
        let mut rules = self.loaded_rules().map_err(TuneError::Rules)?;
        match rules.iter().position(|r| r.key == rule.key) {
            Some(at) => rules[at] = rule.clone(),
            None if rules.len() >= MAX_RULES => return Err(TuneError::TooManyRules),
            None => rules.push(rule.clone()),
        }
        self.store_rules(rules).map_err(TuneError::Rules)?;
        let mut state = lock(&self.state);
        state.problems.remove(&rule.key);
        state.left_alone.remove(&rule.key);
        state.reported.remove(&rule.key);
        // The running processes are looked at again, so the new rule reaches them.
        state.handled.clear();
        Ok(rule)
    }

    /// Removes the rule `key`. Returns whether it existed. Nothing is changed in running
    /// processes: they keep what the rule gave them until they end or the tune is undone.
    pub fn remove_rule(&self, key: &str) -> Result<bool, TuneError> {
        let key = key.trim().to_lowercase();
        let mut rules = self.loaded_rules().map_err(TuneError::Rules)?;
        let before = rules.len();
        rules.retain(|r| r.key != key);
        if rules.len() == before {
            return Ok(false);
        }
        self.store_rules(rules).map_err(TuneError::Rules)?;
        let mut state = lock(&self.state);
        state.problems.remove(&key);
        state.left_alone.remove(&key);
        state.reported.remove(&key);
        Ok(true)
    }

    /// Reapplies the rules to the processes that started since the last pass. Meant to be called
    /// on a tick the shell already has; it is cheap to call: with no rule it returns at once, and
    /// with rules it reads the process list once.
    pub fn apply_rules(&self) -> RulesRun {
        let mut run = RulesRun::default();
        let rules = match self.loaded_rules() {
            Ok(rules) => rules,
            Err(error) => {
                log::warn!("process rules: cannot read them ({})", error_class(&error));
                return run;
            }
        };
        if rules.is_empty() {
            let mut state = lock(&self.state);
            state.handled.clear();
            state.problems.clear();
            state.left_alone.clear();
            state.reported.clear();
            return run;
        }
        let list = match self.processes.list() {
            Ok(list) => list,
            Err(error) => {
                log::warn!(
                    "process rules: the process list could not be read ({})",
                    error_class(&error)
                );
                return run;
            }
        };
        let volt = volt_pids(&list, self.own_pid);
        let due: Vec<(&ProcessRule, Vec<&ProcessInfo>)> = {
            let mut state = lock(&self.state);
            let alive: HashSet<(u32, u64)> =
                list.iter().map(|p| (p.pid, p.created_at_100ns)).collect();
            state.handled.retain(|id| alive.contains(id));
            rules
                .iter()
                .filter_map(|rule| {
                    let new: Vec<&ProcessInfo> = list
                        .iter()
                        .filter(|p| {
                            rule.matches(p)
                                && p.pid >= FIRST_ENDABLE_PID
                                && !is_system_owned(p)
                                && !is_windows_critical(&p.name)
                                && !volt.contains(&p.pid)
                                && !state.handled.contains(&(p.pid, p.created_at_100ns))
                        })
                        .collect();
                    (!new.is_empty()).then_some((rule, new))
                })
                .collect::<Vec<_>>()
        };
        for (rule, processes) in due {
            lock(&self.state)
                .handled
                .extend(processes.iter().map(|p| (p.pid, p.created_at_100ns)));
            let done = self.apply_rule(rule, &processes);
            if done.left_alone > 0 {
                *lock(&self.state)
                    .left_alone
                    .entry(rule.key.clone())
                    .or_insert(0) += done.left_alone;
            }
            match done.problem {
                Some(class) if done.changed == 0 => {
                    run.failed += 1;
                    self.report_once(rule, class);
                }
                _ => {
                    run.changed += done.changed;
                    if done.changed > 0 {
                        lock(&self.state).problems.remove(&rule.key);
                        self.report_applied(rule, done.changed, done.left_alone);
                    }
                }
            }
        }
        run
    }

    /// Applies one rule to processes that started, process by process, straight through the port.
    ///
    /// This writes no undo entry and no activity record of its own: the rule is the record (the
    /// Rules list shows it, and removing it is the undo for the future), and a browser that starts
    /// a renderer every few seconds would otherwise bury the Undo and activity pages. One activity
    /// line per rule and session is written by [`ProcessTuner::report_applied`]. A process that
    /// cannot be changed is counted as left alone and never makes the others wait.
    fn apply_rule(&self, rule: &ProcessRule, processes: &[&ProcessInfo]) -> RuleApplied {
        let mut done = RuleApplied::default();
        // A rule read from a damaged settings value is checked again, like a request.
        if rule.request.validate().is_err() {
            done.problem = Some("error");
            return done;
        }
        let change = rule.request.change();
        let sorted = self.sort(processes.iter().map(|p| (*p).clone()).collect());
        done.left_alone = sorted.needs_administrator;
        if sorted.needs_administrator > 0 {
            done.problem = Some("access denied");
        }
        for c in &sorted.changeable {
            if change
                .affinity_mask
                .is_some_and(|mask| mask & !c.settings.system_affinity_mask != 0)
            {
                done.left_alone += 1;
                done.problem.get_or_insert("error");
                continue;
            }
            if change.is_met_by(&c.settings) {
                continue;
            }
            match self
                .processes
                .apply_settings(c.process.pid, c.process.created_at_100ns, &change)
            {
                Ok(()) => done.changed += 1,
                Err(PortError::NotFound(_)) => {}
                Err(error) => {
                    // Best effort: do not leave a half-written change (priority set, affinity
                    // refused) behind.
                    let _ = self.processes.apply_settings(
                        c.process.pid,
                        c.process.created_at_100ns,
                        &change.restoring(&c.settings),
                    );
                    done.left_alone += 1;
                    done.problem.get_or_insert(error_class(&error));
                }
            }
        }
        done
    }

    /// Writes the one activity line of a rule for this session: the first time it changes
    /// processes. Later processes of the same app follow the rule without further lines.
    fn report_applied(&self, rule: &ProcessRule, changed: u32, left_alone: u32) {
        if !lock(&self.state).reported.insert(rule.key.clone()) {
            return;
        }
        let Some(log) = &self.activity else { return };
        let processes = Phrase::new("count.process").count("count", changed);
        let summary = if left_alone > 0 {
            Phrase::new("summary.ruleAppliedPartly")
                .text("name", rule.name.clone())
                .phrase("processes", processes)
                .phrase(
                    "left",
                    Phrase::new("count.process").count("count", left_alone),
                )
        } else {
            Phrase::new("summary.ruleApplied")
                .text("name", rule.name.clone())
                .phrase("processes", processes)
        };
        let record = NewActivity::said(
            self.clock.now(),
            Source::Automation,
            ActivityOutcome::Applied,
            summary,
            Some(TUNE_LABEL.to_owned()),
        );
        if let Err(error) = log.record(record) {
            log::warn!(
                "could not write the activity record for a rule ({})",
                error_class(&error)
            );
        }
    }

    /// Remembers that `rule` failed and, the first time since it last worked, writes one activity
    /// record about it.
    fn report_once(&self, rule: &ProcessRule, class: &'static str) {
        let first = lock(&self.state)
            .problems
            .insert(rule.key.clone(), class)
            .is_none();
        if !first {
            return;
        }
        log::warn!(
            "process rule for {} could not be applied ({class})",
            rule.name
        );
        let Some(log) = &self.activity else { return };
        let reason = match class {
            "access denied" => error_class_phrase(&PortError::AccessDenied(String::new())),
            _ => crate::phrase::class_phrase(class),
        };
        let summary = Phrase::new("summary.ruleFailed")
            .text("name", rule.name.clone())
            .phrase("reason", reason);
        let record = NewActivity::said(
            self.clock.now(),
            Source::Automation,
            ActivityOutcome::Failed,
            summary,
            Some(TUNE_LABEL.to_owned()),
        );
        if let Err(error) = log.record(record) {
            log::warn!(
                "could not write the activity record for a rule ({})",
                error_class(&error)
            );
        }
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
