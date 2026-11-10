//! The process list and ending a task.
//!
//! [`ProcessMonitor::sample`] reads the running processes through the [`Processes`] port, groups
//! them by executable (one row per app: the path when it is known, else the name) and works out CPU load from the difference between
//! this reading and the previous one. [`ProcessMonitor::end_group`] ends every process of a group
//! and writes one activity record that says it cannot be undone: an ended process cannot be
//! brought back, so nothing goes through the undo journal. Each process is ended together with the
//! creation time the listing gave it, so a process id that was reused in between is never ended.
//!
//! Which processes may be ended is decided here, not in the UI. Windows needs a fixed set of
//! processes to keep running, and Volt must not end itself or the `WebView` it draws in, so both
//! are refused by [`ProcessMonitor::end_group`] whatever the caller asks for.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use thiserror::Error;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::ports::{ProcessInfo, Processes};
use crate::time::{Clock, Timestamp};

/// The activity label of ending an app.
pub const PROCESS_END_LABEL: &str = "process.end";

/// Executable names (lower case) of the processes Windows cannot run without, or that guard the
/// machine. Ending one blue screens, logs the user out or switches off sound, input or
/// protection, so none is ever offered. System-owned session 0 processes are protected as a
/// whole too (see [`ProcessInfo::session_id`]); this list covers the ones that matter by name.
const WINDOWS_CRITICAL: [&str; 25] = [
    "system",
    "system idle process",
    "secure system",
    "registry",
    "memory compression",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "svchost.exe",
    "dwm.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    // The local session manager: part of the logon machinery.
    "lsm.exe",
    // The Windows audio engine: ending it silences the whole machine until it restarts.
    "audiodg.exe",
    // The user-mode driver host: ending it drops devices such as sensors and phones.
    "wudfhost.exe",
    // The print spooler.
    "spoolsv.exe",
    // Text input (keyboard layouts, IME): ending it breaks typing in other apps.
    "ctfmon.exe",
    // Device association: pairing of Bluetooth and network devices.
    "dashost.exe",
    // Microsoft Defender engine, network inspection and the Security Center service.
    "msmpeng.exe",
    "nissrv.exe",
    "securityhealthservice.exe",
];

/// The lowest process id that may be ended. Ids 0 (idle) and 4 (System) are the kernel.
const FIRST_ENDABLE_PID: u32 = 5;

/// 100 ns ticks in one millisecond.
const TICKS_PER_MILLI: u64 = 10_000;

/// Why a group is never offered for ending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Windows needs it.
    Windows,
    /// It is Volt itself or the `WebView` Volt draws in.
    Volt,
}

impl std::fmt::Display for Protection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Windows => "needed by Windows",
            Self::Volt => "Volt itself",
        })
    }
}

/// Whether a group may be ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endability {
    /// It may be ended after a confirmation.
    Allowed,
    /// It may be ended, but it is the Windows shell (taskbar, desktop, File Explorer windows), so
    /// the confirmation gives an extra warning.
    Explorer,
    /// It is never ended.
    Protected(Protection),
}

impl Endability {
    /// Whether the group may be ended at all.
    #[must_use]
    pub fn can_end(self) -> bool {
        !matches!(self, Self::Protected(_))
    }
}

/// Whether `name` is a process Windows needs (case does not matter).
#[must_use]
pub fn is_windows_critical(name: &str) -> bool {
    let lower = name.to_lowercase();
    WINDOWS_CRITICAL.contains(&lower.as_str())
}

/// All processes of one executable, shown as one row.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessGroup {
    /// Stable identity of the row: the executable path in lower case when it is known, else the
    /// executable name in lower case. Two apps with the same file name in different folders are
    /// two groups.
    pub key: String,
    /// The executable name as the first of its processes spells it.
    pub name: String,
    /// The executable path, when it could be read.
    pub path: Option<String>,
    /// How many processes run it.
    pub process_count: u32,
    /// How many of them ending the group would end (Volt's own `WebView` processes and system-owned
    /// session 0 processes are left out of a group they share with an app). 0 for a protected
    /// group.
    pub end_count: u32,
    /// Private working set of all its processes, in bytes.
    pub memory_bytes: u64,
    /// Share of all CPU capacity (all logical processors together are 100) the group used since
    /// the previous sample. `None` for the first sample, which has nothing to compare with.
    pub cpu_percent: Option<f64>,
    /// Whether and how it may be ended.
    pub endability: Endability,
}

/// One reading of the process list.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSample {
    /// One entry per executable, the biggest memory user first.
    pub groups: Vec<ProcessGroup>,
    /// How many processes there are in all.
    pub process_count: u32,
}

/// What happened to one process of an ended group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndStatus {
    /// It was ended.
    Ended,
    /// Windows refused: ending it needs administrator rights (or it belongs to another user).
    NeedsAdministrator,
    /// It had already exited.
    AlreadyGone,
    /// Ending it failed for another reason, named by its class ("error", "unsupported").
    Failed(&'static str),
}

/// One process of an ended group and how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessEnd {
    /// Process id.
    pub pid: u32,
    /// What happened.
    pub status: EndStatus,
}

/// What ending a group did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndReport {
    /// The app's executable name.
    pub name: String,
    /// One entry per process that was tried.
    pub outcomes: Vec<ProcessEnd>,
    /// Whether the activity record could not be written. The processes are not affected.
    pub activity_write_failed: bool,
}

impl EndReport {
    fn count(&self, wanted: impl Fn(EndStatus) -> bool) -> u32 {
        let n = self.outcomes.iter().filter(|o| wanted(o.status)).count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Processes that were ended.
    #[must_use]
    pub fn ended(&self) -> u32 {
        self.count(|s| s == EndStatus::Ended)
    }

    /// Processes Windows would not let Volt end.
    #[must_use]
    pub fn needs_administrator(&self) -> u32 {
        self.count(|s| s == EndStatus::NeedsAdministrator)
    }

    /// Processes that had already exited.
    #[must_use]
    pub fn already_gone(&self) -> u32 {
        self.count(|s| s == EndStatus::AlreadyGone)
    }

    /// Processes that could not be ended for another reason.
    #[must_use]
    pub fn failed(&self) -> u32 {
        self.count(|s| matches!(s, EndStatus::Failed(_)))
    }
}

/// Why a group was not ended. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EndError {
    /// The group may never be ended.
    #[error("{name} cannot be ended: it is {protection}")]
    Protected {
        /// The executable name asked for.
        name: String,
        /// Why.
        protection: Protection,
    },
    /// No process runs that executable (it may have just closed).
    #[error("{0} is not running")]
    NotRunning(String),
    /// The process list could not be read.
    #[error("the process list could not be read: {0}")]
    List(PortError),
}

/// CPU readings of the previous sample, by process id: (creation time, CPU time).
#[derive(Debug)]
struct Previous {
    at: Timestamp,
    cpu_time: HashMap<u32, (u64, u64)>,
}

/// Running totals of one group while a sample is built.
struct Totals {
    group: ProcessGroup,
    cpu_delta: u64,
    volt_count: u32,
    system_count: u32,
}

/// Reads the process list and ends apps, over the [`Processes`] port.
pub struct ProcessMonitor {
    processes: Arc<dyn Processes>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
    own_pid: u32,
    logical_cpus: u32,
    previous: Mutex<Option<Previous>>,
}

impl std::fmt::Debug for ProcessMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessMonitor").finish_non_exhaustive()
    }
}

impl ProcessMonitor {
    /// A monitor over `processes`. `own_pid` is the id of Volt's own process (its descendants,
    /// the `WebView`, are protected too) and `logical_cpus` the number of logical processors, which
    /// CPU load is shared over.
    #[must_use]
    pub fn new(
        processes: Arc<dyn Processes>,
        clock: Arc<dyn Clock>,
        own_pid: u32,
        logical_cpus: u32,
    ) -> Self {
        Self {
            processes,
            clock,
            activity: None,
            own_pid,
            logical_cpus: logical_cpus.max(1),
            previous: Mutex::new(None),
        }
    }

    /// Writes one activity record per ended group to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// The number of logical processors CPU load is shared over.
    #[must_use]
    pub fn logical_cpus(&self) -> u32 {
        self.logical_cpus
    }

    /// Reads the process list, grouped by app. Reads only. CPU load is measured against the
    /// previous call, so call it at a steady pace.
    pub fn sample(&self) -> Result<ProcessSample, PortError> {
        let list = self.processes.list()?;
        let now = self.clock.now();
        let (deltas, elapsed_ticks) = self.take_deltas(&list, now);
        let volt = volt_pids(&list, self.own_pid);
        let capacity =
            elapsed_ticks.map(|ticks| ticks.saturating_mul(u64::from(self.logical_cpus)));

        let mut by_key: HashMap<String, Totals> = HashMap::new();
        for process in &list {
            let key = group_key(process);
            let totals = by_key.entry(key.clone()).or_insert_with(|| Totals {
                group: ProcessGroup {
                    key,
                    name: process.name.clone(),
                    path: process.path.clone().filter(|p| !p.is_empty()),
                    process_count: 0,
                    end_count: 0,
                    memory_bytes: 0,
                    cpu_percent: None,
                    endability: Endability::Allowed,
                },
                cpu_delta: 0,
                volt_count: 0,
                system_count: 0,
            });
            totals.group.process_count += 1;
            totals.group.memory_bytes = totals
                .group
                .memory_bytes
                .saturating_add(process.working_set_bytes);
            totals.cpu_delta = totals
                .cpu_delta
                .saturating_add(deltas.get(&process.pid).copied().unwrap_or(0));
            if volt.contains(&process.pid) {
                totals.volt_count += 1;
            } else if is_system_owned(process) {
                totals.system_count += 1;
            }
        }
        let mut groups: Vec<ProcessGroup> = by_key
            .into_values()
            .map(|mut totals| {
                let group = &mut totals.group;
                group.endability = endability(
                    &group.name.to_lowercase(),
                    totals.volt_count,
                    totals.system_count,
                    group.process_count,
                );
                group.end_count = if group.endability.can_end() {
                    group.process_count - totals.volt_count - totals.system_count
                } else {
                    0
                };
                group.cpu_percent = capacity
                    .filter(|c| *c > 0)
                    .map(|c| percent(totals.cpu_delta, c));
                totals.group
            })
            .collect();
        groups.sort_by(|a, b| {
            b.memory_bytes
                .cmp(&a.memory_bytes)
                .then_with(|| a.key.cmp(&b.key))
        });
        Ok(ProcessSample {
            groups,
            process_count: u32::try_from(list.len()).unwrap_or(u32::MAX),
        })
    }

    /// CPU time used by each process since the previous sample, and the time between the two
    /// samples in 100 ns ticks. `None` for the time when there is no usable previous sample. A
    /// process that is new, or whose id was reused (another creation time, or a counter that went
    /// backwards), contributes 0.
    fn take_deltas(
        &self,
        list: &[ProcessInfo],
        now: Timestamp,
    ) -> (HashMap<u32, u64>, Option<u64>) {
        let mut previous = self.previous.lock().unwrap_or_else(PoisonError::into_inner);
        let mut deltas = HashMap::new();
        let mut elapsed = None;
        if let Some(before) = previous.as_ref() {
            let millis = now
                .as_unix_millis()
                .saturating_sub(before.at.as_unix_millis());
            let Ok(millis) = u64::try_from(millis) else {
                // The clock went backwards: keep the old baseline.
                return (deltas, None);
            };
            if millis == 0 {
                // Two reads in the same millisecond: keep the old baseline.
                return (deltas, None);
            }
            elapsed = Some(millis.saturating_mul(TICKS_PER_MILLI));
            for process in list {
                if let Some(&(created, old)) = before.cpu_time.get(&process.pid)
                    && created == process.created_at_100ns
                {
                    deltas.insert(process.pid, process.cpu_time_100ns.saturating_sub(old));
                }
            }
        }
        *previous = Some(Previous {
            at: now,
            cpu_time: list
                .iter()
                .map(|p| (p.pid, (p.created_at_100ns, p.cpu_time_100ns)))
                .collect(),
        });
        (deltas, elapsed)
    }

    /// Ends every process of the group `key` (a [`ProcessGroup::key`]; case does not matter) and
    /// records it. The list is read again first, so only what runs now is ended, and each process
    /// is ended together with the creation time just read: one whose id was reused since is
    /// reported as [`EndStatus::AlreadyGone`] and left alone.
    ///
    /// Windows-critical executables, system-owned session 0 processes and Volt are refused with
    /// [`EndError::Protected`], before anything is touched. Volt's own `WebView` processes are
    /// skipped when another app shares their executable. A process Windows will not let Volt end
    /// is reported as [`EndStatus::NeedsAdministrator`]; the others are still ended.
    pub fn end_group(&self, name: &str, source: Source) -> Result<EndReport, EndError> {
        let key = name.trim().to_lowercase();
        if is_windows_critical(&key) {
            return Err(EndError::Protected {
                name: name.to_owned(),
                protection: Protection::Windows,
            });
        }
        let list = self.processes.list().map_err(EndError::List)?;
        let members: Vec<&ProcessInfo> = list.iter().filter(|p| group_key(p) == key).collect();
        let Some(first) = members.first() else {
            return Err(EndError::NotRunning(name.to_owned()));
        };
        let display = first.name.clone();
        if is_windows_critical(&display) {
            return Err(EndError::Protected {
                name: display,
                protection: Protection::Windows,
            });
        }
        let volt = volt_pids(&list, self.own_pid);
        let system = members
            .iter()
            .any(|p| p.pid < FIRST_ENDABLE_PID || is_system_owned(p));
        let mut targets: Vec<&ProcessInfo> = members
            .into_iter()
            .filter(|p| p.pid >= FIRST_ENDABLE_PID && !is_system_owned(p) && !volt.contains(&p.pid))
            .collect();
        if targets.is_empty() {
            return Err(EndError::Protected {
                name: display,
                protection: if system {
                    Protection::Windows
                } else {
                    Protection::Volt
                },
            });
        }
        // Children first: ending a parent first would only make its children report "gone".
        let ids: HashSet<u32> = targets.iter().map(|p| p.pid).collect();
        targets.sort_by_key(|p| !ids.contains(&p.parent_pid));
        let outcomes = targets
            .iter()
            .map(|p| ProcessEnd {
                pid: p.pid,
                status: match self.processes.terminate(p.pid, p.created_at_100ns) {
                    Ok(()) => EndStatus::Ended,
                    Err(PortError::AccessDenied(_)) => EndStatus::NeedsAdministrator,
                    Err(PortError::NotFound(_)) => EndStatus::AlreadyGone,
                    Err(error) => EndStatus::Failed(error_class(&error)),
                },
            })
            .collect();
        let mut report = EndReport {
            name: display,
            outcomes,
            activity_write_failed: false,
        };
        report.activity_write_failed = !self.record(&report, source);
        Ok(report)
    }

    /// Writes the activity record. Returns whether it was written (or there is no log).
    fn record(&self, report: &EndReport, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        let activity = NewActivity {
            timestamp: self.clock.now(),
            source,
            outcome: if report.ended() > 0 {
                ActivityOutcome::Applied
            } else {
                ActivityOutcome::Failed
            },
            summary: summary(report),
            batch: None,
            entry: None,
            label: Some(PROCESS_END_LABEL.to_owned()),
        };
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for ending an app ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

/// `percent` of `capacity` that `used` is, between 0 and 100.
fn percent(used: u64, capacity: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)] // a display figure; the counters stay far below 2^53
    let share = used as f64 / capacity as f64 * 100.0;
    share.clamp(0.0, 100.0)
}

/// The identity of the group a process belongs to: its executable path in lower case when known,
/// else its executable name in lower case.
fn group_key(process: &ProcessInfo) -> String {
    match process.path.as_deref() {
        Some(path) if !path.is_empty() => path.to_lowercase(),
        _ => process.name.to_lowercase(),
    }
}

/// Whether a process is system-owned: it runs in session 0, where Windows keeps its services and
/// system processes. A user's apps never run there.
fn is_system_owned(process: &ProcessInfo) -> bool {
    process.session_id == 0
}

/// `name` is the executable name in lower case; `volt` and `system` count the processes of the
/// group that are Volt's or system-owned, out of `total`.
fn endability(name: &str, volt: u32, system: u32, total: u32) -> Endability {
    if is_windows_critical(name) || (system > 0 && system + volt == total) {
        Endability::Protected(Protection::Windows)
    } else if volt == total {
        Endability::Protected(Protection::Volt)
    } else if name == "explorer.exe" {
        Endability::Explorer
    } else {
        Endability::Allowed
    }
}

/// The ids of Volt's own process and everything it started (the `WebView`), found through the
/// parent ids of the list.
fn volt_pids(list: &[ProcessInfo], own_pid: u32) -> HashSet<u32> {
    let mut owned = HashSet::from([own_pid]);
    loop {
        let before = owned.len();
        for process in list {
            if process.pid != 0 && owned.contains(&process.parent_pid) {
                owned.insert(process.pid);
            }
        }
        if owned.len() == before {
            return owned;
        }
    }
}

/// One line for the activity log: the app, what happened to its processes (counts only, never a
/// port message) and that it cannot be undone.
fn summary(report: &EndReport) -> String {
    let total = report.outcomes.len();
    let mut parts = vec![format!(
        "Ended {} (cannot be undone): {} of {} processes",
        report.name,
        report.ended(),
        total
    )];
    if report.needs_administrator() > 0 {
        parts.push(format!(
            "{} need administrator rights",
            report.needs_administrator()
        ));
    }
    if report.already_gone() > 0 {
        parts.push(format!("{} had already exited", report.already_gone()));
    }
    if report.failed() > 0 {
        parts.push(format!("{} could not be ended", report.failed()));
    }
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{Event, EventLog, FakeActivityLog, FakeProcesses};
    use std::sync::atomic::{AtomicI64, Ordering};

    const MIB: u64 = 1 << 20;
    const VOLT: u32 = 100;

    /// A clock the test sets by hand.
    #[derive(Debug, Default)]
    struct SetClock(AtomicI64);

    impl SetClock {
        fn set(&self, millis: i64) {
            self.0.store(millis, Ordering::SeqCst);
        }
    }

    impl Clock for SetClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(self.0.load(Ordering::SeqCst))
        }
    }

    struct Setup {
        processes: Arc<FakeProcesses>,
        log: Arc<FakeActivityLog>,
        events: EventLog,
        clock: Arc<SetClock>,
        monitor: ProcessMonitor,
    }

    fn setup() -> Setup {
        let events = EventLog::default();
        let processes = Arc::new(FakeProcesses::with_log(events.clone()));
        let log = Arc::new(FakeActivityLog::new());
        let clock = Arc::new(SetClock::default());
        clock.set(1_000);
        let monitor = ProcessMonitor::new(processes.clone(), clock.clone(), VOLT, 4)
            .with_activity_log(log.clone());
        Setup {
            processes,
            log,
            events,
            clock,
            monitor,
        }
    }

    fn group<'a>(sample: &'a ProcessSample, key: &str) -> &'a ProcessGroup {
        sample
            .groups
            .iter()
            .find(|g| g.key == key)
            .unwrap_or_else(|| panic!("no group {key}"))
    }

    #[test]
    fn processes_are_grouped_by_executable_name_ignoring_case_and_summed() {
        let s = setup();
        s.processes.add(10, "Chrome.exe", 300 * MIB);
        s.processes.add(11, "chrome.exe", 200 * MIB);
        s.processes.add(12, "notepad.exe", 5 * MIB);
        s.processes.add(13, "code.exe", 900 * MIB);
        let sample = s.monitor.sample().expect("sample");
        assert_eq!(sample.process_count, 4);
        let keys: Vec<_> = sample.groups.iter().map(|g| g.key.as_str()).collect();
        // Biggest memory user first.
        assert_eq!(keys, ["code.exe", "chrome.exe", "notepad.exe"]);
        let chrome = group(&sample, "chrome.exe");
        assert_eq!(chrome.name, "Chrome.exe");
        assert_eq!((chrome.process_count, chrome.end_count), (2, 2));
        assert_eq!(chrome.memory_bytes, 500 * MIB);
        assert_eq!(chrome.endability, Endability::Allowed);
    }

    #[test]
    fn cpu_load_is_the_difference_of_two_samples_over_the_logical_processors() {
        let s = setup();
        s.processes.add_process(10, 1, "a.exe", 0, 0);
        s.processes.add_process(11, 1, "a.exe", 0, 0);
        s.processes.add_process(12, 1, "b.exe", 0, 0);
        let first = s.monitor.sample().expect("first");
        // Nothing to compare with yet.
        assert!(first.groups.iter().all(|g| g.cpu_percent.is_none()));

        // Two seconds later, with 4 logical processors: 8 s of CPU would be 100 %.
        s.clock.set(3_000);
        let sec = 10_000_000;
        s.processes.set_cpu_time(10, sec); // 1 s
        s.processes.set_cpu_time(11, sec); // 1 s
        s.processes.set_cpu_time(12, sec / 2);
        let second = s.monitor.sample().expect("second");
        let a = group(&second, "a.exe").cpu_percent.expect("a");
        let b = group(&second, "b.exe").cpu_percent.expect("b");
        assert!((a - 25.0).abs() < 1e-9, "a was {a}");
        assert!((b - 6.25).abs() < 1e-9, "b was {b}");
    }

    #[test]
    fn a_new_process_and_a_reused_id_add_no_cpu_load_and_load_is_capped() {
        let s = setup();
        s.processes.add_process(10, 1, "a.exe", 0, 5_000_000_000);
        s.monitor.sample().expect("first");
        s.clock.set(2_000);
        // Id 10 now reports less CPU time than before (the id was reused); id 11 is new.
        s.processes.set_cpu_time(10, 1_000);
        s.processes.add_process(11, 1, "b.exe", 0, 9_000_000_000);
        let sample = s.monitor.sample().expect("second");
        assert_eq!(group(&sample, "a.exe").cpu_percent, Some(0.0));
        assert_eq!(group(&sample, "b.exe").cpu_percent, Some(0.0));
        // More CPU time than the machine has never shows above 100.
        s.clock.set(2_001);
        s.processes.set_cpu_time(10, 4_000_000_000);
        let capped = s.monitor.sample().expect("third");
        assert_eq!(group(&capped, "a.exe").cpu_percent, Some(100.0));
    }

    #[test]
    fn two_reads_in_the_same_millisecond_keep_the_old_baseline() {
        let s = setup();
        s.processes.add_process(10, 1, "a.exe", 0, 0);
        s.monitor.sample().expect("first");
        let again = s.monitor.sample().expect("same instant");
        assert_eq!(group(&again, "a.exe").cpu_percent, None);
        s.clock.set(2_000);
        s.processes.set_cpu_time(10, 10_000_000);
        let later = s.monitor.sample().expect("later");
        assert!(group(&later, "a.exe").cpu_percent.is_some());
    }

    #[test]
    fn windows_critical_processes_are_protected_and_never_ended() {
        let s = setup();
        // Every name on the list, one process each.
        let names = WINDOWS_CRITICAL;
        for (i, name) in names.iter().enumerate() {
            s.processes
                .add(500 + u32::try_from(i).expect("small"), name, MIB);
        }
        let sample = s.monitor.sample().expect("sample");
        for group in &sample.groups {
            assert_eq!(
                group.endability,
                Endability::Protected(Protection::Windows),
                "{}",
                group.name
            );
            assert_eq!(group.end_count, 0);
        }
        for name in names {
            // Whatever case the caller uses.
            for spelling in [name.to_owned(), name.to_uppercase()] {
                assert_eq!(
                    s.monitor.end_group(&spelling, Source::Manual),
                    Err(EndError::Protected {
                        name: spelling.clone(),
                        protection: Protection::Windows
                    })
                );
            }
        }
        assert!(s.events.events().is_empty());
        assert!(s.log.records().is_empty());
        assert_eq!(s.processes.list().expect("list").len(), names.len());
    }

    #[test]
    fn each_protected_name_is_critical_in_any_case_and_the_new_ones_are_on_the_list() {
        for name in WINDOWS_CRITICAL {
            assert!(is_windows_critical(name), "{name}");
            assert!(is_windows_critical(&name.to_uppercase()), "{name}");
        }
        for name in [
            "lsm.exe",
            "audiodg.exe",
            "wudfhost.exe",
            "spoolsv.exe",
            "ctfmon.exe",
            "dashost.exe",
            "msmpeng.exe",
            "nissrv.exe",
            "securityhealthservice.exe",
        ] {
            assert!(WINDOWS_CRITICAL.contains(&name), "{name}");
            // Protected even when its path is known and the key is the path.
            let s = setup();
            s.processes.add(600, name, MIB);
            s.processes
                .set_path(600, &format!("C:\\Windows\\System32\\{name}"));
            let sample = s.monitor.sample().expect("sample");
            assert_eq!(
                sample.groups[0].endability,
                Endability::Protected(Protection::Windows),
                "{name}"
            );
            assert!(matches!(
                s.monitor.end_group(&sample.groups[0].key, Source::Manual),
                Err(EndError::Protected {
                    protection: Protection::Windows,
                    ..
                })
            ));
            assert!(s.events.events().is_empty(), "{name}");
        }
    }

    #[test]
    fn session_zero_processes_are_never_offered_for_ending() {
        let s = setup();
        s.processes.add(700, "vendorsvc.exe", MIB);
        s.processes.set_session(700, 0);
        // The same executable also runs in a user's session: only that one is endable.
        s.processes.add(701, "mixed.exe", MIB);
        s.processes.set_session(701, 0);
        s.processes.add(702, "mixed.exe", MIB);
        let sample = s.monitor.sample().expect("sample");
        let svc = group(&sample, "vendorsvc.exe");
        assert_eq!(svc.endability, Endability::Protected(Protection::Windows));
        assert_eq!(svc.end_count, 0);
        let mixed = group(&sample, "mixed.exe");
        assert_eq!(mixed.endability, Endability::Allowed);
        assert_eq!((mixed.process_count, mixed.end_count), (2, 1));
        assert_eq!(
            s.monitor.end_group("vendorsvc.exe", Source::Manual),
            Err(EndError::Protected {
                name: "vendorsvc.exe".into(),
                protection: Protection::Windows
            })
        );
        let report = s
            .monitor
            .end_group("mixed.exe", Source::Manual)
            .expect("end");
        assert_eq!(report.ended(), 1);
        let left: Vec<u32> = s
            .processes
            .list()
            .expect("list")
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(left, [700, 701]);
    }

    #[test]
    fn apps_with_the_same_file_name_but_different_paths_are_separate_groups() {
        let s = setup();
        s.processes.add(10, "updater.exe", MIB);
        s.processes.set_path(10, "C:\\Apps\\One\\Updater.exe");
        s.processes.add(11, "updater.exe", MIB);
        s.processes.set_path(11, "C:\\Apps\\One\\Updater.exe");
        s.processes.add(12, "updater.exe", 2 * MIB);
        s.processes.set_path(12, "C:\\Apps\\Two\\updater.exe");
        // No path known: grouped by name.
        s.processes.add(13, "updater.exe", MIB);
        let sample = s.monitor.sample().expect("sample");
        assert_eq!(sample.groups.len(), 3);
        let one = group(&sample, "c:\\apps\\one\\updater.exe");
        assert_eq!(one.process_count, 2);
        assert_eq!(one.path.as_deref(), Some("C:\\Apps\\One\\Updater.exe"));
        assert_eq!(
            group(&sample, "c:\\apps\\two\\updater.exe").process_count,
            1
        );
        let unknown = group(&sample, "updater.exe");
        assert_eq!((unknown.process_count, unknown.path.clone()), (1, None));

        // Ending one group leaves the other apps alone.
        let report = s
            .monitor
            .end_group("C:\\APPS\\ONE\\UPDATER.EXE", Source::Manual)
            .expect("end");
        assert_eq!(report.ended(), 2);
        let left: Vec<u32> = s
            .processes
            .list()
            .expect("list")
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(left, [12, 13]);
    }

    #[test]
    fn a_reused_process_id_is_reported_gone_and_never_ended() {
        let s = setup();
        s.processes.add(200, "app.exe", MIB);
        s.processes.add(201, "app.exe", MIB);
        s.processes.set_created(200, 5_000);
        s.processes.set_created(201, 6_000);
        // Id 200 now belongs to a newer process than the list the monitor reads shows.
        s.processes.list_stale_creation(200, 4_000);
        let report = s.monitor.end_group("app.exe", Source::Manual).expect("end");
        let status = |pid| {
            report
                .outcomes
                .iter()
                .find(|o| o.pid == pid)
                .expect("outcome")
                .status
        };
        assert_eq!(status(200), EndStatus::AlreadyGone);
        assert_eq!(status(201), EndStatus::Ended);
        assert_eq!((report.ended(), report.already_gone()), (1, 1));
        assert_eq!(s.events.events(), [Event::ProcessTerminated(201)]);
        let left: Vec<u32> = s
            .processes
            .list()
            .expect("list")
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(left, [200]);
    }

    #[test]
    fn a_reused_id_adds_no_cpu_load_even_when_its_counter_went_up() {
        let s = setup();
        s.processes.add_process(10, 1, "a.exe", 0, 1_000);
        s.processes.set_created(10, 1);
        s.monitor.sample().expect("first");
        s.clock.set(2_000);
        // Same id, another process that has already used far more CPU.
        s.processes.set_created(10, 2);
        s.processes.set_cpu_time(10, 9_000_000_000);
        let sample = s.monitor.sample().expect("second");
        assert_eq!(group(&sample, "a.exe").cpu_percent, Some(0.0));
    }

    #[test]
    fn volt_and_its_webview_are_protected_even_when_asked_for_by_name() {
        let s = setup();
        s.processes
            .add_process(VOLT, 1, "volt-desktop.exe", 80 * MIB, 0);
        s.processes
            .add_process(101, VOLT, "msedgewebview2.exe", 60 * MIB, 0);
        // A grandchild is Volt's too.
        s.processes
            .add_process(102, 101, "msedgewebview2.exe", 40 * MIB, 0);
        let sample = s.monitor.sample().expect("sample");
        assert_eq!(
            group(&sample, "volt-desktop.exe").endability,
            Endability::Protected(Protection::Volt)
        );
        let webview = group(&sample, "msedgewebview2.exe");
        assert_eq!(webview.endability, Endability::Protected(Protection::Volt));
        assert_eq!(webview.end_count, 0);
        for name in ["volt-desktop.exe", "msedgewebview2.exe"] {
            assert!(matches!(
                s.monitor.end_group(name, Source::Manual),
                Err(EndError::Protected {
                    protection: Protection::Volt,
                    ..
                })
            ));
        }
        assert!(s.events.events().is_empty());
    }

    #[test]
    fn another_apps_webview_is_ended_but_volts_own_is_left_alone() {
        let s = setup();
        s.processes.add_process(VOLT, 1, "volt-desktop.exe", MIB, 0);
        s.processes
            .add_process(101, VOLT, "msedgewebview2.exe", MIB, 0);
        s.processes
            .add_process(200, 1, "msedgewebview2.exe", MIB, 0);
        s.processes
            .add_process(201, 200, "msedgewebview2.exe", MIB, 0);
        let sample = s.monitor.sample().expect("sample");
        let webview = group(&sample, "msedgewebview2.exe");
        assert_eq!((webview.process_count, webview.end_count), (3, 2));
        assert_eq!(webview.endability, Endability::Allowed);

        let report = s
            .monitor
            .end_group("msedgewebview2.exe", Source::Manual)
            .expect("end");
        assert_eq!(report.ended(), 2);
        let remaining: Vec<u32> = s
            .processes
            .list()
            .expect("list")
            .iter()
            .map(|p| p.pid)
            .collect();
        assert_eq!(remaining, [VOLT, 101]);
    }

    #[test]
    fn explorer_may_be_ended_but_is_marked_for_an_extra_warning() {
        let s = setup();
        s.processes.add(300, "explorer.exe", 90 * MIB);
        let sample = s.monitor.sample().expect("sample");
        let explorer = group(&sample, "explorer.exe");
        assert_eq!(explorer.endability, Endability::Explorer);
        assert!(explorer.endability.can_end());
        assert_eq!(explorer.end_count, 1);
    }

    #[test]
    fn ending_a_group_ends_children_first_and_records_one_activity_that_cannot_be_undone() {
        let s = setup();
        s.processes.add_process(200, 1, "chrome.exe", MIB, 0);
        s.processes.add_process(201, 200, "chrome.exe", MIB, 0);
        s.processes.add_process(202, 200, "chrome.exe", MIB, 0);
        s.processes.add_process(300, 1, "notepad.exe", MIB, 0);
        let report = s
            .monitor
            .end_group("CHROME.EXE", Source::Manual)
            .expect("end");
        assert_eq!(report.name, "chrome.exe");
        assert_eq!((report.ended(), report.failed()), (3, 0));
        assert!(!report.activity_write_failed);
        let events = s.events.events();
        assert_eq!(events.last(), Some(&Event::ProcessTerminated(200)));
        assert_eq!(events.len(), 3);
        // Nothing else was touched.
        assert_eq!(s.processes.list().expect("list").len(), 1);

        let records = s.log.records();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.outcome, ActivityOutcome::Applied);
        assert_eq!(record.label.as_deref(), Some(PROCESS_END_LABEL));
        assert_eq!((record.batch, record.entry), (None, None));
        assert!(
            record
                .summary
                .starts_with("Ended chrome.exe (cannot be undone)")
        );
        assert!(record.summary.contains("3 of 3 processes"));
    }

    #[test]
    fn a_process_windows_refuses_is_reported_per_process_and_the_rest_still_end() {
        let s = setup();
        s.processes.add(200, "vendor.exe", MIB);
        s.processes.add(201, "vendor.exe", MIB);
        s.processes.add(202, "vendor.exe", MIB);
        s.processes.add(203, "vendor.exe", MIB);
        s.processes
            .fail_terminate(201, PortError::AccessDenied("secret detail".into()));
        s.processes
            .fail_terminate(202, PortError::Other("secret detail".into()));
        let report = s
            .monitor
            .end_group("vendor.exe", Source::Manual)
            .expect("end");
        let status = |pid| {
            report
                .outcomes
                .iter()
                .find(|o| o.pid == pid)
                .expect("outcome")
                .status
        };
        assert_eq!(status(200), EndStatus::Ended);
        assert_eq!(status(201), EndStatus::NeedsAdministrator);
        assert_eq!(status(202), EndStatus::Failed("error"));
        assert_eq!(status(203), EndStatus::Ended);
        assert_eq!(
            (
                report.ended(),
                report.needs_administrator(),
                report.failed()
            ),
            (2, 1, 1)
        );
        let record = &s.log.records()[0];
        assert!(record.summary.contains("1 need administrator rights"));
        assert!(record.summary.contains("1 could not be ended"));
        assert!(!record.summary.contains("secret"));
    }

    #[test]
    fn nothing_ended_is_recorded_as_failed_and_a_missing_app_is_not_running() {
        let s = setup();
        s.processes.add(200, "vendor.exe", MIB);
        s.processes
            .fail_terminate(200, PortError::AccessDenied("no".into()));
        let report = s
            .monitor
            .end_group("vendor.exe", Source::Manual)
            .expect("end");
        assert_eq!(report.ended(), 0);
        assert_eq!(s.log.records()[0].outcome, ActivityOutcome::Failed);
        assert_eq!(
            s.monitor.end_group("gone.exe", Source::Manual),
            Err(EndError::NotRunning("gone.exe".into()))
        );
    }

    #[test]
    fn kernel_ids_are_never_ended_and_a_failing_log_is_reported_not_fatal() {
        let s = setup();
        s.processes.add(4, "odd.exe", MIB);
        s.processes.add(0, "odd.exe", MIB);
        assert!(matches!(
            s.monitor.end_group("odd.exe", Source::Manual),
            Err(EndError::Protected { .. })
        ));
        s.processes.add(400, "app.exe", MIB);
        s.log
            .fail_records_with(Some(PortError::Other("disk".into())));
        let report = s.monitor.end_group("app.exe", Source::Manual).expect("end");
        assert_eq!(report.ended(), 1);
        assert!(report.activity_write_failed);
    }
}
