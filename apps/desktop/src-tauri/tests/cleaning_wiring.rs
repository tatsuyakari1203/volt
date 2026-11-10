//! The clean engine inside the composition root: [`build_core`] hands the shell a clean engine
//! built from the embedded rules, the file system port, the system clock and the one store (as the
//! activity log and as the settings).
//!
//! Everything runs over the in-memory fakes of `volt-core` plus a real `Store` in a temp
//! directory. Nothing touches the real file system, and the Windows known folders are replaced by
//! a fake so no real folder is ever resolved here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::{FakeActivityLog, FakeSystem};
use volt_core::{
    ActivityOutcome, Clock, Location, ReconcileReport, ScanFailure, SettingsExt, Source,
    SystemClock, Timestamp, TweakEngine,
};
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::wiring::{
    AppCore, CleaningSetup, InitError, StartupReport, build_core_with_cleaning,
};

fn abs(rest: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    path.extend(rest.split('/'));
    path
}

fn temp_folder() -> PathBuf {
    abs("Users/tester/AppData/Local/Temp")
}

fn fake_folders() -> Arc<FakeKnownFolders> {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, temp_folder());
    folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
    folders.set(Location::UserProfile, abs("Users/tester"));
    folders.set(
        Location::RoamingAppData,
        abs("Users/tester/AppData/Roaming"),
    );
    folders.set(Location::ProgramData, abs("ProgramData"));
    folders.set(Location::Windows, abs("Windows"));
    folders.protect(abs("Users/tester"));
    folders.protect(abs("Windows"));
    folders
}

fn db(dir: &TempDir) -> PathBuf {
    dir.path().join("volt.db")
}

fn setup() -> CleaningSetup<'static> {
    CleaningSetup {
        rules_toml: EMBEDDED_CLEANING_RULES,
        folders: fake_folders(),
    }
}

/// A file modified in 1970: old enough for any minimum age, whatever the clock says.
fn ancient() -> Timestamp {
    Timestamp::from_unix_millis(1_000_000)
}

#[test]
fn the_core_carries_a_clean_engine_over_the_embedded_rules() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let core = build_core_with_cleaning(system.ports(), &db(&dir), EMBEDDED_CATALOGUE, setup())
        .expect("core builds");
    let engine = core.clean_engine().expect("the core has a clean engine");
    let embedded = volt_core::CleaningRules::from_toml(EMBEDDED_CLEANING_RULES).expect("rules");
    assert_eq!(engine.rules().len(), embedded.len());
    assert!(engine.rules().get("user-temp").is_some());
    // Nothing exists on the fake machine: every rule scans as "nothing to clean".
    assert!(engine.scan_all().is_empty());
}

#[test]
fn a_clean_goes_through_the_ports_and_writes_the_activity_log_of_the_store() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .file_system
        .add_file_at(temp_folder().join("old.tmp"), 2048, ancient());
    system
        .file_system
        .add_file_at(temp_folder().join("today.tmp"), 1, SystemClock.now());
    let core = build_core_with_cleaning(system.ports(), &db(&dir), EMBEDDED_CATALOGUE, setup())
        .expect("core builds");
    let engine = core.clean_engine().expect("clean engine");

    let plan = engine.scan(&["user-temp"]).expect("scan");
    assert_eq!(plan.rules[0].failure, None);
    assert_eq!(plan.total_files(), 1, "only the old file is listed");
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    assert_eq!(
        system.file_system.trashed(),
        [temp_folder().join("old.tmp")]
    );

    // The record is in the store the core's activity port reads, and names no path.
    let records = core.activity.list(10, None).expect("activity");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert_eq!(records[0].label.as_deref(), Some("user-temp"));
    assert!(records[0].summary.contains("Recycle Bin"));
    assert!(!records[0].summary.contains("old.tmp"));
}

#[test]
fn the_store_is_the_settings_of_the_clean_engine() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    // A file 10 days old by the real clock, which the engine uses.
    let now = SystemClock.now();
    let ten_days = 10 * 86_400_000;
    system.file_system.add_file_at(
        temp_folder().join("ten-days.tmp"),
        1,
        Timestamp::from_unix_millis(now.as_unix_millis() - ten_days),
    );
    let core = build_core_with_cleaning(system.ports(), &db(&dir), EMBEDDED_CATALOGUE, setup())
        .expect("core builds");
    let engine = core.clean_engine().expect("clean engine");

    // The rule's own 7 days take it.
    assert_eq!(engine.scan(&["user-temp"]).expect("scan").total_files(), 1);

    // A setting written through the core's settings port (the store) is read by the next scan.
    core.settings
        .set_typed("clean.temp_min_age_days", &30_u32)
        .expect("set");
    let plan = engine.scan(&["user-temp"]).expect("scan");
    assert_eq!(plan.rules[0].min_age_days, 30);
    assert_eq!(
        plan.total_files(),
        0,
        "a file newer than the configured age is left alone"
    );

    // An unusable value stops the rule, typed.
    core.settings
        .set_typed("clean.temp_min_age_days", &0_u32)
        .expect("set");
    let plan = engine.scan(&["user-temp"]).expect("scan");
    assert_eq!(
        plan.rules[0].failure,
        Some(ScanFailure::MinAgeSettingInvalid)
    );
}

#[test]
fn invalid_cleaning_rules_are_a_typed_init_error_and_never_a_panic() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let result = build_core_with_cleaning(
        system.ports(),
        &db(&dir),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: "[[rule]]\nid = \"Bad Id\"\n",
            folders: fake_folders(),
        },
    );
    let Err(InitError::CleaningRules(error)) = result else {
        panic!("expected a cleaning rules error");
    };
    assert!(!error.issues.is_empty());
    assert!(error.to_string().contains("cleaning rules rejected"));
    assert!(
        !db(&dir).exists(),
        "the rules are checked before the database is opened"
    );

    // Text that is not even TOML is the same kind of error.
    let result = build_core_with_cleaning(
        system.ports(),
        &db(&dir),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: "this is = not [toml",
            folders: fake_folders(),
        },
    );
    assert!(matches!(result, Err(InitError::CleaningRules(_))));
}

#[test]
fn a_core_assembled_by_hand_has_no_clean_engine_until_one_is_attached() {
    let system = FakeSystem::new();
    let catalogue = Arc::new(volt_core::Catalogue::from_toml(EMBEDDED_CATALOGUE).expect("valid"));
    let engine = TweakEngine::new(
        catalogue,
        system.ports(),
        system.journal.clone(),
        system.clock.clone(),
    );
    let core = AppCore::new(
        engine,
        system.journal.clone(),
        Arc::new(volt_core::fakes::FakeSettings::new()),
        Arc::new(FakeActivityLog::new()),
        StartupReport {
            store_path: Path::new("fake").to_path_buf(),
            recovery: None,
            reconcile: Ok(ReconcileReport::default()),
        },
    );
    assert!(core.clean_engine().is_none());
}
