//! The cleaner commands (`volt_desktop_lib::cleaner`) over the wired core: list the rules, scan,
//! clean the chosen rules of the scan, and the Recycle Bin card.
//!
//! Everything runs over the in-memory fakes of `volt-core` plus a real `Store` in a temp
//! directory. Nothing touches the real file system or the real Recycle Bin: the bin is a fake, and
//! "empty" is only ever called on it.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::{FakeKnownFolders, FakeRecycleBin};
use volt_core::fakes::FakeSystem;
use volt_core::{ActivityOutcome, Location, PortError, RecycleBinManager, SystemClock, Timestamp};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::cleaner::{self, CleanCategoryView, CleanScanProgress};
use volt_desktop_lib::wiring::{AppCore, CleaningSetup, build_core_with_cleaning};

fn abs(rest: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    path.extend(rest.split('/'));
    path
}

fn temp_folder() -> PathBuf {
    abs("Users/tester/AppData/Local/Temp")
}

fn windows_temp() -> PathBuf {
    abs("Windows/Temp")
}

fn fake_folders() -> Arc<FakeKnownFolders> {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, temp_folder());
    folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
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

/// A file modified in 1970: old enough for any minimum age.
fn ancient() -> Timestamp {
    Timestamp::from_unix_millis(1_000_000)
}

struct Fixture {
    _dir: TempDir,
    system: FakeSystem,
    bin: Arc<FakeRecycleBin>,
    core: AppCore,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .file_system
        .add_file_at(temp_folder().join("old.tmp"), 2048, ancient());
    system
        .file_system
        .add_file_at(temp_folder().join("sub").join("deep.tmp"), 1024, ancient());
    system
        .file_system
        .add_dir(temp_folder().join("sub"), Some(ancient()));
    system
        .file_system
        .add_file_at(windows_temp().join("machine.tmp"), 4096, ancient());
    system.file_system.add_dir(windows_temp(), Some(ancient()));
    let bin = Arc::new(FakeRecycleBin::holding(3, 5_000_000));
    let core = build_core_with_cleaning(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders: fake_folders(),
        },
    )
    .expect("core builds");
    let manager = RecycleBinManager::new(bin.clone(), Arc::new(SystemClock))
        .with_activity_log(core.activity.clone());
    Fixture {
        _dir: dir,
        system,
        bin,
        core: core.with_recycle_bin(manager),
    }
}

fn scan(core: &AppCore) -> cleaner::CleanScanResult {
    cleaner::scan_cleaning(core, &|_| {}).expect("scan")
}

fn ids(list: &[&str]) -> Vec<String> {
    list.iter().map(|id| (*id).to_owned()).collect()
}

#[test]
fn the_rules_are_listed_with_category_risk_and_the_administrator_flag() {
    let fx = fixture();
    let rules = cleaner::list_cleaning_rules(&fx.core).expect("rules");
    assert_eq!(rules.len(), 24);
    let temp = rules.iter().find(|r| r.id == "user-temp").expect("rule");
    assert_eq!(temp.category, CleanCategoryView::TemporaryFiles);
    assert_eq!(temp.category_label, "Temporary files");
    assert!(!temp.needs_administrator);
    assert_eq!(temp.min_age_days, 7);
    let machine = rules.iter().find(|r| r.id == "windows-temp").expect("rule");
    assert!(machine.needs_administrator);
}

#[test]
fn a_scan_reports_counts_and_sizes_per_rule_and_never_a_full_path() {
    let fx = fixture();
    let result = scan(&fx.core);
    assert!(!result.cancelled);
    assert!(!result.plan_id.is_empty());
    assert_eq!(result.rules.len(), 24);

    let temp = result
        .rules
        .iter()
        .find(|r| r.rule.id == "user-temp")
        .expect("rule");
    assert_eq!(temp.files, 2);
    assert!((temp.bytes - 3072.0).abs() < f64::EPSILON);
    assert!(!temp.truncated && temp.problem.is_none());
    assert!(
        temp.samples
            .iter()
            .any(|s| s == "user_temp/old.tmp" || s == "user_temp/sub/deep.tmp"),
        "{:?}",
        temp.samples
    );

    // The profile name appears nowhere in what the UI receives.
    let json = serde_json::to_string(&result).expect("json");
    assert!(!json.contains("tester"), "{json}");
    assert!(!json.contains("Users"), "{json}");
    // The rule descriptions name well-known folders such as C:\Windows\Temp; samples never start
    // with a drive.
    assert!(
        result
            .rules
            .iter()
            .flat_map(|r| &r.samples)
            .all(|s| !s.contains(':') && !s.contains('\\')),
        "{json}"
    );
}

#[test]
fn a_scan_emits_progress_before_each_rule() {
    let fx = fixture();
    let seen = RefCell::new(Vec::new());
    cleaner::scan_cleaning(&fx.core, &|p| seen.borrow_mut().push(p)).expect("scan");
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 24);
    assert_eq!(seen[0], CleanScanProgress { done: 0, total: 24 });
    assert_eq!(
        seen[10],
        CleanScanProgress {
            done: 10,
            total: 24
        }
    );
}

#[test]
fn cleaning_moves_only_the_chosen_rules_and_says_where_the_files_went() {
    let fx = fixture();
    let result = scan(&fx.core);
    let run = cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"])).expect("clean");

    assert_eq!(run.moved_files, 2);
    assert!((run.moved_bytes - 3072.0).abs() < f64::EPSILON);
    assert_eq!(run.skipped_total, 0);
    assert!(
        run.message.contains("to the Recycle Bin"),
        "{}",
        run.message
    );
    assert!(
        run.message.contains("until the Recycle Bin is emptied"),
        "{}",
        run.message
    );
    assert_eq!(run.rules.len(), 1);
    assert_eq!(run.rules[0].id, "user-temp");

    let trashed = fx.system.file_system.trashed();
    assert!(trashed.contains(&temp_folder().join("old.tmp")));
    assert!(
        !trashed.iter().any(|p| p.starts_with(windows_temp())),
        "a rule that was not chosen is left alone"
    );
    assert_eq!(fx.bin.empty_calls(), 0, "cleaning never empties the bin");
}

#[test]
fn a_scan_can_be_cleaned_once_and_a_new_scan_replaces_the_old_one() {
    let fx = fixture();
    let first = scan(&fx.core);
    let second = scan(&fx.core);
    assert_ne!(first.plan_id, second.plan_id);

    let stale = cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"]));
    assert!(matches!(stale, Err(IpcError::NotFound { .. })), "{stale:?}");
    assert!(fx.system.file_system.trashed().is_empty());

    cleaner::clean_plan(&fx.core, &second.plan_id, &ids(&["user-temp"])).expect("clean");
    let again = cleaner::clean_plan(&fx.core, &second.plan_id, &ids(&["user-temp"]));
    assert!(matches!(again, Err(IpcError::NotFound { .. })), "{again:?}");
}

#[test]
fn rules_that_need_administrator_rights_are_scanned_but_cannot_be_cleaned() {
    let fx = fixture();
    let result = scan(&fx.core);
    let machine = result
        .rules
        .iter()
        .find(|r| r.rule.id == "windows-temp")
        .expect("rule");
    assert!(machine.rule.needs_administrator);
    assert_eq!(machine.files, 1, "the scan is read-only, so it may look");

    let refused = cleaner::clean_plan(
        &fx.core,
        &result.plan_id,
        &ids(&["user-temp", "windows-temp"]),
    );
    assert!(
        matches!(refused, Err(IpcError::AccessDenied { .. })),
        "{refused:?}"
    );
    assert!(
        fx.system.file_system.trashed().is_empty(),
        "nothing moves when any chosen rule is refused"
    );
    // The refusal does not use the scan up.
    cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"])).expect("clean");
}

#[test]
fn a_clean_with_nothing_chosen_or_an_unknown_rule_is_refused() {
    let fx = fixture();
    let result = scan(&fx.core);
    let none = cleaner::clean_plan(&fx.core, &result.plan_id, &[]);
    assert!(
        matches!(none, Err(IpcError::InvalidInput { .. })),
        "{none:?}"
    );
    let unknown = cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["no-such-rule"]));
    assert!(
        matches!(unknown, Err(IpcError::InvalidInput { .. })),
        "{unknown:?}"
    );
    assert!(fx.system.file_system.trashed().is_empty());
}

#[test]
fn skipped_items_are_grouped_by_reason() {
    let fx = fixture();
    fx.system.file_system.fail_trash_of(
        temp_folder().join("old.tmp"),
        PortError::AccessDenied("locked".into()),
    );
    let result = scan(&fx.core);
    let run = cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"])).expect("clean");
    assert_eq!(run.moved_files, 1);
    assert_eq!(run.skipped_total, 1);
    assert_eq!(run.skipped.len(), 1);
    assert_eq!(run.skipped[0].reason, "locked, in use or access denied");
    assert_eq!(run.skipped[0].count, 1);
    assert!(
        run.message.contains("1 item left in place"),
        "{}",
        run.message
    );
    let json = serde_json::to_string(&run).expect("json");
    assert!(!json.contains("tester"), "{json}");
}

#[test]
fn cancelling_a_scan_keeps_the_earlier_one() {
    let fx = fixture();
    let first = scan(&fx.core);

    let cancelled = cleaner::scan_cleaning(&fx.core, &|progress| {
        if progress.done == 3 {
            cleaner::cancel_cleaning_scan(&fx.core);
        }
    })
    .expect("scan");
    assert!(cancelled.cancelled);
    assert!(cancelled.plan_id.is_empty() && cancelled.rules.is_empty());

    // The earlier scan still stands and can be cleaned.
    cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"])).expect("clean");

    // A cancel with no scan running does not cancel the next one.
    cleaner::cancel_cleaning_scan(&fx.core);
    assert!(!scan(&fx.core).cancelled);
}

#[test]
fn only_one_scan_runs_at_a_time_and_a_clean_waits_for_it() {
    let fx = fixture();
    let first = scan(&fx.core);
    let inner = RefCell::new(None);
    cleaner::scan_cleaning(&fx.core, &|progress| {
        if progress.done == 0 {
            *inner.borrow_mut() = Some((
                cleaner::scan_cleaning(&fx.core, &|_| {}),
                cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"])),
            ));
        }
    })
    .expect("scan");
    let (second_scan, clean) = inner.into_inner().expect("ran");
    assert!(matches!(second_scan, Err(IpcError::InvalidInput { .. })));
    assert!(matches!(clean, Err(IpcError::InvalidInput { .. })));
    assert!(fx.system.file_system.trashed().is_empty());
}

#[test]
fn the_recycle_bin_card_reads_the_bin_and_emptying_is_recorded_as_permanent() {
    let fx = fixture();
    let view = cleaner::recycle_bin_status(&fx.core).expect("status");
    assert!((view.item_count - 3.0).abs() < f64::EPSILON);
    assert!((view.size_bytes - 5_000_000.0).abs() < f64::EPSILON);
    assert_eq!(fx.bin.empty_calls(), 0, "reading never empties");

    let result = cleaner::empty_recycle_bin(&fx.core).expect("empty");
    assert!(result.emptied);
    assert!((result.item_count - 3.0).abs() < f64::EPSILON);
    assert_eq!(fx.bin.empty_calls(), 1);

    let records = fx.core.activity.list(10, None).expect("activity");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert!(records[0].summary.contains("permanently deleted"));
    assert!(records[0].summary.contains("cannot be undone"));

    // Nothing left to empty: no second call, no second record.
    let again = cleaner::empty_recycle_bin(&fx.core).expect("empty");
    assert!(!again.emptied);
    assert_eq!(fx.bin.empty_calls(), 1);
}

#[test]
fn a_failing_bin_is_an_error_the_ui_can_show() {
    let fx = fixture();
    fx.bin
        .fail_status_with(Some(PortError::Other("shell failed".into())));
    assert!(cleaner::recycle_bin_status(&fx.core).is_err());
    assert!(cleaner::empty_recycle_bin(&fx.core).is_err());
    assert_eq!(fx.bin.empty_calls(), 0);
}

#[test]
fn a_core_built_without_a_bin_never_reaches_the_real_one() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let core = build_core_with_cleaning(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders: fake_folders(),
        },
    )
    .expect("core builds");
    assert!(core.recycle_bin().is_none());
    assert!(matches!(
        cleaner::empty_recycle_bin(&core),
        Err(IpcError::Unsupported { .. })
    ));
    assert!(matches!(
        cleaner::recycle_bin_status(&core),
        Err(IpcError::Unsupported { .. })
    ));
}
