//! Automation tests. Nothing waits on real time: the clock is moved by hand and the world is a set
//! of fakes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::clean::fakes::FakeKnownFolders;
use crate::clean::{CleaningRules, Location};
use crate::exclusions::{EXCLUSIONS_SETTINGS_KEY, ExclusionList};
use crate::fakes::{
    FakeActivityLog, FakeEnvironment, FakeFileSystem, FakeMemory, FakeSettings, ManualClock,
};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 86_400_000;
/// Midnight UTC of Friday 2027-01-15.
const FRIDAY: i64 = 20_833 * DAY;
const NOW: i64 = FRIDAY + 8 * HOUR;
const GIB: u64 = 1 << 30;
const MIB_BYTES: u64 = 1 << 20;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(millis)
}

fn daily(hour: u16) -> Schedule {
    Schedule {
        enabled: true,
        minute_of_day: hour * 60,
        rule_ids: vec!["user-temp".to_owned()],
        ..Schedule::default()
    }
}

// ---- the pure decisions -----------------------------------------------------------------

struct World {
    lists: Option<MemoryLists>,
    fullscreen: bool,
    battery_saver: bool,
    offset: i32,
    manual: Option<Timestamp>,
}

impl World {
    fn quiet() -> Self {
        Self {
            lists: Some(MemoryLists::from_lists(
                16 * GIB,
                6 * GIB,
                0,
                512 * MIB_BYTES,
            )),
            fullscreen: false,
            battery_saver: false,
            offset: 0,
            manual: None,
        }
    }
}

impl Probes for World {
    fn memory_lists(&self) -> Option<MemoryLists> {
        self.lists
    }
    fn fullscreen_app_active(&self) -> bool {
        self.fullscreen
    }
    fn battery_saver_on(&self) -> bool {
        self.battery_saver
    }
    fn utc_offset_minutes(&self) -> i32 {
        self.offset
    }
    fn last_manual_run(&self) -> Option<Timestamp> {
        self.manual
    }
}

#[test]
fn the_latest_due_moment_follows_the_local_day_and_week() {
    let due = |now: i64, offset: i32, schedule: &Schedule| {
        latest_due(at(now), offset, schedule).as_unix_millis()
    };
    let three = daily(3);
    assert_eq!(due(NOW, 0, &three), FRIDAY + 3 * HOUR);
    // Before today's moment, the last one is yesterday's.
    assert_eq!(due(FRIDAY + 2 * HOUR, 0, &three), FRIDAY - 21 * HOUR);
    // 15:00 local at UTC+7: 03:00 local today is 20:00 UTC the day before.
    assert_eq!(due(NOW, 420, &three), FRIDAY - 4 * HOUR);
    // Weekly on Monday (0), Friday (4) at 09:00 (the day is not over yet) and Sunday (6).
    let weekly = |weekday: u8, minute_of_day: u16| Schedule {
        frequency: Frequency::Weekly,
        weekday,
        minute_of_day,
        ..daily(0)
    };
    assert_eq!(due(NOW, 0, &weekly(0, 180)), FRIDAY - 4 * DAY + 3 * HOUR);
    assert_eq!(due(NOW, 0, &weekly(4, 540)), FRIDAY - 7 * DAY + 9 * HOUR);
    assert_eq!(due(NOW, 0, &weekly(4, 420)), FRIDAY + 7 * HOUR);
    assert_eq!(due(NOW, 0, &weekly(6, 180)), FRIDAY - 5 * DAY + 3 * HOUR);
}

#[test]
fn the_memory_rule_needs_both_thresholds_and_is_off_by_default() {
    let rule = MemoryRule {
        enabled: true,
        ..MemoryRule::default()
    };
    let state = AutomationState::default();
    let decide = |rule: &MemoryRule, world: &World| {
        memory_decision(rule, &state, at(NOW), STARTUP_GRACE_MS, world)
    };
    assert_eq!(
        decide(&MemoryRule::default(), &World::quiet()),
        Decision::Idle
    );
    assert_eq!(decide(&rule, &World::quiet()), Decision::Run);
    // Plenty of free memory: nothing to do.
    let mut roomy = World::quiet();
    roomy.lists = Some(MemoryLists::from_lists(16 * GIB, 6 * GIB, 0, 4 * GIB));
    assert_eq!(decide(&rule, &roomy), Decision::Idle);
    // Little standby: not worth cleaning.
    let mut small = World::quiet();
    small.lists = Some(MemoryLists::from_lists(
        16 * GIB,
        2 * GIB,
        0,
        512 * MIB_BYTES,
    ));
    assert_eq!(decide(&rule, &small), Decision::Idle);
    // The thresholds are strict: exactly at the limit does not trigger.
    let mut edge = World::quiet();
    edge.lists = Some(MemoryLists::from_lists(
        16 * GIB,
        u64::from(rule.standby_above_mb) * MIB_BYTES,
        0,
        512 * MIB_BYTES,
    ));
    assert_eq!(decide(&rule, &edge), Decision::Idle);
    // Memory that cannot be read is not a reason to clean.
    let mut blind = World::quiet();
    blind.lists = None;
    assert_eq!(decide(&rule, &blind), Decision::Idle);
}

#[test]
fn the_memory_rule_waits_for_start_cooldown_and_pauses() {
    let rule = MemoryRule {
        enabled: true,
        cooldown_minutes: 30,
        ..MemoryRule::default()
    };
    let world = World::quiet();
    let fresh = AutomationState::default();
    // Not in the first minutes after start.
    assert_eq!(
        memory_decision(&rule, &fresh, at(NOW), STARTUP_GRACE_MS - 1, &world),
        Decision::Idle
    );
    // Cooldown after an automatic run.
    let ran = AutomationState {
        last_memory_run: Some(at(NOW - 29 * 60_000)),
        ..fresh
    };
    assert_eq!(
        memory_decision(&rule, &ran, at(NOW), STARTUP_GRACE_MS, &world),
        Decision::Idle
    );
    let rested = AutomationState {
        last_memory_run: Some(at(NOW - 31 * 60_000)),
        ..fresh
    };
    assert_eq!(
        memory_decision(&rule, &rested, at(NOW), STARTUP_GRACE_MS, &world),
        Decision::Run
    );
    // Pauses, in order: a recent manual run, a fullscreen app, battery saver.
    let paused = |world: &World| memory_decision(&rule, &fresh, at(NOW), STARTUP_GRACE_MS, world);
    let mut manual = World::quiet();
    manual.manual = Some(at(NOW - 10 * 60_000));
    manual.fullscreen = true;
    assert_eq!(paused(&manual), Decision::Pause(PauseReason::ManualRun));
    manual.manual = Some(at(NOW - 31 * 60_000));
    assert_eq!(paused(&manual), Decision::Pause(PauseReason::Fullscreen));
    manual.fullscreen = false;
    manual.battery_saver = true;
    assert_eq!(paused(&manual), Decision::Pause(PauseReason::BatterySaver));
    manual.battery_saver = false;
    assert_eq!(paused(&manual), Decision::Run);
}

#[test]
fn a_schedule_runs_once_per_period_and_a_missed_one_runs_at_the_next_start() {
    let schedule = daily(3);
    let world = World::quiet();
    let armed = |at_ms: i64| AutomationState {
        clean_armed_at: Some(at(at_ms)),
        ..AutomationState::default()
    };
    let decide = |state: &AutomationState, now: i64| {
        cleaning_decision(&schedule, state, at(now), STARTUP_GRACE_MS, &world)
    };
    // Switched on at 08:00: today's 03:00 is not missed.
    assert_eq!(decide(&armed(NOW), NOW + HOUR), Decision::Idle);
    // Tomorrow 03:00 has passed and nothing ran: due, also when Volt starts at noon.
    assert_eq!(decide(&armed(NOW), NOW + 28 * HOUR), Decision::Run);
    // After the run the period is served, however often the timer ticks.
    let served = AutomationState {
        last_clean_run: Some(at(NOW + 27 * HOUR + 1)),
        ..armed(NOW)
    };
    assert_eq!(decide(&served, NOW + 28 * HOUR), Decision::Idle);
    assert_eq!(decide(&served, NOW + 40 * HOUR), Decision::Idle);
    // The next day's moment is a new period.
    assert_eq!(decide(&served, NOW + 52 * HOUR), Decision::Run);
    // An empty selection, or a schedule that is off, never runs.
    let empty = Schedule {
        rule_ids: Vec::new(),
        ..daily(3)
    };
    assert_eq!(
        cleaning_decision(
            &empty,
            &armed(NOW),
            at(NOW + 28 * HOUR),
            STARTUP_GRACE_MS,
            &world
        ),
        Decision::Idle
    );
    assert_eq!(
        cleaning_decision(
            &Schedule::default(),
            &armed(NOW),
            at(NOW + 28 * HOUR),
            STARTUP_GRACE_MS,
            &world
        ),
        Decision::Idle
    );
}

#[test]
fn a_schedule_does_not_run_in_the_first_minutes_and_pauses_skip_the_period() {
    let schedule = daily(3);
    let state = AutomationState {
        clean_armed_at: Some(at(NOW)),
        ..AutomationState::default()
    };
    let now = at(NOW + 28 * HOUR);
    let mut world = World::quiet();
    assert_eq!(
        cleaning_decision(&schedule, &state, now, STARTUP_GRACE_MS - 1, &world),
        Decision::Idle
    );
    world.fullscreen = true;
    assert_eq!(
        cleaning_decision(&schedule, &state, now, STARTUP_GRACE_MS, &world),
        Decision::Pause(PauseReason::Fullscreen)
    );
    world.fullscreen = false;
    world.manual = Some(at(NOW + 28 * HOUR - 5 * 60_000));
    assert_eq!(
        cleaning_decision(&schedule, &state, now, STARTUP_GRACE_MS, &world),
        Decision::Pause(PauseReason::ManualRun)
    );
}

#[test]
fn the_next_run_is_the_next_occurrence_or_now_when_one_is_waiting() {
    let schedule = daily(3);
    let armed = AutomationState {
        clean_armed_at: Some(at(NOW)),
        ..AutomationState::default()
    };
    assert_eq!(
        next_cleaning_run(&schedule, &armed, at(NOW + HOUR), 0),
        Some(at(FRIDAY + DAY + 3 * HOUR))
    );
    assert_eq!(
        next_cleaning_run(&schedule, &armed, at(NOW + 28 * HOUR), 0),
        Some(at(NOW + 28 * HOUR))
    );
    assert_eq!(
        next_cleaning_run(&Schedule::default(), &armed, at(NOW), 0),
        None
    );
}

#[test]
fn the_configuration_is_off_by_default_bounded_and_falls_back_to_off() {
    let settings = FakeSettings::new();
    let config = AutomationConfig::load(&settings);
    assert_eq!(config, AutomationConfig::default());
    assert!(!config.any_enabled());
    assert!(config.validate().is_ok());

    let mut bad = AutomationConfig::default();
    bad.memory.cooldown_minutes = 1;
    assert!(bad.validate().is_err());
    bad.memory.cooldown_minutes = 30;
    bad.memory.standby_above_mb = 10;
    assert!(bad.validate().is_err());
    bad.memory.standby_above_mb = 4096;
    bad.cleaning.minute_of_day = 1440;
    assert!(bad.validate().is_err());
    bad.cleaning.minute_of_day = 0;
    bad.cleaning.weekday = 7;
    assert!(bad.validate().is_err());

    // A stored value out of range is clamped, never trusted; unreadable means off.
    let mut stored = AutomationConfig::default();
    stored.memory.enabled = true;
    stored.memory.cooldown_minutes = 1;
    stored.memory.free_below_mb = u32::MAX;
    settings.set_typed(CONFIG_KEY, &stored).expect("stored");
    let loaded = AutomationConfig::load(&settings);
    assert!(loaded.memory.enabled);
    assert_eq!(loaded.memory.cooldown_minutes, *COOLDOWN_MINUTES.start());
    assert_eq!(loaded.memory.free_below_mb, *FREE_BELOW_MB.end());
    settings
        .set(CONFIG_KEY, &serde_json::json!("nonsense"))
        .expect("stored");
    assert_eq!(
        AutomationConfig::load(&settings),
        AutomationConfig::default()
    );
}

// ---- the runner over fakes ---------------------------------------------------------------

const RULES: &str = r#"
[[rule]]
id = "user-temp"
title = "Temporary files"
description = "Files in the temp folder."
category = "temporary_files"
risk = "safe"
include = ["**"]
[rule.root]
location = "user_temp"
[rule.constraints]
min_age_days = 1
max_depth = 2

[[rule]]
id = "machine-rule"
title = "Machine"
description = "Needs administrator rights."
category = "logs"
risk = "safe"
include = ["**"]
requires_admin = true
[rule.root]
location = "windows"
path = "Temp"
[rule.constraints]
min_age_days = 0
max_depth = 1

[[rule]]
id = "picked"
title = "Picked"
description = "The user picks the items."
category = "logs"
risk = "safe"
include = ["**"]
unit = "entries"
pick = true
[rule.root]
location = "local_app_data"
path = "Picked"
[rule.constraints]
min_age_days = 0
max_depth = 0
"#;

fn abs(rest: &str) -> PathBuf {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    Path::new(base).join(rest)
}

struct Fixture {
    memory: Arc<FakeMemory>,
    fs: Arc<FakeFileSystem>,
    activity: Arc<FakeActivityLog>,
    settings: Arc<FakeSettings>,
    env: Arc<FakeEnvironment>,
    clock: Arc<ManualClock>,
    cleaner: MemoryCleaner,
    engine: CleanEngine,
    automation: Automation,
}

fn fixture() -> Fixture {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, abs("Users/t/Temp"));
    folders.set(Location::LocalAppData, abs("Users/t/Local"));
    folders.set(Location::Windows, abs("Windows"));
    folders.protect(abs("Users/t"));
    folders.protect(abs("Windows"));
    let fs = Arc::new(FakeFileSystem::new());
    let old = at(NOW - 10 * DAY);
    fs.add_file_at(abs("Users/t/Temp/a.tmp"), 1000, old);
    fs.add_file_at(abs("Users/t/Temp/b.tmp"), 2000, old);
    fs.add_file_at(abs("Windows/Temp/m.tmp"), 900, old);
    fs.add_file_at(abs("Users/t/Local/Picked/p.bin"), 700, old);
    let clock = Arc::new(ManualClock::new(NOW));
    let activity = Arc::new(FakeActivityLog::new());
    let settings = Arc::new(FakeSettings::new());
    let memory = Arc::new(FakeMemory::new());
    let env = Arc::new(FakeEnvironment::new());
    let engine = CleanEngine::new(
        CleaningRules::from_toml(RULES).expect("valid rules"),
        folders,
        fs.clone(),
        clock.clone(),
    )
    .with_activity_log(activity.clone());
    let cleaner = MemoryCleaner::new(memory.clone(), clock.clone())
        .with_activity_log(activity.clone())
        .with_exclusions(settings.clone());
    let automation = Automation::new(
        settings.clone(),
        activity.clone(),
        clock.clone(),
        env.clone(),
    );
    Fixture {
        memory,
        fs,
        activity,
        settings,
        env,
        clock,
        cleaner,
        engine,
        automation,
    }
}

impl Fixture {
    fn tick(&self) -> TickReport {
        self.automation
            .tick(&self.cleaner, Some(&self.engine), &BoostConfig::default())
    }

    fn enable_memory(&self) {
        let mut config = self.automation.config();
        config.memory.enabled = true;
        self.automation.save_config(&config).expect("saved");
    }

    fn crowd_memory(&self) {
        self.memory.set_lists(MemoryLists::from_lists(
            16 * GIB,
            6 * GIB,
            0,
            512 * MIB_BYTES,
        ));
    }
}

#[test]
fn everything_is_off_by_default_and_a_tick_does_nothing() {
    let f = fixture();
    f.crowd_memory();
    f.clock.advance(3 * DAY);
    assert_eq!(f.tick(), TickReport::default());
    assert!(f.memory.purged().is_empty());
    assert!(f.activity.records().is_empty());
}

#[test]
fn memory_is_cleaned_like_boost_recorded_as_automation_and_then_rests_for_the_cooldown() {
    let f = fixture();
    f.enable_memory();
    f.crowd_memory();
    f.clock.advance(STARTUP_GRACE_MS);
    let report = f.tick();
    assert!(
        report
            .memory
            .as_ref()
            .is_some_and(MemoryCleanReport::any_purged)
    );
    assert!(report.worth_telling());
    // Boost's default operation only: the working-set trim.
    assert_eq!(f.memory.purged(), [MemoryArea::WorkingSets]);
    let records = f.activity.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, Source::Automation);
    assert_eq!(records[0].label.as_deref(), Some(MEMORY_LABEL));

    // Crowded again a minute later: the cooldown holds it back.
    f.crowd_memory();
    f.clock.advance(60_000);
    assert_eq!(f.tick(), TickReport::default());
    f.clock.advance(31 * 60_000);
    assert!(f.tick().memory.is_some());
    assert_eq!(f.memory.purged().len(), 2);
}

#[test]
fn memory_honours_the_protected_programs_list() {
    let f = fixture();
    let mut protected = ExclusionList::new();
    protected.add("app.exe").expect("added");
    f.settings
        .set_typed(EXCLUSIONS_SETTINGS_KEY, &protected)
        .expect("stored");
    f.enable_memory();
    f.crowd_memory();
    f.clock.advance(STARTUP_GRACE_MS);
    assert!(f.tick().memory.is_some());
    assert_eq!(f.memory.trimmed_processes(), ["editor.exe"]);
}

#[test]
fn memory_does_not_run_after_a_manual_run_or_while_paused_and_is_not_queued() {
    let f = fixture();
    f.enable_memory();
    f.clock.advance(STARTUP_GRACE_MS);

    // The user trimmed by hand a moment ago.
    let manual = f.cleaner.run(&[MemoryArea::WorkingSets], Source::Manual);
    assert!(manual.any_purged());
    f.crowd_memory();
    assert_eq!(f.tick(), TickReport::default());
    assert_eq!(f.memory.purged().len(), 1, "only the manual run");

    // Past the cooldown, but a fullscreen app is active: skipped, nothing waits.
    f.clock.advance(31 * 60_000);
    f.env.set_fullscreen(true);
    assert_eq!(f.tick(), TickReport::default());
    f.env.set_fullscreen(false);
    f.env.set_battery_saver(true);
    assert_eq!(f.tick(), TickReport::default());
    assert_eq!(f.memory.purged().len(), 1);

    // When the pause ends the thresholds are judged afresh.
    f.env.set_battery_saver(false);
    assert!(f.tick().memory.is_some());
    assert_eq!(f.memory.purged().len(), 2);
}

#[test]
fn memory_does_not_run_when_the_only_operations_need_administrator_rights() {
    let f = fixture();
    f.enable_memory();
    f.crowd_memory();
    f.memory
        .make_unavailable(MemoryArea::WorkingSets, Availability::NeedsAdministrator);
    f.clock.advance(STARTUP_GRACE_MS);
    assert_eq!(f.tick(), TickReport::default());
    assert!(f.memory.purged().is_empty());
}

fn schedule_config(f: &Fixture, ids: &[&str]) {
    let mut config = f.automation.config();
    config.cleaning = Schedule {
        rule_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        ..daily(3)
    };
    f.automation.save_config(&config).expect("saved");
}

#[test]
fn a_scheduled_clean_moves_only_the_rules_that_may_run_alone_once_per_period() {
    let f = fixture();
    // The selection holds a rule that needs the helper, one picked by hand and one that does not
    // exist: none of them may run.
    schedule_config(&f, &["user-temp", "machine-rule", "picked", "gone"]);
    f.clock.advance(STARTUP_GRACE_MS);
    assert_eq!(
        f.tick(),
        TickReport::default(),
        "today's 03:00 is not missed"
    );

    // Tomorrow 03:30.
    f.clock.advance(19 * HOUR + 30 * 60_000);
    let report = f.tick();
    let Some(CleaningOutcome::Cleaned(cleaned)) = &report.cleaning else {
        panic!("the clean should have run: {report:?}");
    };
    assert_eq!(cleaned.moved_files(), 2);
    let mut trashed = f.fs.trashed();
    trashed.sort();
    assert_eq!(
        trashed,
        [abs("Users/t/Temp/a.tmp"), abs("Users/t/Temp/b.tmp")],
        "no administrator rule, no picked rule"
    );
    assert!(report.worth_telling());
    let records = f.activity.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, Source::Automation);
    assert_eq!(records[0].label.as_deref(), Some("user-temp"));

    // The period is served: later ticks do nothing.
    f.clock.advance(HOUR);
    assert_eq!(f.tick(), TickReport::default());
    assert!(f.automation.state().last_clean_run.is_some());

    // The next day there is nothing left to move.
    f.clock.advance(DAY);
    assert_eq!(f.tick().cleaning, Some(CleaningOutcome::NothingToClean));
    assert_eq!(f.activity.records().len(), 1);
}

#[test]
fn a_missed_scheduled_clean_runs_once_when_volt_starts_again() {
    let f = fixture();
    schedule_config(&f, &["user-temp"]);
    // Volt was closed for three days: a new process starts with the saved state.
    let later = Arc::new(ManualClock::new(NOW + 3 * DAY + 10 * HOUR));
    let restarted = Automation::new(
        f.settings.clone(),
        f.activity.clone(),
        later.clone(),
        f.env.clone(),
    );
    assert_eq!(
        restarted
            .tick(&f.cleaner, Some(&f.engine), &BoostConfig::default())
            .cleaning,
        None,
        "not in the first minutes after the start"
    );
    later.advance(STARTUP_GRACE_MS);
    let report = restarted.tick(&f.cleaner, Some(&f.engine), &BoostConfig::default());
    assert!(matches!(report.cleaning, Some(CleaningOutcome::Cleaned(_))));
    later.advance(HOUR);
    assert_eq!(
        restarted.tick(&f.cleaner, Some(&f.engine), &BoostConfig::default()),
        TickReport::default(),
        "three missed days still make one run"
    );
}

#[test]
fn a_paused_scheduled_clean_is_skipped_for_the_period_not_queued() {
    let f = fixture();
    schedule_config(&f, &["user-temp"]);
    f.clock.advance(STARTUP_GRACE_MS + 19 * HOUR + 30 * 60_000);
    f.env.set_fullscreen(true);
    let report = f.tick();
    assert_eq!(
        report.cleaning,
        Some(CleaningOutcome::Skipped(PauseReason::Fullscreen))
    );
    assert!(!report.worth_telling());
    assert!(f.fs.trashed().is_empty());
    let skip = f.automation.state().last_clean_skip.expect("remembered");
    assert_eq!(skip.reason, PauseReason::Fullscreen);

    // The game ends an hour later: the skipped period is not made up.
    f.env.set_fullscreen(false);
    f.clock.advance(HOUR);
    assert_eq!(f.tick(), TickReport::default());
    assert!(f.fs.trashed().is_empty());
    // The next day's occurrence runs.
    f.clock.advance(DAY);
    assert!(matches!(
        f.tick().cleaning,
        Some(CleaningOutcome::Cleaned(_))
    ));
}

#[test]
fn moving_the_schedule_starts_a_new_first_period() {
    let f = fixture();
    schedule_config(&f, &["user-temp"]);
    f.clock.advance(DAY);
    // 3 hours later in the day is still today's past at 08:00+: move it to 07:00 and it is not
    // counted as missed.
    let mut config = f.automation.config();
    config.cleaning.minute_of_day = 7 * 60;
    f.automation.save_config(&config).expect("saved");
    f.clock.advance(STARTUP_GRACE_MS);
    assert_eq!(f.tick(), TickReport::default());
    assert!(f.fs.trashed().is_empty());
}

#[test]
fn the_status_names_the_pause_and_the_next_run() {
    let f = fixture();
    schedule_config(&f, &["user-temp"]);
    let status = f.automation.status(Some(&f.engine));
    assert_eq!(status.paused, None);
    assert_eq!(status.next_cleaning_run, Some(at(FRIDAY + DAY + 3 * HOUR)));
    f.env.set_battery_saver(true);
    assert_eq!(
        f.automation.status(Some(&f.engine)).paused,
        Some(PauseReason::BatterySaver)
    );
}
