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
    assert_eq!(rules.len(), 40);
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
    assert_eq!(result.rules.len(), 40);

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
    assert_eq!(seen.len(), 40);
    assert_eq!(seen[0], CleanScanProgress { done: 0, total: 40 });
    assert_eq!(
        seen[10],
        CleanScanProgress {
            done: 10,
            total: 40
        }
    );
}

#[test]
fn cleaning_moves_only_the_chosen_rules_and_says_where_the_files_went() {
    let fx = fixture();
    let result = scan(&fx.core);
    let run =
        cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"]), &[]).expect("clean");

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

    let stale = cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"]), &[]);
    assert!(matches!(stale, Err(IpcError::NotFound { .. })), "{stale:?}");
    assert!(fx.system.file_system.trashed().is_empty());

    cleaner::clean_plan(&fx.core, &second.plan_id, &ids(&["user-temp"]), &[]).expect("clean");
    let again = cleaner::clean_plan(&fx.core, &second.plan_id, &ids(&["user-temp"]), &[]);
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
        &[],
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
    cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"]), &[]).expect("clean");
}

#[test]
fn a_clean_with_nothing_chosen_or_an_unknown_rule_is_refused() {
    let fx = fixture();
    let result = scan(&fx.core);
    let none = cleaner::clean_plan(&fx.core, &result.plan_id, &[], &[]);
    assert!(
        matches!(none, Err(IpcError::InvalidInput { .. })),
        "{none:?}"
    );
    let unknown = cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["no-such-rule"]), &[]);
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
    let run =
        cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"]), &[]).expect("clean");
    assert_eq!(run.moved_files, 1);
    assert_eq!(run.skipped_total, 1);
    assert_eq!(run.skipped.len(), 1);
    assert_eq!(run.skipped[0].reason, "locked, in use or access denied");
    assert_eq!(run.skipped[0].count, 1);
    assert_eq!(run.skipped[0].reason_phrase.code, "skip.locked");
    assert!(
        run.message.contains("1 item left in place"),
        "{}",
        run.message
    );
    let json = serde_json::to_string(&run).expect("json");
    assert!(!json.contains("tester"), "{json}");
}

#[test]
fn cancelling_a_scan_returns_what_it_reached_and_says_it_is_partial() {
    let fx = fixture();
    let first = scan(&fx.core);

    let cancelled = cleaner::scan_cleaning(&fx.core, &|progress| {
        if progress.done == 3 {
            cleaner::cancel_cleaning_scan(&fx.core);
        }
    })
    .expect("scan");
    assert!(cancelled.cancelled);
    assert!(!cancelled.plan_id.is_empty());
    // Rules 0 to 3 were started; the fourth was cancelled while it was walked (or before it was).
    assert!(cancelled.rules.len() <= 4 && !cancelled.rules.is_empty());

    // The partial scan replaced the earlier one, which is no longer cleanable.
    let stale = cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"]), &[]);
    assert!(matches!(stale, Err(IpcError::NotFound { .. })));
    // What the partial scan listed can be cleaned after the usual confirmation.
    cleaner::clean_plan(&fx.core, &cancelled.plan_id, &ids(&["user-temp"]), &[]).expect("clean");

    // A cancel with no scan running does not cancel the next one.
    cleaner::cancel_cleaning_scan(&fx.core);
    assert!(!scan(&fx.core).cancelled);
}

#[test]
fn a_scan_of_chosen_rules_looks_at_only_those_and_refuses_bad_input() {
    let fx = fixture();
    let progress = RefCell::new(Vec::new());
    let result = cleaner::scan_cleaning_rules(&fx.core, &ids(&["user-temp"]), &|p| {
        progress.borrow_mut().push(p);
    })
    .expect("scan");
    assert!(!result.cancelled && !result.plan_id.is_empty());
    assert_eq!(result.rules.len(), 1);
    assert_eq!(result.rules[0].rule.id, "user-temp");
    assert_eq!(result.rules[0].files, 2);
    assert_eq!(progress.borrow().last().map(|p| p.total), Some(1));
    // Nothing moved: it is a scan.
    assert!(fx.system.file_system.trashed().is_empty());
    cleaner::clean_plan(&fx.core, &result.plan_id, &ids(&["user-temp"]), &[]).expect("clean");

    let none = cleaner::scan_cleaning_rules(&fx.core, &[], &|_| {});
    assert!(matches!(none, Err(IpcError::InvalidInput { .. })));
    let unknown = cleaner::scan_cleaning_rules(&fx.core, &ids(&["no-such-rule"]), &|_| {});
    assert!(matches!(unknown, Err(IpcError::InvalidInput { .. })));
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
                cleaner::clean_plan(&fx.core, &first.plan_id, &ids(&["user-temp"]), &[]),
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

// ---- items, folders and picked items ----------------------------------------------------------

fn hub() -> PathBuf {
    abs("Users/tester/.cache/huggingface/hub")
}

fn with_models(fx: &Fixture) {
    let fs = &fx.system.file_system;
    fs.add_file_at(hub().join("models--org--big/blobs/a"), 6000, ancient());
    fs.add_file_at(hub().join("models--org--small/blobs/b"), 300, ancient());
}

#[test]
fn a_scan_lists_the_items_of_a_picked_rule_and_only_the_picked_ones_move() {
    let fx = fixture();
    with_models(&fx);
    let result = scan(&fx.core);
    let view = result
        .rules
        .iter()
        .find(|v| v.rule.id == "huggingface-hub-cache")
        .expect("rule");
    assert!(view.rule.pick_items);
    assert_eq!(view.rule.category, CleanCategoryView::AiModelCache);
    let names: Vec<&str> = view.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["models--org--big", "models--org--small"]);
    assert!((view.items[0].bytes - 6000.0).abs() < 0.5);
    assert_eq!(view.items[0].files, 1);
    assert_eq!(view.files, 2);
    // Items are named, never located: a name is not a path.
    assert!(view.items.iter().all(|i| !i.name.contains(['/', '\\'])));
    // Other rules list no items.
    assert!(
        result
            .rules
            .iter()
            .filter(|v| !v.rule.pick_items)
            .all(|v| v.items.is_empty())
    );

    let rule = ids(&["huggingface-hub-cache"]);
    // Choosing the rule without picking an item is refused, and so is an item that is not there
    // or items for a rule that is not picked.
    let none = cleaner::clean_plan(&fx.core, &result.plan_id, &rule, &[]);
    assert!(
        matches!(none, Err(IpcError::InvalidInput { .. })),
        "{none:?}"
    );
    let pick = |rule_id: &str, items: Vec<u32>| cleaner::ItemPick {
        rule_id: rule_id.to_owned(),
        items,
    };
    let bad = cleaner::clean_plan(
        &fx.core,
        &result.plan_id,
        &rule,
        &[pick("huggingface-hub-cache", vec![9])],
    );
    assert!(matches!(bad, Err(IpcError::InvalidInput { .. })), "{bad:?}");
    let stray = cleaner::clean_plan(
        &fx.core,
        &result.plan_id,
        &rule,
        &[pick("user-temp", vec![0])],
    );
    assert!(
        matches!(stray, Err(IpcError::InvalidInput { .. })),
        "{stray:?}"
    );
    assert!(fx.system.file_system.trashed().is_empty());

    let run = cleaner::clean_plan(
        &fx.core,
        &result.plan_id,
        &rule,
        &[pick("huggingface-hub-cache", vec![1])],
    )
    .expect("clean");
    assert_eq!(run.moved_files, 1);
    assert!((run.moved_bytes - 300.0).abs() < 0.5);
    assert_eq!(
        fx.system.file_system.trashed(),
        [hub().join("models--org--small")]
    );
    assert!(
        fx.system
            .file_system
            .exists(&hub().join("models--org--big/blobs/a"))
    );
}

#[test]
fn a_folder_is_added_for_a_rule_that_needs_one_and_removed_again() {
    let fx = fixture();
    let comfy = abs("AI/ComfyUI");
    fx.system.file_system.add_dir(&comfy, Some(ancient()));
    fx.system
        .file_system
        .add_file_at(comfy.join("temp/p.png"), 10, ancient());
    let listed = |fx: &Fixture| {
        cleaner::list_cleaning_rules(&fx.core)
            .expect("rules")
            .into_iter()
            .find(|r| r.id == "comfyui-temp")
            .expect("rule")
    };
    let before = listed(&fx);
    assert!(before.needs_folder);
    assert_eq!(before.chosen_folder, None);
    let waiting = scan(&fx.core);
    let view = waiting
        .rules
        .iter()
        .find(|v| v.rule.id == "comfyui-temp")
        .expect("rule");
    assert_eq!(view.problem, None, "waiting for a folder is not a problem");
    assert_eq!(view.files, 0);

    // Refused: a rule that needs no folder, a folder that is not there, an output folder.
    for (rule, folder) in [
        ("user-temp", comfy.to_string_lossy().into_owned()),
        (
            "comfyui-temp",
            abs("AI/Missing").to_string_lossy().into_owned(),
        ),
    ] {
        let refused = cleaner::choose_cleaning_folder(&fx.core, rule, &folder);
        assert!(
            matches!(refused, Err(IpcError::InvalidInput { .. })),
            "{rule}: {refused:?}"
        );
    }
    fx.system.file_system.add_dir(comfy.join("output"), None);
    let output = cleaner::choose_cleaning_folder(
        &fx.core,
        "comfyui-temp",
        &comfy.join("output").to_string_lossy(),
    );
    assert!(
        matches!(output, Err(IpcError::InvalidInput { .. })),
        "{output:?}"
    );

    cleaner::choose_cleaning_folder(&fx.core, "comfyui-temp", &comfy.to_string_lossy())
        .expect("chosen");
    assert_eq!(
        listed(&fx).chosen_folder.as_deref(),
        Some(comfy.to_string_lossy().as_ref())
    );
    let found = scan(&fx.core);
    let view = found
        .rules
        .iter()
        .find(|v| v.rule.id == "comfyui-temp")
        .expect("rule");
    assert_eq!(view.files, 1);
    assert_eq!(view.samples, ["chosen_folder/temp/p.png"]);

    cleaner::forget_cleaning_folder(&fx.core, "comfyui-temp").expect("forgotten");
    assert_eq!(listed(&fx).chosen_folder, None);
    assert!(fx.system.file_system.exists(&comfy.join("temp/p.png")));
}
