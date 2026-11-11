//! The health report at the IPC boundary: the transfer types and the one command that returns it.
//!
//! The rules live in `volt_core::health`; this module only reads the real machine, hands the
//! result to them and translates it into the shell's own types (see [`crate::api`] for why the
//! shell owns its transfer types).
//!
//! **Computed on demand.** The command reads the system when Home opens or the user asks to
//! check again, never on the metrics tick: memory pressure changes second by second, and a
//! score that moved while the user read it would look like a fault. The live cards show the
//! moving numbers; the score is a statement about a moment, and says when that moment was.
//!
//! **Independent of the sampler.** The command needs only the registry and one read of the
//! machine, so it works when the metrics sampler has not produced its first sample. The read is
//! a [`WinMetrics`] without a GPU query: three cheap Win32 calls that leave nothing open. The
//! startup load is the one input that comes from the core (the startup manager lists what is
//! enabled); when the engine could not start it is simply unchecked.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use volt_core::health;
use volt_core::{
    Clock, Deduction, FixAction, HealthCheck, HealthLevel, HealthReport, MetricsSource, Registry,
    StartupLoad, SystemClock,
};
use volt_win::{WinMetrics, WinRegistry};

use crate::IpcError;
use crate::phrase::PhraseView;
use crate::wiring::AppState;

/// The mount of the system disk when `SystemDrive` is not set: Windows is on `C:` unless the
/// environment says otherwise.
const DEFAULT_SYSTEM_MOUNT: &str = "C:\\";

/// A grade of the score, for the headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum HealthLevelView {
    /// The PC is in good shape.
    Good,
    /// A few things deserve attention.
    Fair,
    /// Several things need attention.
    Poor,
    /// No check could run, so there is no grade and the score means nothing.
    Unknown,
}

impl From<HealthLevel> for HealthLevelView {
    fn from(level: HealthLevel) -> Self {
        match level {
            HealthLevel::Good => Self::Good,
            HealthLevel::Fair => Self::Fair,
            HealthLevel::Poor => Self::Poor,
            // `HealthLevel` is non-exhaustive: a level added later is not shown as a grade.
            _ => Self::Unknown,
        }
    }
}

/// What the user can do about a deduction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FixActionView {
    /// Open the Cleaner.
    OpenCleaner,
    /// Open Memory.
    OpenMemory,
    /// Open Startup.
    OpenStartup,
    /// Restart Windows. A sentence for the user: Volt has no button that restarts the PC.
    RestartWindows,
}

impl FixActionView {
    /// `None` for a fix this build of the UI contract does not know (the core enum is
    /// non-exhaustive); the deduction is then listed without a button.
    fn from_core(fix: FixAction) -> Option<Self> {
        match fix {
            FixAction::OpenCleaner => Some(Self::OpenCleaner),
            FixAction::OpenMemory => Some(Self::OpenMemory),
            FixAction::OpenStartup => Some(Self::OpenStartup),
            FixAction::RestartWindows => Some(Self::RestartWindows),
            _ => None,
        }
    }
}

/// One input of the score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum HealthCheckView {
    /// Free space on the system disk.
    SystemDisk,
    /// Memory pressure.
    Memory,
    /// Whether Windows waits for a restart.
    PendingRestart,
    /// Load of the apps that start with Windows.
    Startup,
}

impl HealthCheckView {
    fn from_core(check: HealthCheck) -> Option<Self> {
        match check {
            HealthCheck::SystemDisk => Some(Self::SystemDisk),
            HealthCheck::Memory => Some(Self::Memory),
            HealthCheck::PendingRestart => Some(Self::PendingRestart),
            HealthCheck::Startup => Some(Self::Startup),
            _ => None,
        }
    }
}

/// One reason the score is below 100.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DeductionView {
    /// Stable id, such as `disk.low`.
    pub id: String,
    /// Points taken off the score.
    pub points: u8,
    /// A short name for the problem.
    pub title: String,
    /// What was found and why it matters, in plain words, in English: the fallback of
    /// `reason_phrase`.
    pub reason: String,
    /// The reason as a code (`health.disk`, `health.memory`, `health.restart`,
    /// `health.startup`) and the figures it is built from, for the UI to write in its own
    /// language.
    pub reason_phrase: PhraseView,
    /// What to do about it. `null` only for a fix the UI does not know yet.
    pub fix: Option<FixActionView>,
}

impl From<&Deduction> for DeductionView {
    fn from(deduction: &Deduction) -> Self {
        Self {
            id: deduction.id.to_owned(),
            points: deduction.points,
            title: deduction.title.to_owned(),
            reason: deduction.reason.clone(),
            reason_phrase: PhraseView::from(&deduction.reason_phrase),
            fix: FixActionView::from_core(deduction.fix),
        }
    }
}

/// The result of one health check, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct HealthView {
    /// 0 to 100: exactly 100 minus the points of `deductions`.
    pub score: u8,
    /// A grade of the score.
    pub level: HealthLevelView,
    /// Why the score is below 100. Empty when nothing was found.
    pub deductions: Vec<DeductionView>,
    /// Checks that could not run: their absence from `deductions` is not good news.
    pub unchecked: Vec<HealthCheckView>,
    /// The system disk the free-space rule looked at, such as `C:\`.
    pub system_mount: String,
    /// When the system was read, ISO 8601 in UTC.
    pub checked_at: String,
}

impl HealthView {
    fn new(report: &HealthReport, system_mount: &str, checked_at: String) -> Self {
        Self {
            score: report.score(),
            level: report.level().into(),
            deductions: report
                .deductions()
                .iter()
                .map(DeductionView::from)
                .collect(),
            unchecked: report
                .unchecked()
                .iter()
                .copied()
                .filter_map(HealthCheckView::from_core)
                .collect(),
            system_mount: system_mount.to_owned(),
            checked_at,
        }
    }
}

/// The mount of the system disk from the value of the `SystemDrive` environment variable
/// (`C:`), as the `C:\` the metrics list uses. Anything that is not a drive letter and a colon
/// falls back to `C:\`.
#[must_use]
pub(crate) fn system_mount(system_drive: Option<&str>) -> String {
    let drive = system_drive.map(|value| value.trim().trim_end_matches('\\'));
    match drive {
        Some(value)
            if value.len() == 2
                && value.ends_with(':')
                && value
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic()) =>
        {
            format!("{}\\", value.to_ascii_uppercase())
        }
        _ => DEFAULT_SYSTEM_MOUNT.to_owned(),
    }
}

/// Reads the system through the given ports and builds the view. Pure apart from the reads.
pub(crate) fn collect_report(
    registry: &dyn Registry,
    metrics: &dyn MetricsSource,
    system_mount: &str,
    startup_load: Option<StartupLoad>,
    clock: &dyn Clock,
) -> HealthView {
    let report = health::inspect(registry, metrics, system_mount, startup_load);
    log::debug!(
        "health check: score {}, {} deduction(s), {} unchecked",
        report.score(),
        report.deductions().len(),
        report.unchecked().len()
    );
    HealthView::new(&report, system_mount, clock.now().to_iso8601())
}

/// The startup load from the startup manager (a read-only listing of entries, without the
/// scheduled tasks the count does not use), `None` when the core or the
/// manager is missing or a source could not be read.
fn startup_load_of(state: &AppState) -> Option<StartupLoad> {
    let core = state.core().ok()?;
    let manager = core.startup_apps().ok()?;
    StartupLoad::from_listing(&manager.list_entries())
}

/// Reads the real machine: the real registry, one read of CPU time, memory and disks, and the
/// system disk named by the environment.
fn collect_real_report(startup_load: Option<StartupLoad>) -> HealthView {
    let mount = system_mount(std::env::var("SystemDrive").ok().as_deref());
    collect_report(
        &WinRegistry::new(),
        &WinMetrics::without_gpu(),
        &mount,
        startup_load,
        &SystemClock,
    )
}

/// Tauri command: checks the machine now and returns the health score with every deduction.
///
/// Read-only. It runs on the blocking pool, never on the main thread. A check that cannot run is
/// reported in `unchecked`, not as an error: the command fails only when its background task
/// does.
#[tauri::command]
#[specta::specta]
pub(crate) async fn health_report(state: State<'_, AppState>) -> Result<HealthView, IpcError> {
    let startup_load = startup_load_of(&state);
    tauri::async_runtime::spawn_blocking(move || collect_real_report(startup_load))
        .await
        .map_err(|error| {
            IpcError::other(
                &volt_core::Phrase::new("err.backgroundTask").text("detail", error.to_string()),
            )
        })
}

#[cfg(test)]
mod tests {
    use volt_core::fakes::FakeRegistry;
    use volt_core::metrics::fake::{FakeMetrics, typical_sample};
    use volt_core::{
        DiskReading, MemoryStatus, PortError, RawSample, RegistryHive, RegistryView, Timestamp,
    };

    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(0)
        }
    }

    fn sample(used_percent_tenths: u64, c_free: u64) -> RawSample {
        let mut raw = typical_sample();
        raw.memory = MemoryStatus {
            total_physical_bytes: 1000 * GIB,
            available_physical_bytes: (1000 - used_percent_tenths) * GIB,
        };
        raw.disks = vec![DiskReading {
            mount: "C:\\".into(),
            total_bytes: 200 * GIB,
            free_bytes: c_free,
        }];
        raw
    }

    const QUIET: Option<StartupLoad> = Some(StartupLoad { enabled_apps: 2 });

    #[test]
    fn the_view_carries_the_score_the_deductions_and_the_time() {
        let registry = FakeRegistry::new();
        registry.add_key(
            RegistryHive::LocalMachine,
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired",
            RegistryView::Native,
        );
        // 85 percent of memory in use (12 points), 15 GB free of 200 GB (20 points), restart (10).
        let metrics = FakeMetrics::constant(sample(850, 15 * GIB));
        let view = collect_report(&registry, &metrics, "C:\\", QUIET, &FixedClock);

        assert_eq!(view.score, 100 - 12 - 20 - 10);
        assert_eq!(view.level, HealthLevelView::Poor);
        let ids: Vec<&str> = view.deductions.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["disk.low", "memory.high", "restart.pending"]);
        let points: u8 = view.deductions.iter().map(|d| d.points).sum();
        assert_eq!(view.score, 100 - points);
        assert_eq!(view.deductions[0].fix, Some(FixActionView::OpenCleaner));
        assert_eq!(view.deductions[1].fix, Some(FixActionView::OpenMemory));
        assert_eq!(view.deductions[2].fix, Some(FixActionView::RestartWindows));
        assert!(view.unchecked.is_empty());
        assert_eq!(view.system_mount, "C:\\");
        assert_eq!(view.checked_at, "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn the_startup_load_joins_the_score_and_a_missing_one_is_unchecked() {
        let metrics = FakeMetrics::constant(sample(400, 150 * GIB));
        let busy = Some(StartupLoad { enabled_apps: 16 });
        let view = collect_report(&FakeRegistry::new(), &metrics, "C:\\", busy, &FixedClock);
        assert_eq!(view.score, 100 - 15);
        assert_eq!(view.deductions[0].id, "startup.many");
        assert_eq!(view.deductions[0].fix, Some(FixActionView::OpenStartup));

        let view = collect_report(&FakeRegistry::new(), &metrics, "C:\\", None, &FixedClock);
        assert_eq!(view.unchecked, [HealthCheckView::Startup]);
        assert_eq!(view.score, 100);
    }

    #[test]
    fn a_healthy_machine_is_a_hundred_with_no_deductions() {
        let metrics = FakeMetrics::constant(sample(400, 150 * GIB));
        let view = collect_report(&FakeRegistry::new(), &metrics, "C:\\", QUIET, &FixedClock);
        assert_eq!(view.score, 100);
        assert_eq!(view.level, HealthLevelView::Good);
        assert!(view.deductions.is_empty());
    }

    #[test]
    fn checks_that_could_not_run_are_reported_not_hidden() {
        let registry = FakeRegistry::new();
        registry.fail_key_exists(
            RegistryHive::LocalMachine,
            "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending",
            RegistryView::Native,
            PortError::AccessDenied("hklm".into()),
        );
        let metrics = FakeMetrics::scripted(vec![Err(PortError::Other("no counters".into()))]);
        let view = collect_report(&registry, &metrics, "C:\\", None, &FixedClock);
        assert_eq!(view.level, HealthLevelView::Unknown);
        assert_eq!(
            view.unchecked,
            [
                HealthCheckView::SystemDisk,
                HealthCheckView::Memory,
                HealthCheckView::PendingRestart,
                HealthCheckView::Startup
            ]
        );
        assert!(view.deductions.is_empty());
    }

    #[test]
    fn the_view_serialises_in_camel_case() {
        let metrics = FakeMetrics::constant(sample(950, 3 * GIB));
        let view = collect_report(&FakeRegistry::new(), &metrics, "C:\\", QUIET, &FixedClock);
        let json = serde_json::to_value(&view).unwrap_or_default();
        assert_eq!(json["score"], 100 - 30 - 25);
        assert_eq!(json["level"], "poor");
        assert_eq!(json["deductions"][0]["id"], "disk.critical");
        assert_eq!(json["deductions"][0]["fix"], "openCleaner");
        assert_eq!(json["deductions"][1]["fix"], "openMemory");
        assert_eq!(json["systemMount"], "C:\\");
        assert!(json["checkedAt"].is_string());
    }

    #[test]
    fn the_real_machine_can_be_checked_read_only() {
        // Reads the real markers and one sample of the machine; changes nothing. It asserts the
        // shape of the answer, not the state of whichever machine runs the test.
        let view = collect_real_report(Some(StartupLoad { enabled_apps: 0 }));
        println!("{view:#?}");
        let points: u8 = view.deductions.iter().map(|d| d.points).sum();
        assert_eq!(view.score, 100 - points);
        assert!(view.score <= 100);
        assert!(view.checked_at.ends_with('Z'));
        assert!(view.system_mount.ends_with(":\\"));
        // On Windows the disk and memory can always be read, and the markers are readable by
        // any user.
        assert!(view.unchecked.is_empty(), "{:?}", view.unchecked);
        assert_ne!(view.level, HealthLevelView::Unknown);
    }

    #[test]
    fn the_system_mount_comes_from_the_environment_or_falls_back_to_c() {
        assert_eq!(system_mount(Some("C:")), "C:\\");
        assert_eq!(system_mount(Some("d:")), "D:\\");
        assert_eq!(system_mount(Some("E:\\")), "E:\\");
        assert_eq!(system_mount(Some(" c: ")), "C:\\");
        for odd in [
            None,
            Some(""),
            Some("C"),
            Some("CC:"),
            Some("1:"),
            Some("\\\\server"),
        ] {
            assert_eq!(system_mount(odd), "C:\\", "{odd:?}");
        }
    }
}
