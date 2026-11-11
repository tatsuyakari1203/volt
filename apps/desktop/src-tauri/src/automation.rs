//! Automation at the IPC boundary: the transfer types, the two commands behind the Automation
//! card in Settings, one tick of the timer and the sentences an automatic run leaves behind.
//!
//! The rules live in `volt_core::automation`; this module translates them into the shell's own
//! types (see [`crate::api`] for why the shell owns its transfer types). The timer that calls
//! [`tick_of`] is in [`crate::agent`].

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;
use volt_core::automation::{COOLDOWN_MINUTES, FREE_BELOW_MB, STANDBY_ABOVE_MB, is_eligible};
use volt_core::boost::available;
use volt_core::{
    AutomationConfig, AutomationStatus, BoostConfig, CleaningOutcome, Frequency, MemoryRule,
    PauseReason, Phrase, Schedule, TickReport, Timestamp,
};

use crate::IpcError;
use crate::cleaner::count32;
use crate::commands::blocking;
use crate::language::Lang;
use crate::wiring::{AppCore, AppState};

/// The memory threshold rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRuleView {
    /// Whether the rule is on.
    pub enabled: bool,
    /// Clean only when standby memory is above this many MB.
    pub standby_above_mb: u32,
    /// And free memory is below this many MB.
    pub free_below_mb: u32,
    /// Minutes to wait after a clean.
    pub cooldown_minutes: u32,
}

/// How often the cleaning schedule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum FrequencyView {
    /// Every day.
    Daily,
    /// Once a week.
    Weekly,
}

/// The cleaning schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleView {
    /// Whether the schedule is on.
    pub enabled: bool,
    /// Daily or weekly.
    pub frequency: FrequencyView,
    /// For a weekly schedule: 0 is Monday, 6 is Sunday.
    pub weekday: u32,
    /// Local time of day, in minutes after midnight.
    pub minute_of_day: u32,
    /// The cleaning rules that run.
    pub rule_ids: Vec<String>,
}

/// Everything the user sets for automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationConfigView {
    /// Memory cleaning on thresholds.
    pub memory: MemoryRuleView,
    /// Cleaning on a schedule.
    pub cleaning: ScheduleView,
    /// Whether an automatic run shows a notification when no window is open.
    pub notify: bool,
}

/// Why an automatic run was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum PauseView {
    /// A fullscreen app or a presentation is active.
    Fullscreen,
    /// Battery saver is on.
    BatterySaver,
    /// The user cleaned by hand a moment ago.
    ManualRun,
}

/// A scheduled run that was skipped.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkippedRunView {
    /// When, in milliseconds since the Unix epoch.
    pub at: f64,
    /// Why.
    pub reason: PauseView,
}

/// What the user may enter, so the form never offers a value the core refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationBoundsView {
    /// Lowest and highest standby threshold, in MB.
    pub standby_above_mb: [u32; 2],
    /// Lowest and highest free memory threshold, in MB.
    pub free_below_mb: [u32; 2],
    /// Shortest and longest cooldown, in minutes.
    pub cooldown_minutes: [u32; 2],
}

/// The configuration and what happened last.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct AutomationStatusView {
    /// The saved configuration (everything off until the user turns it on).
    pub config: AutomationConfigView,
    /// What the user may enter.
    pub bounds: AutomationBoundsView,
    /// The last automatic memory clean, in milliseconds since the Unix epoch.
    pub last_memory_run: Option<f64>,
    /// The last scheduled clean.
    pub last_clean_run: Option<f64>,
    /// The last scheduled clean that was skipped.
    pub last_clean_skip: Option<SkippedRunView>,
    /// When the schedule runs next; the current time when a run is waiting.
    pub next_clean_run: Option<f64>,
    /// The pause that is on right now, if any.
    pub paused: Option<PauseView>,
}

/// Sent to every window after an automatic run, with the new status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, Event)]
pub struct AutomationUpdated(pub AutomationStatusView);

fn millis(at: Timestamp) -> f64 {
    // Display only: milliseconds since 1970 are far below 2^53.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a timestamp in milliseconds is far below 2^53"
    )]
    let value = at.as_unix_millis() as f64;
    value
}

impl PauseView {
    pub(crate) fn from_core(reason: PauseReason) -> Self {
        match reason {
            PauseReason::Fullscreen => Self::Fullscreen,
            PauseReason::BatterySaver => Self::BatterySaver,
            PauseReason::ManualRun => Self::ManualRun,
        }
    }
}

impl MemoryRuleView {
    pub(crate) fn from_core(rule: &MemoryRule) -> Self {
        Self {
            enabled: rule.enabled,
            standby_above_mb: rule.standby_above_mb,
            free_below_mb: rule.free_below_mb,
            cooldown_minutes: rule.cooldown_minutes,
        }
    }

    pub(crate) fn to_core(&self) -> MemoryRule {
        MemoryRule {
            enabled: self.enabled,
            standby_above_mb: self.standby_above_mb,
            free_below_mb: self.free_below_mb,
            cooldown_minutes: self.cooldown_minutes,
        }
    }
}

impl ScheduleView {
    pub(crate) fn from_core(schedule: &Schedule) -> Self {
        Self {
            enabled: schedule.enabled,
            frequency: match schedule.frequency {
                Frequency::Daily => FrequencyView::Daily,
                Frequency::Weekly => FrequencyView::Weekly,
            },
            weekday: u32::from(schedule.weekday),
            minute_of_day: u32::from(schedule.minute_of_day),
            rule_ids: schedule.rule_ids.clone(),
        }
    }

    /// The core form. A number that does not fit becomes the largest value, which the core's
    /// bounds then refuse.
    pub(crate) fn to_core(&self) -> Schedule {
        let narrow = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
        Schedule {
            enabled: self.enabled,
            frequency: match self.frequency {
                FrequencyView::Daily => Frequency::Daily,
                FrequencyView::Weekly => Frequency::Weekly,
            },
            weekday: u8::try_from(self.weekday).unwrap_or(u8::MAX),
            minute_of_day: narrow(self.minute_of_day),
            rule_ids: self.rule_ids.clone(),
        }
    }
}

impl AutomationConfigView {
    pub(crate) fn from_core(config: &AutomationConfig) -> Self {
        Self {
            memory: MemoryRuleView::from_core(&config.memory),
            cleaning: ScheduleView::from_core(&config.cleaning),
            notify: config.notify,
        }
    }

    fn to_core(&self) -> AutomationConfig {
        AutomationConfig {
            memory: self.memory.to_core(),
            cleaning: self.cleaning.to_core(),
            notify: self.notify,
        }
    }
}

fn bounds() -> AutomationBoundsView {
    let pair = |range: &std::ops::RangeInclusive<u32>| [*range.start(), *range.end()];
    AutomationBoundsView {
        standby_above_mb: pair(&STANDBY_ABOVE_MB),
        free_below_mb: pair(&FREE_BELOW_MB),
        cooldown_minutes: pair(&COOLDOWN_MINUTES),
    }
}

fn status_view(status: &AutomationStatus) -> AutomationStatusView {
    AutomationStatusView {
        config: AutomationConfigView::from_core(&status.config),
        bounds: bounds(),
        last_memory_run: status.state.last_memory_run.map(millis),
        last_clean_run: status.state.last_clean_run.map(millis),
        last_clean_skip: status.state.last_clean_skip.map(|skip| SkippedRunView {
            at: millis(skip.at),
            reason: PauseView::from_core(skip.reason),
        }),
        next_clean_run: status.next_cleaning_run.map(millis),
        paused: status.paused.map(PauseView::from_core),
    }
}

/// The saved configuration, what happened last and the pause that is on now.
pub fn automation_status_of(core: &AppCore) -> Result<AutomationStatusView, IpcError> {
    let automation = core.automation()?;
    Ok(status_view(&automation.status(core.clean_engine())))
}

/// Saves the configuration and returns the new status.
///
/// # Errors
/// `invalidInput` when a number is out of its bounds or the schedule names a cleaning rule that
/// cannot run on its own (unknown, or one that needs administrator rights, a chosen folder or a
/// choice of items).
pub fn set_automation_config_of(
    core: &AppCore,
    config: &AutomationConfigView,
) -> Result<AutomationStatusView, IpcError> {
    let automation = core.automation()?;
    let config = config.to_core();
    if let Some(engine) = core.clean_engine() {
        for id in &config.cleaning.rule_ids {
            match engine.rules().get(id) {
                Some(rule) if is_eligible(rule) => {}
                Some(rule) => {
                    return Err(IpcError::invalid(
                        &Phrase::new("err.automation.notAutomatic")
                            .text("name", rule.title.clone()),
                    ));
                }
                None => {
                    return Err(IpcError::not_found(
                        &Phrase::new("err.ruleNotInScan").text("id", id),
                    ));
                }
            }
        }
    }
    automation.save_config(&config)?;
    automation_status_of(core)
}

/// One look at the world for the timer: runs what the memory rule or the schedule says is due.
/// Holds the engine gate while it runs, so it never overlaps a tweak, a Boost or a manual clean.
/// Never runs anything while both features are off.
pub fn tick_of(core: &AppCore) -> Result<TickReport, IpcError> {
    let automation = core.automation()?;
    let memory = core.memory_cleaner()?;
    let boost = BoostConfig::load(core.settings.as_ref());
    let _guard = core.exclusive();
    let report = automation.tick(memory, core.clean_engine(), &boost);
    let history_lost = report
        .memory
        .as_ref()
        .is_some_and(|run| run.activity_write_failed)
        || matches!(
            &report.cleaning,
            Some(CleaningOutcome::Cleaned(cleaned)) if cleaned.activity_write_failures > 0
        );
    if history_lost {
        core.note_history_incomplete();
    }
    Ok(report)
}

/// Available memory before and after an automatic memory clean, when both were read.
fn memory_change(report: &TickReport) -> Option<(u64, u64)> {
    let run = report.memory.as_ref().filter(|run| run.any_purged())?;
    Some((available(run.before?), available(run.after?)))
}

fn moved_files(report: &TickReport) -> Option<u32> {
    match &report.cleaning {
        Some(CleaningOutcome::Cleaned(cleaned)) if cleaned.moved_files() > 0 => {
            Some(count32(cleaned.moved_files()))
        }
        _ => None,
    }
}

/// A short line for the tray tooltip after an automatic run, or `None` when nothing was changed.
#[must_use]
pub fn tooltip_line(report: &TickReport, lang: Lang) -> Option<String> {
    let phrase = if let Some((before, after)) = memory_change(report) {
        Phrase::new("automation.tooltip.memory")
            .bytes("before", before)
            .bytes("after", after)
    } else {
        moved_files(report)?;
        Phrase::new("automation.tooltip.clean")
    };
    Some(crate::phrase::write(&phrase, lang))
}

/// The text of the notification after an automatic run, or `None` when nothing was changed. It
/// says what ran; it never claims disk space was freed.
#[must_use]
pub fn notification_body(report: &TickReport, lang: Lang) -> Option<String> {
    let mut parts = Vec::new();
    if let Some((before, after)) = memory_change(report) {
        parts.push(
            Phrase::new("automation.notice.memory")
                .bytes("before", before)
                .bytes("after", after),
        );
    }
    if let Some(files) = moved_files(report) {
        parts.push(
            Phrase::new("automation.notice.clean")
                .phrase("files", Phrase::new("count.file").count("count", files)),
        );
    }
    if parts.is_empty() {
        return None;
    }
    Some(
        parts
            .iter()
            .map(|part| crate::phrase::write(part, lang))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Tauri command: the saved configuration, what happened last and the pause that is on now.
#[tauri::command]
#[specta::specta]
pub(crate) async fn automation_status(
    state: tauri::State<'_, AppState>,
) -> Result<AutomationStatusView, IpcError> {
    blocking(&state, automation_status_of).await
}

/// Tauri command: saves the configuration and returns the new status. The timer wakes to look at
/// it at once.
#[tauri::command]
#[specta::specta]
pub(crate) async fn set_automation_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    config: AutomationConfigView,
) -> Result<AutomationStatusView, IpcError> {
    let status = blocking(&state, move |core| set_automation_config_of(core, &config)).await?;
    crate::agent::wake(&app);
    Ok(status)
}
