//! Process priority, CPU affinity, Efficiency mode and the saved rules at the IPC boundary.
//!
//! The rules (which classes are offered, what may be changed, how a change is journaled and
//! undone, how a rule matches) live in `volt_core::tuning`; this module translates them into the
//! shell's own types and runs the work off the main thread. It also holds the one hook that
//! reapplies the saved rules: [`RulesSource`], a decorator of the live-metrics source that calls
//! the tuner every few reads. The metrics sampler is the thread the shell already runs while the
//! main window exists, so rules cost no thread and no timer of their own; with no rule saved a
//! tick returns without reading anything, and while the app sits in the tray nothing runs at all.
//!
//! Cores cross the boundary as a list of core numbers, never as a bit mask, so the UI cannot
//! build a mask that a 64-bit integer would not carry, and an empty list is an error the core
//! reports.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{AppHandle, Manager as _, Runtime};
use volt_core::{
    ExecError, Inspection, MetricsSource, PortError, PriorityClass, ProcessTuner, RawSample,
    RuleStatus, Source, TuneError, TuneReport, TuneRequest,
};

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// How many metrics reads (one a second) lie between two passes over the rules: five seconds, so
/// a rule reaches a process that started within about five seconds, and the process list is read
/// at most once every five seconds, and only when a rule is saved.
pub(crate) const RULES_EVERY_READS: u32 = 5;

/// The priority classes Volt offers. Mirrors [`volt_core::PriorityClass`]; there is no Realtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PriorityView {
    /// Low (Windows calls it Idle).
    Idle,
    /// Below normal.
    BelowNormal,
    /// Normal.
    Normal,
    /// Above normal.
    AboveNormal,
    /// High.
    High,
}

impl From<PriorityClass> for PriorityView {
    fn from(class: PriorityClass) -> Self {
        match class {
            PriorityClass::Idle => Self::Idle,
            PriorityClass::BelowNormal => Self::BelowNormal,
            PriorityClass::Normal => Self::Normal,
            PriorityClass::AboveNormal => Self::AboveNormal,
            PriorityClass::High => Self::High,
        }
    }
}

impl From<PriorityView> for PriorityClass {
    fn from(view: PriorityView) -> Self {
        match view {
            PriorityView::Idle => Self::Idle,
            PriorityView::BelowNormal => Self::BelowNormal,
            PriorityView::Normal => Self::Normal,
            PriorityView::AboveNormal => Self::AboveNormal,
            PriorityView::High => Self::High,
        }
    }
}

/// What the user chose for an app. A field left out (`null`) is left alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TuneRequestView {
    /// The priority class. Efficiency mode on overrides it with Low.
    pub priority: Option<PriorityView>,
    /// The cores to run on, by number (0 is the first). Must not be empty.
    pub cores: Option<Vec<u32>>,
    /// Efficiency mode: on is power throttling together with Low priority; off gives the process
    /// back to the Windows default.
    pub efficiency: Option<bool>,
}

/// The core numbers set in `mask`, lowest first.
fn cores_of(mask: u64) -> Vec<u32> {
    (0..64).filter(|core| mask & (1 << core) != 0).collect()
}

fn mask_of(cores: &[u32]) -> Result<u64, IpcError> {
    cores.iter().try_fold(0_u64, |mask, core| {
        if *core < 64 {
            Ok(mask | (1 << core))
        } else {
            Err(IpcError::InvalidInput {
                message: format!("core {core} is out of range"),
            })
        }
    })
}

impl TryFrom<&TuneRequestView> for TuneRequest {
    type Error = IpcError;

    fn try_from(view: &TuneRequestView) -> Result<Self, IpcError> {
        Ok(Self {
            priority: view.priority.map(PriorityClass::from),
            affinity_mask: view.cores.as_deref().map(mask_of).transpose()?,
            efficiency: view.efficiency,
        })
    }
}

impl From<&TuneRequest> for TuneRequestView {
    fn from(request: &TuneRequest) -> Self {
        Self {
            priority: request.priority.map(PriorityView::from),
            cores: request.affinity_mask.map(cores_of),
            efficiency: request.efficiency,
        }
    }
}

/// What an app's processes hold now and how many of them can be changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct InspectionView {
    /// The executable name.
    pub name: String,
    /// The executable path, when known.
    pub path: Option<String>,
    /// How many processes the app has that Volt may offer to change.
    pub processes: u32,
    /// How many of them can be changed now.
    pub changeable: u32,
    /// How many Windows will not let Volt change without administrator rights.
    pub needs_administrator: u32,
    /// How many could not be read for another reason.
    pub unsupported: u32,
    /// The class every changeable process has, or `null` when they differ (or none can be
    /// changed).
    pub priority: Option<PriorityView>,
    /// The cores every changeable process runs on, or `null` when they differ.
    pub cores: Option<Vec<u32>>,
    /// The cores that can be chosen: the ones every changeable process may use. Empty when none
    /// can be changed.
    pub available_cores: Vec<u32>,
    /// Whether Efficiency mode is on for every changeable process, or `null` when they differ.
    pub efficiency: Option<bool>,
}

impl From<&Inspection> for InspectionView {
    fn from(inspection: &Inspection) -> Self {
        Self {
            name: inspection.name.clone(),
            path: inspection.path.clone(),
            processes: inspection.processes,
            changeable: inspection.changeable,
            needs_administrator: inspection.needs_administrator,
            unsupported: inspection.unsupported,
            priority: inspection.priority.map(PriorityView::from),
            cores: inspection.affinity_mask.map(cores_of),
            available_cores: cores_of(inspection.system_affinity_mask),
            efficiency: inspection.efficiency,
        }
    }
}

/// What a change did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct TuneResult {
    /// The executable name.
    pub name: String,
    /// Processes whose settings were changed.
    pub changed: u32,
    /// Processes that already had what was asked for.
    pub already_set: u32,
    /// Processes Windows would not let Volt change (administrator rights).
    pub needs_administrator: u32,
    /// Processes that had ended or could not be read.
    pub skipped: u32,
    /// Whether the change was written to the activity history. It is in the undo journal either
    /// way.
    pub recorded: bool,
    /// Whether the settings were saved as a rule for the app.
    pub rule_saved: bool,
}

impl TuneResult {
    fn of(report: &TuneReport, rule_saved: bool) -> Self {
        Self {
            name: report.name.clone(),
            changed: report.changed,
            already_set: report.already_set,
            needs_administrator: report.needs_administrator,
            skipped: report.skipped,
            recorded: !report.activity_write_failed,
            rule_saved,
        }
    }
}

/// Why a saved rule could not be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum RuleProblemView {
    /// Windows lets only an administrator change the app.
    NeedsAdministrator,
    /// It failed for another reason.
    Failed,
}

/// A saved rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleView {
    /// Identity of the rule (pass it to `removeProcessRule`): the executable path in lower case,
    /// or the name when the path was not known.
    pub id: String,
    /// The executable name.
    pub name: String,
    /// The executable path, when it was known.
    pub path: Option<String>,
    /// What it applies.
    pub request: TuneRequestView,
    /// Why it was last not applied, until it works again.
    pub problem: Option<RuleProblemView>,
    /// How many processes of the app the rule could not change since Volt started (Windows
    /// refused them) although it changed others.
    pub left_alone: u32,
}

impl From<&RuleStatus> for RuleView {
    fn from(status: &RuleStatus) -> Self {
        Self {
            id: status.rule.key.clone(),
            name: status.rule.name.clone(),
            path: status.rule.path.clone(),
            request: (&status.rule.request).into(),
            problem: status.problem.map(|class| {
                if class == "access denied" {
                    RuleProblemView::NeedsAdministrator
                } else {
                    RuleProblemView::Failed
                }
            }),
            left_alone: status.left_alone,
        }
    }
}

impl From<TuneError> for IpcError {
    fn from(error: TuneError) -> Self {
        let message = error.to_string();
        match error {
            TuneError::NotRunning(_) => Self::NotFound { message },
            TuneError::Protected { .. }
            | TuneError::Nothing
            | TuneError::EmptyAffinity
            | TuneError::UnknownCore
            | TuneError::TooManyRules => Self::InvalidInput { message },
            TuneError::List(port)
            | TuneError::Rules(port)
            | TuneError::Read(port)
            | TuneError::Exec(
                ExecError::Detect(port)
                | ExecError::Journal(port)
                | ExecError::MarkApplied(port)
                | ExecError::Apply { error: port, .. },
            ) => port.into(),
            TuneError::Exec(_) => Self::Other { message },
        }
    }
}

fn tuner(core: &AppCore) -> Result<&ProcessTuner, IpcError> {
    core.process_tuner()
}

/// What the processes of the app `id` hold now. Reads only: this is the dry run of
/// [`tune_app_of`]. Protected apps are refused (`invalidInput`), an app that is not running is
/// `notFound`.
pub fn inspect_app_of(core: &AppCore, id: &str) -> Result<InspectionView, IpcError> {
    Ok((&tuner(core)?.inspect(id)?).into())
}

/// Changes the settings of the app `id`, journals it so it can be undone from the Undo page, and,
/// when `save_rule` is set and something could be changed, saves the same settings as the rule
/// of the app. Protected apps, an empty set of cores and cores the system does not have are
/// refused before anything is changed.
pub fn tune_app_of(
    core: &AppCore,
    id: &str,
    request: &TuneRequestView,
    save_rule: bool,
) -> Result<TuneResult, IpcError> {
    let tuner = tuner(core)?;
    let request = TuneRequest::try_from(request)?;
    let report = tuner.tune(id, &request, Source::Manual)?;
    // A rule for an app nothing of which can be changed would only ever fail.
    let changeable = report.changed + report.already_set > 0;
    let rule_saved = if save_rule && changeable {
        tuner.save_rule(id, &request)?;
        true
    } else {
        false
    };
    Ok(TuneResult::of(&report, rule_saved))
}

/// The saved rules, sorted by name.
pub fn process_rules_of(core: &AppCore) -> Result<Vec<RuleView>, IpcError> {
    Ok(tuner(core)?.rules()?.iter().map(RuleView::from).collect())
}

/// Removes the rule `id` and returns the rules that are left. Processes that run keep what the
/// rule gave them until they end or the change is undone.
pub fn remove_process_rule_of(core: &AppCore, id: &str) -> Result<Vec<RuleView>, IpcError> {
    tuner(core)?.remove_rule(id)?;
    process_rules_of(core)
}

/// Tauri command: what the processes of an app hold now (priority, cores, Efficiency mode) and
/// how many of them can be changed without administrator rights. Reads only.
#[tauri::command]
#[specta::specta]
pub(crate) async fn inspect_app(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<InspectionView, IpcError> {
    blocking(&state, move |core| inspect_app_of(core, &id)).await
}

/// Tauri command: changes the priority, cores and Efficiency mode of an app, and optionally saves
/// them as a rule that is reapplied whenever the app starts. Recorded in the undo journal.
#[tauri::command]
#[specta::specta]
pub(crate) async fn tune_app(
    state: tauri::State<'_, AppState>,
    id: String,
    request: TuneRequestView,
    save_rule: bool,
) -> Result<TuneResult, IpcError> {
    blocking(&state, move |core| {
        tune_app_of(core, &id, &request, save_rule)
    })
    .await
}

/// Tauri command: the saved rules.
#[tauri::command]
#[specta::specta]
pub(crate) async fn list_process_rules(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<RuleView>, IpcError> {
    blocking(&state, process_rules_of).await
}

/// Tauri command: removes a saved rule. Resolves with the rules that are left.
#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_process_rule(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Vec<RuleView>, IpcError> {
    blocking(&state, move |core| remove_process_rule_of(core, &id)).await
}

// ---- reapplying the rules on the sampler's tick ---------------------------------------------

/// Runs one pass over the saved rules, if the core and the tuner exist.
pub(crate) fn apply_rules_on<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Ok(core) = state.core() else { return };
    if let Ok(tuner) = core.process_tuner() {
        let run = tuner.apply_rules();
        if run.changed > 0 || run.failed > 0 {
            log::info!(
                "process rules: {} process(es) changed, {} rule(s) failed",
                run.changed,
                run.failed
            );
        }
    }
}

/// A metrics source that also calls `tick` on every `every`-th read, the first one included.
///
/// This is how the rules ride on the sampler: the sampler already runs while the window exists
/// and builds its source when it starts, so no thread, timer or watcher is added, and nothing
/// runs while the app sits in the tray. A slow tick only delays the next metrics read, which the
/// sampler's deadline grid absorbs.
pub(crate) struct RulesSource {
    inner: Box<dyn MetricsSource>,
    tick: Arc<dyn Fn() + Send + Sync>,
    every: u32,
    reads: AtomicU32,
}

impl RulesSource {
    /// Wraps `inner`. `every` below 1 means every read.
    pub(crate) fn new(
        inner: Box<dyn MetricsSource>,
        tick: Arc<dyn Fn() + Send + Sync>,
        every: u32,
    ) -> Self {
        Self {
            inner,
            tick,
            every: every.max(1),
            reads: AtomicU32::new(0),
        }
    }
}

impl MetricsSource for RulesSource {
    fn read(&self) -> Result<RawSample, PortError> {
        let raw = self.inner.read();
        let nth = self.reads.fetch_add(1, Ordering::Relaxed);
        if nth % self.every == 0 {
            (self.tick)();
        }
        raw
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use volt_core::metrics::fake::{FakeMetrics, typical_sample};

    use super::*;

    #[test]
    fn cores_and_masks_round_trip_and_a_core_out_of_range_is_refused() {
        assert_eq!(cores_of(0b1011), [0, 1, 3]);
        assert_eq!(mask_of(&[0, 1, 3]).expect("mask"), 0b1011);
        assert_eq!(mask_of(&[]).expect("empty"), 0);
        assert!(mask_of(&[64]).is_err());
        assert_eq!(cores_of(1 << 63), [63]);
    }

    #[test]
    fn an_empty_list_of_cores_reaches_the_core_as_an_empty_affinity() {
        let view = TuneRequestView {
            priority: None,
            cores: Some(vec![]),
            efficiency: None,
        };
        let request = TuneRequest::try_from(&view).expect("request");
        assert_eq!(request.affinity_mask, Some(0));
        assert_eq!(request.validate(), Err(TuneError::EmptyAffinity));
    }

    struct Counting(Arc<FakeMetrics>);

    impl MetricsSource for Counting {
        fn read(&self) -> Result<RawSample, PortError> {
            self.0.read()
        }
    }

    #[test]
    fn the_tick_runs_on_the_first_read_and_then_every_nth() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let ticks = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&ticks);
        let source = RulesSource::new(
            Box::new(Counting(fake)),
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
            3,
        );
        for _ in 0..7 {
            let _ = source.read();
        }
        // Reads 0, 3 and 6.
        assert_eq!(ticks.load(Ordering::SeqCst), 3);
    }
}
