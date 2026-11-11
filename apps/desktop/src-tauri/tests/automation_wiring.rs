//! Automation over a real core built on the in-memory fakes and a throwaway store. Nothing here
//! touches this machine: memory, files, the fullscreen and battery signals and the clock are fakes.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::automation::STARTUP_GRACE_MS;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::{FakeEnvironment, FakeSystem, ManualClock};
use volt_core::{
    Automation, CleaningOutcome, Location, MemoryArea, MemoryLists, Source, Timestamp,
};
use volt_desktop_lib::automation::{
    AutomationConfigView, FrequencyView, PauseView, automation_status_of, notification_body,
    set_automation_config_of, tick_of, tooltip_line,
};
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::language::Lang;
use volt_desktop_lib::wiring::{AppCore, CleaningSetup, build_core_with_cleaning};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// 08:00 UTC on a Friday.
const NOW: i64 = 20_833 * DAY + 8 * HOUR;
const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

fn abs(rest: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    path.extend(rest.split('/'));
    path
}

struct Rig {
    core: AppCore,
    system: FakeSystem,
    env: Arc<FakeEnvironment>,
    clock: Arc<ManualClock>,
    _dir: TempDir,
}

fn rig() -> Rig {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, abs("Users/tester/AppData/Local/Temp"));
    folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
    folders.set(
        Location::RoamingAppData,
        abs("Users/tester/AppData/Roaming"),
    );
    folders.set(Location::ProgramData, abs("ProgramData"));
    folders.set(Location::Windows, abs("Windows"));
    folders.protect(abs("Users/tester"));
    folders.protect(abs("Windows"));
    let core = build_core_with_cleaning(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders,
        },
    )
    .expect("core builds");
    let env = Arc::new(FakeEnvironment::new());
    let clock = Arc::new(ManualClock::new(NOW));
    let automation = Automation::new(
        core.settings.clone(),
        core.activity.clone(),
        clock.clone(),
        env.clone(),
    );
    Rig {
        core: core.with_automation(automation),
        system,
        env,
        clock,
        _dir: dir,
    }
}

fn add_temp_file(system: &FakeSystem, name: &str, size: u64) {
    system.file_system.add_file_at(
        abs("Users/tester/AppData/Local/Temp").join(name),
        size,
        Timestamp::from_unix_millis(1_000_000),
    );
}

fn crowd(system: &FakeSystem) {
    system
        .memory
        .set_lists(MemoryLists::from_lists(16 * GIB, 6 * GIB, 0, 512 * MIB));
}

fn config(rig: &Rig) -> AutomationConfigView {
    automation_status_of(&rig.core).expect("status").config
}

#[test]
fn both_features_are_off_by_default_and_a_tick_changes_nothing() {
    let rig = rig();
    add_temp_file(&rig.system, "a.tmp", 4096);
    crowd(&rig.system);
    let status = automation_status_of(&rig.core).expect("status");
    assert!(!status.config.memory.enabled && !status.config.cleaning.enabled);
    assert!(status.config.notify);
    assert_eq!(status.next_clean_run, None);
    assert_eq!(status.bounds.cooldown_minutes, [5, 1440]);

    rig.clock.advance(3 * DAY);
    let report = tick_of(&rig.core).expect("tick");
    assert!(report.memory.is_none() && report.cleaning.is_none());
    assert!(rig.system.memory.purged().is_empty());
    assert!(rig.system.file_system.trashed().is_empty());
}

#[test]
fn the_configuration_is_checked_before_it_is_saved() {
    let rig = rig();
    let mut view = config(&rig);

    view.memory.cooldown_minutes = 1;
    assert!(set_automation_config_of(&rig.core, &view).is_err());
    view.memory.cooldown_minutes = 30;

    // A rule that needs the helper cannot be put on the schedule, and neither can an unknown one.
    view.cleaning.enabled = true;
    view.cleaning.rule_ids = vec!["windows-temp".to_owned()];
    assert!(set_automation_config_of(&rig.core, &view).is_err());
    view.cleaning.rule_ids = vec!["no-such-rule".to_owned()];
    assert!(set_automation_config_of(&rig.core, &view).is_err());
    assert!(!config(&rig).cleaning.enabled, "nothing was saved");

    view.cleaning.rule_ids = vec!["user-temp".to_owned()];
    view.cleaning.frequency = FrequencyView::Weekly;
    view.cleaning.weekday = 6;
    view.cleaning.minute_of_day = 3 * 60;
    let saved = set_automation_config_of(&rig.core, &view).expect("saved");
    assert_eq!(saved.config, view);
    assert!(saved.next_clean_run.is_some());
    assert_eq!(automation_status_of(&rig.core).expect("status"), saved);
}

#[test]
fn a_threshold_run_is_the_boost_clean_recorded_as_automation_with_its_sentences() {
    let rig = rig();
    let mut view = config(&rig);
    view.memory.enabled = true;
    set_automation_config_of(&rig.core, &view).expect("saved");
    crowd(&rig.system);
    rig.clock.advance(STARTUP_GRACE_MS);

    let report = tick_of(&rig.core).expect("tick");
    assert!(report.worth_telling());
    assert_eq!(rig.system.memory.purged(), [MemoryArea::WorkingSets]);
    let records = rig.core.activity.list(10, None).expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].source, Source::Automation);

    let body = notification_body(&report, Lang::English).expect("body");
    assert!(
        body.starts_with("Memory cleaned automatically: available "),
        "{body}"
    );
    assert!(!body.contains("freed"));
    let line = tooltip_line(&report, Lang::Vietnamese).expect("line");
    assert!(line.starts_with("Volt: đã tự dọn bộ nhớ"), "{line}");

    // The cooldown holds the next one back, and the page can say when it last ran.
    crowd(&rig.system);
    assert!(tick_of(&rig.core).expect("tick").memory.is_none());
    assert!(
        automation_status_of(&rig.core)
            .expect("status")
            .last_memory_run
            .is_some()
    );
}

#[test]
fn a_scheduled_clean_moves_to_the_recycle_bin_once_and_is_recorded_as_automation() {
    let rig = rig();
    add_temp_file(&rig.system, "a.tmp", 4096);
    let mut view = config(&rig);
    view.cleaning.enabled = true;
    view.cleaning.minute_of_day = 3 * 60;
    view.cleaning.rule_ids = vec!["user-temp".to_owned()];
    set_automation_config_of(&rig.core, &view).expect("saved");

    // Tomorrow 03:30.
    rig.clock.advance(19 * HOUR + 30 * 60_000);
    let report = tick_of(&rig.core).expect("tick");
    let Some(CleaningOutcome::Cleaned(cleaned)) = &report.cleaning else {
        panic!("the schedule should have run: {report:?}");
    };
    assert_eq!(cleaned.moved_files(), 1);
    assert_eq!(rig.system.file_system.trashed().len(), 1);
    let records = rig.core.activity.list(10, None).expect("records");
    assert!(records.iter().all(|r| r.source == Source::Automation));
    assert!(
        records
            .iter()
            .any(|r| r.label.as_deref() == Some("user-temp"))
    );
    let body = notification_body(&report, Lang::English).expect("body");
    assert_eq!(body, "Scheduled clean: 1 file moved to the Recycle Bin.");

    // Served for the day.
    rig.clock.advance(HOUR);
    assert!(tick_of(&rig.core).expect("tick").cleaning.is_none());
    let status = automation_status_of(&rig.core).expect("status");
    assert!(status.last_clean_run.is_some());
    assert!(status.next_clean_run.is_some());
}

#[test]
fn a_fullscreen_app_skips_the_scheduled_clean_and_the_page_says_why() {
    let rig = rig();
    add_temp_file(&rig.system, "a.tmp", 4096);
    let mut view = config(&rig);
    view.cleaning.enabled = true;
    view.cleaning.minute_of_day = 3 * 60;
    view.cleaning.rule_ids = vec!["user-temp".to_owned()];
    set_automation_config_of(&rig.core, &view).expect("saved");

    rig.env.set_fullscreen(true);
    assert_eq!(
        automation_status_of(&rig.core).expect("status").paused,
        Some(PauseView::Fullscreen)
    );
    rig.clock.advance(19 * HOUR + 30 * 60_000);
    let report = tick_of(&rig.core).expect("tick");
    assert!(matches!(
        report.cleaning,
        Some(CleaningOutcome::Skipped(volt_core::PauseReason::Fullscreen))
    ));
    assert!(rig.system.file_system.trashed().is_empty());
    assert!(notification_body(&report, Lang::English).is_none());
    let skipped = automation_status_of(&rig.core)
        .expect("status")
        .last_clean_skip
        .expect("skip is remembered");
    assert_eq!(skipped.reason, PauseView::Fullscreen);
}
