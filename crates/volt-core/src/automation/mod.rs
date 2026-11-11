//! Automation: memory cleaning on thresholds and cleaning on a schedule, both off by default.
//!
//! Volt looks after the machine from the tray process, without a service or a scheduled task. A
//! coarse timer (one tick a minute, and none at all while both features are off) asks
//! [`Automation::tick`] what to do. The decisions are pure functions over the clock and a set of
//! [`Probes`], so everything here is tested with fakes and nothing waits on real time:
//!
//! - **Memory rule** ([`MemoryRule`]): when standby memory is above X and free memory is below Y,
//!   run the same unelevated memory clean as Boost (the operations Boost is set to, without the
//!   administrator ones, honouring the protected programs list), then stay quiet for a cooldown.
//! - **Cleaning schedule** ([`Schedule`]): daily or weekly at a local time, run a saved selection
//!   of cleaning rules. A run that was missed (Volt was not running) is made at the next start,
//!   and a period is never served twice. Only rules that need no administrator rights, no chosen
//!   folder and no item-by-item choice are ever run ([`is_eligible`]); items go to the Recycle
//!   Bin exactly as a manual clean does.
//! - **Pauses**: a fullscreen app or a presentation, battery saver, and the minutes after a manual
//!   run. A paused memory run is simply not made (it is judged again at the next tick); a paused
//!   scheduled run is skipped for that period and remembered as skipped, never queued up.
//!
//! Every automatic run is recorded in the activity log with [`Source::Automation`] by the engines
//! it uses, exactly like the manual one. Automation never reverts anything by itself.

use std::fmt;
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};

use crate::activity::{ActivityLog, error_class};
use crate::boost::BoostConfig;
use crate::clean::{CleanEngine, CleanReport, CleaningRule};
use crate::error::PortError;
use crate::journal::Source;
use crate::memory::{MEMORY_LABEL, MemoryCleanReport, MemoryCleaner};
use crate::ports::{Availability, MemoryArea, MemoryLists};
use crate::settings::{Settings, SettingsExt};
use crate::time::{Clock, Timestamp};

#[cfg(test)]
mod tests;

/// The settings key that holds the [`AutomationConfig`].
pub const CONFIG_KEY: &str = "automation.config";

/// The settings key that holds the [`AutomationState`].
pub const STATE_KEY: &str = "automation.state";

/// Seconds between two ticks of the timer while a feature is on.
pub const TICK_SECS: u64 = 60;

/// Nothing runs in the first minutes after Volt starts, so a missed run does not compete with
/// the start of the session.
pub const STARTUP_GRACE_MS: i64 = 3 * MINUTE_MS;

/// A scheduled run is skipped when the user cleaned by hand within this long before it.
pub const MANUAL_PAUSE_MS: i64 = 15 * MINUTE_MS;

/// What the user may enter: standby memory above this many MB triggers the rule.
pub const STANDBY_ABOVE_MB: RangeInclusive<u32> = 256..=262_144;

/// What the user may enter: free memory below this many MB triggers the rule.
pub const FREE_BELOW_MB: RangeInclusive<u32> = 128..=131_072;

/// What the user may enter: minutes between two automatic memory cleans.
pub const COOLDOWN_MINUTES: RangeInclusive<u32> = 5..=1440;

/// The most cleaning rules a schedule may hold.
pub const MAX_SCHEDULE_RULES: usize = 64;

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 86_400_000;
const MIB: u64 = 1 << 20;
const LAST_WEEKDAY: u8 = 6;
const LAST_MINUTE_OF_DAY: u16 = 1439;

/// How often a cleaning schedule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Frequency {
    /// Every day.
    #[default]
    Daily,
    /// Once a week, on [`Schedule::weekday`].
    Weekly,
}

/// The threshold rule for memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryRule {
    /// Whether the rule is on. Off by default.
    pub enabled: bool,
    /// Run only when standby memory is above this many MB.
    pub standby_above_mb: u32,
    /// And free memory is below this many MB.
    pub free_below_mb: u32,
    /// Minutes to wait after a run before another may start.
    pub cooldown_minutes: u32,
}

impl Default for MemoryRule {
    fn default() -> Self {
        Self {
            enabled: false,
            standby_above_mb: 4096,
            free_below_mb: 1024,
            cooldown_minutes: 30,
        }
    }
}

/// The cleaning schedule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    /// Whether the schedule is on. Off by default.
    pub enabled: bool,
    /// Daily or weekly.
    pub frequency: Frequency,
    /// For a weekly schedule: 0 is Monday, 6 is Sunday.
    pub weekday: u8,
    /// Local time of day, in minutes after midnight.
    pub minute_of_day: u16,
    /// The cleaning rules that run (a saved selection, in this order).
    pub rule_ids: Vec<String>,
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            frequency: Frequency::Daily,
            weekday: LAST_WEEKDAY,
            minute_of_day: 180,
            rule_ids: Vec::new(),
        }
    }
}

/// Everything the user sets for automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationConfig {
    /// Memory cleaning on thresholds.
    pub memory: MemoryRule,
    /// Cleaning on a schedule.
    pub cleaning: Schedule,
    /// Whether an automatic run shows a notification (when no window is open).
    pub notify: bool,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            memory: MemoryRule::default(),
            cleaning: Schedule::default(),
            notify: true,
        }
    }
}

impl AutomationConfig {
    /// Whether either feature is on.
    #[must_use]
    pub fn any_enabled(&self) -> bool {
        self.memory.enabled || self.cleaning.enabled
    }

    /// Checks what the user entered against the bounds.
    pub fn validate(&self) -> Result<(), PortError> {
        let invalid = |what: &str| Err(PortError::InvalidInput(what.to_owned()));
        if !STANDBY_ABOVE_MB.contains(&self.memory.standby_above_mb) {
            return invalid("the standby threshold is out of range");
        }
        if !FREE_BELOW_MB.contains(&self.memory.free_below_mb) {
            return invalid("the free memory threshold is out of range");
        }
        if !COOLDOWN_MINUTES.contains(&self.memory.cooldown_minutes) {
            return invalid("the cooldown is out of range");
        }
        if self.cleaning.weekday > LAST_WEEKDAY || self.cleaning.minute_of_day > LAST_MINUTE_OF_DAY
        {
            return invalid("the schedule day or time is out of range");
        }
        if self.cleaning.rule_ids.len() > MAX_SCHEDULE_RULES {
            return invalid("the schedule holds too many rules");
        }
        Ok(())
    }

    /// The same configuration with every number moved into its bounds.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        let clamp =
            |value: u32, range: &RangeInclusive<u32>| value.clamp(*range.start(), *range.end());
        self.memory.standby_above_mb = clamp(self.memory.standby_above_mb, &STANDBY_ABOVE_MB);
        self.memory.free_below_mb = clamp(self.memory.free_below_mb, &FREE_BELOW_MB);
        self.memory.cooldown_minutes = clamp(self.memory.cooldown_minutes, &COOLDOWN_MINUTES);
        self.cleaning.weekday = self.cleaning.weekday.min(LAST_WEEKDAY);
        self.cleaning.minute_of_day = self.cleaning.minute_of_day.min(LAST_MINUTE_OF_DAY);
        self.cleaning.rule_ids.truncate(MAX_SCHEDULE_RULES);
        self
    }

    /// The stored configuration with its numbers clamped, or the default (everything off) when
    /// none is stored or the stored value cannot be read.
    #[must_use]
    pub fn load(settings: &dyn Settings) -> Self {
        match settings.get_typed::<Self>(CONFIG_KEY) {
            Ok(Some(config)) => config.clamped(),
            Ok(None) => Self::default(),
            Err(error) => {
                log::warn!(
                    "the automation settings could not be read ({}); automation stays off",
                    error_class(&error)
                );
                Self::default()
            }
        }
    }
}

/// Why an automatic run was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// A fullscreen app or a presentation is active.
    Fullscreen,
    /// Battery saver is on.
    BatterySaver,
    /// The user ran a clean by hand a moment ago.
    ManualRun,
}

/// A scheduled run that was skipped, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedRun {
    /// When it was skipped.
    pub at: Timestamp,
    /// Why.
    pub reason: PauseReason,
}

/// What automation remembers between ticks and restarts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutomationState {
    /// The last automatic memory clean.
    pub last_memory_run: Option<Timestamp>,
    /// The last scheduled clean.
    pub last_clean_run: Option<Timestamp>,
    /// The last scheduled clean that was skipped.
    pub last_clean_skip: Option<SkippedRun>,
    /// When the schedule was last switched on or moved: occurrences before it do not count as
    /// missed.
    pub clean_armed_at: Option<Timestamp>,
}

impl AutomationState {
    fn load(settings: &dyn Settings) -> Self {
        settings
            .get_typed::<Self>(STATE_KEY)
            .unwrap_or_else(|error| {
                log::warn!(
                    "the automation state could not be read ({}); starting fresh",
                    error_class(&error)
                );
                None
            })
            .unwrap_or_default()
    }

    /// The newest of the times that settle the schedule's current period.
    fn clean_reference(&self) -> Option<Timestamp> {
        [
            self.last_clean_run,
            self.last_clean_skip.map(|skip| skip.at),
            self.clean_armed_at,
        ]
        .into_iter()
        .flatten()
        .max()
    }
}

/// Port: what the operating system says about the moment.
pub trait Environment: Send + Sync {
    /// Whether a fullscreen app or a presentation is active.
    fn fullscreen_app_active(&self) -> bool;
    /// Whether battery saver is on.
    fn battery_saver_on(&self) -> bool;
    /// Local time minus UTC, in minutes, now (daylight saving included).
    fn utc_offset_minutes(&self) -> i32;
}

/// What the pure decisions look at. Each call reads the world at that moment, so a decision reads
/// only what it needs.
pub trait Probes {
    /// Physical memory by list, or `None` when it cannot be read.
    fn memory_lists(&self) -> Option<MemoryLists>;
    /// Whether a fullscreen app or a presentation is active.
    fn fullscreen_app_active(&self) -> bool;
    /// Whether battery saver is on.
    fn battery_saver_on(&self) -> bool;
    /// Local time minus UTC, in minutes.
    fn utc_offset_minutes(&self) -> i32;
    /// When the user last cleaned by hand (memory or a cleaning rule).
    fn last_manual_run(&self) -> Option<Timestamp>;
}

/// What a decision says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing is due.
    Idle,
    /// Run now.
    Run,
    /// Something is due but a pause is on.
    Pause(PauseReason),
}

/// The first pause that applies: a recent manual run (within `manual_window_ms`), a fullscreen app
/// or a presentation, battery saver.
#[must_use]
pub fn current_pause(
    now: Timestamp,
    manual_window_ms: i64,
    probes: &dyn Probes,
) -> Option<PauseReason> {
    if let Some(last) = probes.last_manual_run()
        && now.as_unix_millis().saturating_sub(last.as_unix_millis()) < manual_window_ms
    {
        return Some(PauseReason::ManualRun);
    }
    if probes.fullscreen_app_active() {
        return Some(PauseReason::Fullscreen);
    }
    if probes.battery_saver_on() {
        return Some(PauseReason::BatterySaver);
    }
    None
}

fn minutes_ms(minutes: u32) -> i64 {
    i64::from(minutes) * MINUTE_MS
}

fn elapsed(now: Timestamp, since: Timestamp) -> i64 {
    now.as_unix_millis().saturating_sub(since.as_unix_millis())
}

/// Whether the memory rule says to clean now. `since_start_ms` is how long Volt has been running.
/// Reads memory only when the rule is on and out of its cooldown, and the pauses only when the
/// thresholds are met.
#[must_use]
pub fn memory_decision(
    rule: &MemoryRule,
    state: &AutomationState,
    now: Timestamp,
    since_start_ms: i64,
    probes: &dyn Probes,
) -> Decision {
    if !rule.enabled || since_start_ms < STARTUP_GRACE_MS {
        return Decision::Idle;
    }
    let cooldown = minutes_ms(rule.cooldown_minutes);
    if state
        .last_memory_run
        .is_some_and(|last| elapsed(now, last) < cooldown)
    {
        return Decision::Idle;
    }
    let Some(lists) = probes.memory_lists() else {
        return Decision::Idle;
    };
    let standby_high = lists.standby_bytes > u64::from(rule.standby_above_mb) * MIB;
    let free_low = lists.free_bytes < u64::from(rule.free_below_mb) * MIB;
    if !(standby_high && free_low) {
        return Decision::Idle;
    }
    // After a manual run the same cooldown applies as after an automatic one.
    match current_pause(now, cooldown, probes) {
        Some(reason) => Decision::Pause(reason),
        None => Decision::Run,
    }
}

/// The most recent scheduled moment at or before `now`, in local time `utc_offset_minutes` from
/// UTC.
#[must_use]
pub fn latest_due(now: Timestamp, utc_offset_minutes: i32, schedule: &Schedule) -> Timestamp {
    let offset = i64::from(utc_offset_minutes) * MINUTE_MS;
    let local = now.as_unix_millis().saturating_add(offset);
    let day = local.div_euclid(DAY_MS);
    let time_of_day = i64::from(schedule.minute_of_day) * MINUTE_MS;
    let (first_day, period) = match schedule.frequency {
        Frequency::Daily => (day, DAY_MS),
        Frequency::Weekly => {
            // 1970-01-01 was a Thursday; Monday is 0.
            let weekday = (day + 3).rem_euclid(7);
            let back = (weekday - i64::from(schedule.weekday)).rem_euclid(7);
            (day - back, 7 * DAY_MS)
        }
    };
    let mut due = first_day * DAY_MS + time_of_day;
    if due > local {
        due -= period;
    }
    Timestamp::from_unix_millis(due - offset)
}

/// Whether the cleaning schedule says to run now.
#[must_use]
pub fn cleaning_decision(
    schedule: &Schedule,
    state: &AutomationState,
    now: Timestamp,
    since_start_ms: i64,
    probes: &dyn Probes,
) -> Decision {
    if !schedule.enabled || schedule.rule_ids.is_empty() || since_start_ms < STARTUP_GRACE_MS {
        return Decision::Idle;
    }
    let Some(reference) = state.clean_reference() else {
        return Decision::Idle;
    };
    if latest_due(now, probes.utc_offset_minutes(), schedule) <= reference {
        return Decision::Idle;
    }
    match current_pause(now, MANUAL_PAUSE_MS, probes) {
        Some(reason) => Decision::Pause(reason),
        None => Decision::Run,
    }
}

/// When the schedule runs next: now when a run is due and waiting, otherwise the next occurrence.
/// `None` when it is off or empty.
#[must_use]
pub fn next_cleaning_run(
    schedule: &Schedule,
    state: &AutomationState,
    now: Timestamp,
    utc_offset_minutes: i32,
) -> Option<Timestamp> {
    if !schedule.enabled || schedule.rule_ids.is_empty() {
        return None;
    }
    let due = latest_due(now, utc_offset_minutes, schedule);
    if state
        .clean_reference()
        .is_some_and(|reference| due > reference)
    {
        return Some(now);
    }
    let period = match schedule.frequency {
        Frequency::Daily => DAY_MS,
        Frequency::Weekly => 7 * DAY_MS,
    };
    Some(Timestamp::from_unix_millis(due.as_unix_millis() + period))
}

/// Whether a cleaning rule may be run without the user: it needs no administrator rights, no
/// chosen folder and no item-by-item choice.
#[must_use]
pub fn is_eligible(rule: &CleaningRule) -> bool {
    !rule.requires_admin && !rule.pick && !rule.needs_chosen_folder()
}

/// How a scheduled clean ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleaningOutcome {
    /// The rules were cleaned.
    Cleaned(CleanReport),
    /// The rules found nothing to move.
    NothingToClean,
    /// A pause was on; this period was skipped.
    Skipped(PauseReason),
}

/// What one tick did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// The automatic memory clean, when one ran.
    pub memory: Option<MemoryCleanReport>,
    /// The scheduled clean, when one was due.
    pub cleaning: Option<CleaningOutcome>,
}

impl TickReport {
    /// Whether the tick changed anything worth telling the user about.
    #[must_use]
    pub fn worth_telling(&self) -> bool {
        self.memory
            .as_ref()
            .is_some_and(MemoryCleanReport::any_purged)
            || matches!(&self.cleaning, Some(CleaningOutcome::Cleaned(report)) if report.moved_files() > 0)
    }
}

/// What the Automation page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationStatus {
    /// The saved configuration.
    pub config: AutomationConfig,
    /// What happened last.
    pub state: AutomationState,
    /// When the schedule runs next.
    pub next_cleaning_run: Option<Timestamp>,
    /// The pause that is on right now, if any.
    pub paused: Option<PauseReason>,
}

struct LiveProbes<'a> {
    memory: Option<&'a MemoryCleaner>,
    env: &'a dyn Environment,
    activity: &'a dyn ActivityLog,
    clean_labels: Vec<&'a str>,
}

impl Probes for LiveProbes<'_> {
    fn memory_lists(&self) -> Option<MemoryLists> {
        self.memory?.lists().ok()
    }

    fn fullscreen_app_active(&self) -> bool {
        self.env.fullscreen_app_active()
    }

    fn battery_saver_on(&self) -> bool {
        self.env.battery_saver_on()
    }

    fn utc_offset_minutes(&self) -> i32 {
        self.env.utc_offset_minutes()
    }

    fn last_manual_run(&self) -> Option<Timestamp> {
        let records = self.activity.list(200, None).ok()?;
        records
            .into_iter()
            .find(|record| {
                record.source == Source::Manual
                    && record.label.as_deref().is_some_and(|label| {
                        label == MEMORY_LABEL || self.clean_labels.contains(&label)
                    })
            })
            .map(|record| record.timestamp)
    }
}

/// Runs the automation decisions and carries them out.
pub struct Automation {
    settings: Arc<dyn Settings>,
    activity: Arc<dyn ActivityLog>,
    clock: Arc<dyn Clock>,
    env: Arc<dyn Environment>,
    state: Mutex<AutomationState>,
    started_at: Timestamp,
}

impl fmt::Debug for Automation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Automation").finish_non_exhaustive()
    }
}

impl Automation {
    /// Automation over the given ports. Volt "starts" now: nothing runs in the first minutes.
    #[must_use]
    pub fn new(
        settings: Arc<dyn Settings>,
        activity: Arc<dyn ActivityLog>,
        clock: Arc<dyn Clock>,
        env: Arc<dyn Environment>,
    ) -> Self {
        let state = AutomationState::load(settings.as_ref());
        let started_at = clock.now();
        Self {
            settings,
            activity,
            clock,
            env,
            state: Mutex::new(state),
            started_at,
        }
    }

    fn state_guard(&self) -> MutexGuard<'_, AutomationState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The saved configuration (everything off when none is saved).
    #[must_use]
    pub fn config(&self) -> AutomationConfig {
        AutomationConfig::load(self.settings.as_ref())
    }

    /// What automation remembers.
    #[must_use]
    pub fn state(&self) -> AutomationState {
        *self.state_guard()
    }

    fn update_state(&self, change: impl FnOnce(&mut AutomationState)) {
        let mut state = self.state_guard();
        change(&mut state);
        // The copy in memory still stops a repeat when the write fails.
        if let Err(error) = self.settings.set_typed(STATE_KEY, &*state) {
            log::warn!(
                "the automation state could not be saved ({})",
                error_class(&error)
            );
        }
    }

    /// Saves `config` after checking its bounds. Switching the schedule on, or moving it, starts
    /// its first period now, so the moment already past today is not counted as missed.
    pub fn save_config(&self, config: &AutomationConfig) -> Result<(), PortError> {
        config.validate()?;
        let before = self.config();
        self.settings.set_typed(CONFIG_KEY, config)?;
        let (was, will) = (&before.cleaning, &config.cleaning);
        let moved = !was.enabled
            || was.frequency != will.frequency
            || was.weekday != will.weekday
            || was.minute_of_day != will.minute_of_day;
        if will.enabled && moved {
            let now = self.clock.now();
            self.update_state(|state| state.clean_armed_at = Some(now));
        }
        Ok(())
    }

    /// The configuration, what happened last, the next scheduled run and the pause that is on.
    #[must_use]
    pub fn status(&self, clean: Option<&CleanEngine>) -> AutomationStatus {
        let config = self.config();
        let state = self.state();
        let now = self.clock.now();
        let probes = self.probes(None, clean);
        AutomationStatus {
            next_cleaning_run: next_cleaning_run(
                &config.cleaning,
                &state,
                now,
                self.env.utc_offset_minutes(),
            ),
            paused: current_pause(now, MANUAL_PAUSE_MS, &probes),
            config,
            state,
        }
    }

    /// The pause that is on right now (a fullscreen app or a presentation, battery saver, or a
    /// clean by hand a moment ago), if any. Profiles use the same rule before an automatic
    /// switch.
    #[must_use]
    pub fn pause_now(&self, clean: Option<&CleanEngine>) -> Option<PauseReason> {
        current_pause(self.clock.now(), MANUAL_PAUSE_MS, &self.probes(None, clean))
    }

    fn probes<'a>(
        &'a self,
        memory: Option<&'a MemoryCleaner>,
        clean: Option<&'a CleanEngine>,
    ) -> LiveProbes<'a> {
        LiveProbes {
            memory,
            env: self.env.as_ref(),
            activity: self.activity.as_ref(),
            clean_labels: clean
                .map(|engine| {
                    engine
                        .rules()
                        .rules()
                        .iter()
                        .map(|r| r.id.as_str())
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// One look at the world: does the memory rule or the schedule say to run? If so, runs it.
    /// Does nothing (and reads nothing) while both features are off.
    #[must_use]
    pub fn tick(
        &self,
        memory: &MemoryCleaner,
        clean: Option<&CleanEngine>,
        boost: &BoostConfig,
    ) -> TickReport {
        let config = self.config();
        let mut report = TickReport::default();
        if !config.any_enabled() {
            return report;
        }
        let now = self.clock.now();
        let since_start = elapsed(now, self.started_at);
        let state = self.state();
        let probes = self.probes(Some(memory), clean);

        let areas = runnable_areas(memory, boost);
        if !areas.is_empty()
            && memory_decision(&config.memory, &state, now, since_start, &probes) == Decision::Run
        {
            report.memory = Some(memory.run(&areas, Source::Automation));
            // Set even when the purge failed, so a failure is not retried every minute.
            self.update_state(|state| state.last_memory_run = Some(now));
        }

        if config.cleaning.enabled && state.clean_armed_at.is_none() {
            self.update_state(|state| state.clean_armed_at = Some(now));
        }
        report.cleaning =
            match cleaning_decision(&config.cleaning, &state, now, since_start, &probes) {
                Decision::Idle => None,
                Decision::Pause(reason) => {
                    self.update_state(|state| {
                        state.last_clean_skip = Some(SkippedRun { at: now, reason });
                    });
                    Some(CleaningOutcome::Skipped(reason))
                }
                Decision::Run => {
                    let outcome = clean_selection(clean, &config.cleaning);
                    self.update_state(|state| state.last_clean_run = Some(now));
                    Some(outcome)
                }
            };
        report
    }
}

/// The memory operations Boost is set to that can run without administrator rights.
fn runnable_areas(memory: &MemoryCleaner, boost: &BoostConfig) -> Vec<MemoryArea> {
    memory
        .availability()
        .into_iter()
        .filter(|(area, availability)| {
            boost.areas.contains(area) && *availability == Availability::Available
        })
        .map(|(area, _)| area)
        .collect()
}

/// Cleans the schedule's rules that may run on their own, into the Recycle Bin.
fn clean_selection(clean: Option<&CleanEngine>, schedule: &Schedule) -> CleaningOutcome {
    let Some(engine) = clean else {
        return CleaningOutcome::NothingToClean;
    };
    let mut ids: Vec<&str> = Vec::new();
    for id in &schedule.rule_ids {
        let eligible = engine.rules().get(id).is_some_and(is_eligible);
        if eligible && !ids.contains(&id.as_str()) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return CleaningOutcome::NothingToClean;
    }
    let plan = match engine.scan(&ids) {
        Ok(plan) => plan,
        Err(error) => {
            log::warn!("the scheduled clean could not scan: {error}");
            return CleaningOutcome::NothingToClean;
        }
    };
    if plan.is_empty() {
        return CleaningOutcome::NothingToClean;
    }
    CleaningOutcome::Cleaned(engine.clean(&plan.confirm_rules(&ids), Source::Automation))
}
