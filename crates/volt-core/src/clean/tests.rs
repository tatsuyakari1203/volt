//! Tests of the cleaning engine against in-memory fakes. Nothing here touches
//! the real file system.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::fakes::FakeKnownFolders;
use super::*;
use crate::activity::ActivityOutcome;
use crate::error::PortError;
use crate::fakes::{FakeActivityLog, FakeFileSystem, FakeSettings, ManualClock};
use crate::journal::Source;
use crate::label::Risk;
use crate::ports::FileSystem;
use crate::settings::Settings;
use crate::time::Timestamp;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

fn abs(rest: &str) -> PathBuf {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    Path::new(base).join(rest)
}

fn ago(days: i64) -> Timestamp {
    Timestamp::from_unix_millis(NOW - days * DAY)
}

fn temp() -> PathBuf {
    abs("Users/tester/AppData/Local/Temp")
}

struct Fixture {
    fs: Arc<FakeFileSystem>,
    folders: Arc<FakeKnownFolders>,
    activity: Arc<FakeActivityLog>,
    clock: Arc<ManualClock>,
}

impl Fixture {
    fn new() -> Self {
        let folders = Arc::new(FakeKnownFolders::new());
        folders.set(Location::UserTemp, temp());
        folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
        folders.protect(abs("Users/tester"));
        folders.protect(abs("Windows"));
        folders.protect(abs("Users/tester/Documents"));
        Self {
            fs: Arc::new(FakeFileSystem::new()),
            folders,
            activity: Arc::new(FakeActivityLog::new()),
            clock: Arc::new(ManualClock::new(NOW)),
        }
    }

    fn engine(&self, toml: &str) -> CleanEngine {
        CleanEngine::new(
            CleaningRules::from_toml(toml).expect("valid rules"),
            self.folders.clone(),
            self.fs.clone(),
            self.clock.clone(),
        )
    }

    fn logged_engine(&self, toml: &str) -> CleanEngine {
        self.engine(toml).with_activity_log(self.activity.clone())
    }
}

/// A rule over the temp folder with the given constraints and patterns.
fn temp_rule(id: &str, include: &str, exclude: &str, constraints: &str) -> String {
    format!(
        r#"
[[rule]]
id = "{id}"
title = "Temp files"
description = "Disposable files in the temp folder."
category = "temporary_files"
risk = "safe"
include = {include}
exclude = {exclude}

[rule.root]
location = "user_temp"

[rule.constraints]
{constraints}
"#
    )
}

fn default_rule() -> String {
    temp_rule(
        "temp-files",
        r#"["**"]"#,
        r#"["keep/**", "**/*.lock"]"#,
        "min_age_days = 1\nmax_depth = 3\nremove_empty_dirs = false",
    )
}

fn only(plan: &CleanPlan) -> &RuleScan {
    assert_eq!(plan.rules.len(), 1);
    &plan.rules[0]
}

fn paths(scan: &RuleScan) -> Vec<PathBuf> {
    scan.items.iter().map(|item| item.path.clone()).collect()
}

// ---- loading rules ---------------------------------------------------------------------------

#[test]
fn a_valid_rule_loads_with_every_field() {
    let rules = CleaningRules::from_toml(
        r#"
[[rule]]
id = "browser-cache"
title = "Browser cache"
description = "Cached pages."
category = "browser_cache"
risk = "caution"
include = ["**/*.cache", "data_*"]
exclude = ["profile/**"]
root = { location = "local_app_data", path = "Vendor/Browser/Cache" }
constraints = { min_age_days = 7, max_depth = 5, remove_empty_dirs = true }
"#,
    )
    .expect("loads");
    let rule = rules.get("browser-cache").expect("rule");
    assert_eq!(rule.category, CleanCategory::BrowserCache);
    assert_eq!(rule.risk, Risk::Caution);
    assert_eq!(rule.root.location(), Location::LocalAppData);
    assert_eq!(rule.root.relative(), ["Vendor", "Browser", "Cache"]);
    assert_eq!(rule.root.to_string(), "local_app_data/Vendor/Browser/Cache");
    assert_eq!(rule.include.len(), 2);
    assert_eq!(rule.exclude[0].as_str(), "profile/**");
    assert_eq!(
        rule.constraints,
        Constraints {
            min_age_days: 7,
            max_depth: 5,
            remove_empty_dirs: true
        }
    );
    assert_eq!(rules.len(), 1);
}

#[test]
fn the_temp_folder_itself_is_a_valid_root_and_empty_dirs_default_to_kept() {
    let rules = CleaningRules::from_toml(&temp_rule(
        "t",
        r#"["**"]"#,
        "[]",
        "min_age_days = 0\nmax_depth = 0",
    ))
    .expect("loads");
    let rule = rules.get("t").expect("rule");
    assert!(rule.root.relative().is_empty());
    assert!(!rule.constraints.remove_empty_dirs);
}

fn issues_of(toml: &str) -> Vec<(Option<String>, String, RuleIssueKind)> {
    CleaningRules::from_toml(toml)
        .expect_err("must be rejected")
        .issues
        .into_iter()
        .map(|i| (i.rule, i.field, i.kind))
        .collect()
}

fn one_rule(root: &str, include: &str, constraints: &str) -> String {
    format!(
        r#"
[[rule]]
id = "r"
title = "t"
description = "d"
category = "logs"
risk = "safe"
include = {include}
root = {root}
constraints = {constraints}
"#
    )
}

const OK_CONSTRAINTS: &str = "{ min_age_days = 1, max_depth = 2 }";

#[test]
fn dangerous_roots_are_rejected_at_load() {
    let local = |path: &str| format!(r#"{{ location = "local_app_data", path = '{path}' }}"#);
    for (path, why) in [
        ("C:/Windows", "drive"),
        ("C:", "drive"),
        ("/Windows", "absolute"),
        ("\\Windows", "backslash"),
        ("Cache/../../Documents", "parent"),
        ("..", "parent"),
        ("", "whole location"),
        ("Cache/", "trailing slash"),
        ("Cache//x", "doubled slash"),
        ("./Cache", "dot"),
        ("Cache.", "trailing dot"),
        ("Ca*che", "wildcard"),
        ("a\\b", "backslash"),
    ] {
        let issues = issues_of(&one_rule(&local(path), r#"["**"]"#, OK_CONSTRAINTS));
        assert_eq!(issues.len(), 1, "{path:?} ({why}): {issues:?}");
        assert_eq!(issues[0].1, "root.path", "{path:?}");
        assert_eq!(issues[0].0.as_deref(), Some("r"));
    }
    // The classes of refusal are distinguished.
    let kind = |path: &str| {
        issues_of(&one_rule(&local(path), r#"["**"]"#, OK_CONSTRAINTS))
            .remove(0)
            .2
    };
    assert!(matches!(kind("C:/Windows"), RuleIssueKind::UnsafeRoot(_)));
    assert!(matches!(kind("/x"), RuleIssueKind::UnsafeRoot(_)));
    assert!(matches!(kind("a/../b"), RuleIssueKind::UnsafeRoot(_)));
    assert!(matches!(kind(""), RuleIssueKind::UnsafeRoot(_)));
    assert!(matches!(kind("a//b"), RuleIssueKind::Malformed(_)));
}

#[test]
fn every_problem_of_a_file_is_reported_with_rule_id_and_field() {
    let issues = issues_of(
        r#"
extra = 1

[[rule]]
id = "Bad_Id"
title = " "
description = "d"
category = "nonsense"
risk = "safe"
include = []
root = { location = "nowhere", path = "x" }
constraints = { min_age_days = -1, max_depth = 99, unknown = 1 }
colour = "red"

[[rule]]
id = "dup"
title = "t"
description = "d"
category = "logs"
risk = "safe"
include = ["../x", "[a]", 5]
root = { location = "user_temp" }
constraints = { min_age_days = 1, max_depth = 1 }

[[rule]]
id = "dup"
title = "t"
description = "d"
category = "logs"
risk = "safe"
include = ["**"]
root = { location = "user_temp" }
"#,
    );
    let has = |rule: Option<&str>, field: &str| {
        issues
            .iter()
            .any(|(r, f, _)| r.as_deref() == rule && f == field)
    };
    assert!(has(None, "extra"), "{issues:?}");
    assert!(has(Some("Bad_Id"), "id"));
    assert!(has(Some("Bad_Id"), "title"));
    assert!(has(Some("Bad_Id"), "category"));
    assert!(has(Some("Bad_Id"), "include"));
    assert!(has(Some("Bad_Id"), "root.location"));
    assert!(has(Some("Bad_Id"), "constraints.min_age_days"));
    assert!(has(Some("Bad_Id"), "constraints.max_depth"));
    assert!(has(Some("Bad_Id"), "constraints.unknown"));
    assert!(has(Some("Bad_Id"), "colour"));
    assert!(has(Some("dup"), "include[0]"));
    assert!(has(Some("dup"), "include[1]"));
    assert!(has(Some("dup"), "include[2]"));
    // The second `dup` has no constraints and repeats the id.
    assert!(has(Some("dup"), "constraints"));
    assert!(issues.iter().any(
        |(_, f, k)| f == "id" && matches!(k, RuleIssueKind::DuplicateId { first_position: 2 })
    ));
    assert!(
        issues
            .iter()
            .any(|(_, f, k)| f == "include[0]" && matches!(k, RuleIssueKind::UnsafePattern(_)))
    );
    assert!(
        issues
            .iter()
            .any(|(_, f, k)| f == "include[1]" && matches!(k, RuleIssueKind::Malformed(_)))
    );
}

#[test]
fn a_rule_without_constraints_or_with_an_empty_include_is_rejected() {
    let missing = issues_of(
        r#"
[[rule]]
id = "r"
title = "t"
description = "d"
category = "logs"
risk = "safe"
include = ["**"]
root = { location = "user_temp" }
"#,
    );
    assert!(
        missing
            .iter()
            .any(|(_, f, k)| f == "constraints" && *k == RuleIssueKind::MissingField)
    );
    let partial = issues_of(&one_rule(
        r#"{ location = "user_temp" }"#,
        "[]",
        "{ min_age_days = 1 }",
    ));
    assert!(
        partial
            .iter()
            .any(|(_, f, k)| f == "include" && *k == RuleIssueKind::Empty)
    );
    assert!(
        partial
            .iter()
            .any(|(_, f, k)| f == "constraints.max_depth" && *k == RuleIssueKind::MissingField)
    );
}

#[test]
fn invalid_toml_is_one_syntax_issue() {
    let error = CleaningRules::from_toml("[[rule]\nid =").expect_err("syntax");
    assert_eq!(error.issues.len(), 1);
    assert!(matches!(error.issues[0].kind, RuleIssueKind::Syntax { .. }));
    assert!(error.to_string().contains("1 problem"));
}

// ---- scanning --------------------------------------------------------------------------------

#[test]
fn a_scan_lists_files_with_sizes_and_totals_and_changes_nothing() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 100, ago(3));
    fx.fs.add_file_at(temp().join("sub/b.tmp"), 250, ago(5));
    fx.fs
        .add_file_at(temp().join("sub/deeper/c.tmp"), 7, ago(2));
    let engine = fx.logged_engine(&default_rule());

    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);

    assert_eq!(scan.failure, None);
    assert!(scan.root_exists);
    assert_eq!(scan.root.as_deref(), Some(temp().as_path()));
    assert_eq!(scan.file_count(), 3);
    assert_eq!(scan.total_bytes(), 357);
    assert_eq!(plan.total_files(), 3);
    assert_eq!(plan.total_bytes(), 357);
    assert!(!plan.is_empty());
    assert_eq!(scan.rule_id, "temp-files");
    assert_eq!(scan.risk, Risk::Safe);
    assert_eq!(scan.category, CleanCategory::TemporaryFiles);
    let a = &scan.items[0];
    assert_eq!(
        (a.kind, a.size_bytes, a.modified),
        (ItemKind::File, 100, Some(ago(3)))
    );
    // Read-only: no file was moved, no record was written.
    assert!(fx.fs.trashed().is_empty());
    assert!(fx.activity.records().is_empty());
    assert!(fx.fs.exists(&temp().join("a.tmp")));
}

#[test]
fn a_scan_is_deterministic_and_sorted() {
    let fx = Fixture::new();
    for name in ["c.tmp", "a.tmp", "b.tmp"] {
        fx.fs.add_file_at(temp().join(name), 1, ago(2));
    }
    let engine = fx.engine(&default_rule());
    let first = paths(only(&engine.scan(&["temp-files"]).expect("scan")));
    let second = paths(only(&engine.scan_all()));
    assert_eq!(first, second);
    assert_eq!(
        first,
        [
            temp().join("a.tmp"),
            temp().join("b.tmp"),
            temp().join("c.tmp")
        ]
    );
}

#[test]
fn exclude_patterns_keep_files_and_folders_out_and_folders_are_not_entered() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("a.lock"), 1, ago(2));
    fx.fs
        .add_file_at(temp().join("keep/precious.dat"), 1, ago(9));
    fx.fs.add_file_at(temp().join("other/x.LOCK"), 1, ago(2));
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(paths(scan), [temp().join("a.tmp")]);
    // `keep` and the two lock files (matching ignores case).
    assert_eq!(scan.counts.excluded, 3);
    assert!(
        !fx.fs.read_dir_calls().contains(&temp().join("keep")),
        "an excluded folder must not be entered"
    );
}

#[test]
fn include_patterns_decide_what_is_taken() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("a.txt"), 1, ago(2));
    fx.fs.add_file_at(temp().join("sub/b.tmp"), 1, ago(2));
    let engine = fx.engine(&temp_rule(
        "only-tmp",
        r#"["*.tmp"]"#,
        "[]",
        "min_age_days = 0\nmax_depth = 3",
    ));
    let plan = engine.scan(&["only-tmp"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(paths(scan), [temp().join("a.tmp")]);
    assert_eq!(scan.counts.not_matching, 2);
}

#[test]
fn files_newer_than_the_minimum_age_or_without_a_time_are_skipped_with_a_reason() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("old.tmp"), 1, ago(2));
    fx.fs.add_file_at(
        temp().join("new.tmp"),
        1,
        Timestamp::from_unix_millis(NOW - DAY / 2),
    );
    fx.fs.add_file_at(
        temp().join("future.tmp"),
        1,
        Timestamp::from_unix_millis(NOW + DAY),
    );
    fx.fs.add_file(temp().join("unknown.tmp"), 1);
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(paths(scan), [temp().join("old.tmp")]);
    let reason_of = |name: &str| {
        scan.skipped
            .iter()
            .find(|s| s.path == temp().join(name))
            .map(|s| s.reason)
    };
    assert_eq!(reason_of("new.tmp"), Some(SkipReason::TooNew));
    assert_eq!(reason_of("future.tmp"), Some(SkipReason::TooNew));
    assert_eq!(reason_of("unknown.tmp"), Some(SkipReason::AgeUnknown));
    assert_eq!(scan.counts.too_new, 2);
    assert_eq!(scan.counts.age_unknown, 1);
}

#[test]
fn a_file_exactly_as_old_as_the_minimum_is_taken() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("edge.tmp"), 1, ago(1));
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    assert_eq!(paths(only(&plan)), [temp().join("edge.tmp")]);
}

#[test]
fn max_depth_limits_how_deep_a_scan_goes() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("d1/b.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("d1/d2/c.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("d1/d2/d3/d.tmp"), 1, ago(2));
    let at = |depth: u32| {
        let engine = fx.engine(&temp_rule(
            "r",
            r#"["**"]"#,
            "[]",
            &format!("min_age_days = 0\nmax_depth = {depth}"),
        ));
        let plan = engine.scan(&["r"]).expect("scan");
        let scan = only(&plan);
        (
            scan.file_count(),
            scan.counts.beyond_max_depth,
            scan.skipped.clone(),
        )
    };
    assert_eq!(at(0).0, 1);
    assert_eq!(at(0).1, 1);
    assert_eq!(at(1).0, 2);
    assert_eq!(at(2).0, 3);
    let (files, beyond, skipped) = at(2);
    assert_eq!((files, beyond), (3, 1));
    assert_eq!(skipped[0].path, temp().join("d1/d2/d3"));
    assert_eq!(skipped[0].reason, SkipReason::BeyondMaxDepth);
    assert_eq!(at(3).0, 4);
}

#[test]
fn a_scan_never_follows_a_link_and_never_yields_anything_outside_the_root() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    // A junction inside the root pointing at precious data elsewhere: the fake
    // shows the target's files below the link, as a real junction would.
    let precious = abs("Users/tester/Documents");
    fx.fs.add_file_at(precious.join("thesis.docx"), 9, ago(900));
    fx.fs
        .add_file_at(temp().join("link/thesis.docx"), 9, ago(900));
    fx.fs.add_link(temp().join("link"));
    // A sibling folder outside the root.
    fx.fs
        .add_file_at(abs("Users/tester/AppData/Local/Other/x.tmp"), 5, ago(9));

    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);

    assert_eq!(paths(scan), [temp().join("a.tmp")]);
    assert_eq!(scan.counts.links, 1);
    assert_eq!(scan.skipped.len(), 1);
    assert_eq!(scan.skipped[0].path, temp().join("link"));
    assert_eq!(scan.skipped[0].reason, SkipReason::Link);
    assert!(
        fx.fs
            .read_dir_calls()
            .iter()
            .all(|dir| dir.starts_with(temp()) && !dir.starts_with(temp().join("link"))),
        "only folders of the root were listed, never the link: {:?}",
        fx.fs.read_dir_calls()
    );
    // Everything planned lies inside the root.
    assert!(scan.items.iter().all(|i| i.path.starts_with(temp())));
}

#[test]
fn a_link_is_never_taken_even_if_it_matches_the_rule() {
    let fx = Fixture::new();
    fx.fs.add_link(temp().join("cache.tmp"));
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    assert!(only(&plan).items.is_empty());
    assert_eq!(only(&plan).counts.links, 1);
}

#[test]
fn a_root_that_is_a_link_or_a_file_is_refused_and_a_missing_root_is_just_empty() {
    let fx = Fixture::new();
    let engine = fx.engine(&default_rule());

    let plan = engine.scan(&["temp-files"]).expect("scan");
    assert!(!only(&plan).root_exists);
    assert_eq!(only(&plan).failure, None);
    assert!(plan.is_empty());

    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    fx.fs.add_link(temp());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    assert_eq!(only(&plan).failure, Some(ScanFailure::RootIsLink));
    assert!(only(&plan).items.is_empty());

    let fx = Fixture::new();
    fx.fs.add_file(temp(), 3);
    let plan = fx
        .engine(&default_rule())
        .scan(&["temp-files"])
        .expect("scan");
    assert_eq!(only(&plan).failure, Some(ScanFailure::RootNotDirectory));
}

#[test]
fn roots_that_resolve_to_broad_or_protected_folders_are_refused_at_scan() {
    let engine_with = |temp_dir: PathBuf| {
        let fx = Fixture::new();
        fx.folders.set(Location::UserTemp, temp_dir.clone());
        fx.fs.add_file_at(temp_dir.join("a.tmp"), 1, ago(9));
        let plan = fx
            .engine(&default_rule())
            .scan(&["temp-files"])
            .expect("scan");
        (only(&plan).failure.clone(), only(&plan).items.len())
    };
    // The drive root and a top-level folder.
    for broad in [abs(""), abs("Windows"), abs("Temp"), abs("Users")] {
        assert_eq!(
            engine_with(broad.clone()),
            (Some(ScanFailure::Root(RootError::TooBroad)), 0),
            "{broad:?}"
        );
    }
    // The profile directory and a protected folder inside it; case does not
    // matter. (A folder above the profile is a top-level folder, refused above.)
    for protected in [
        abs("Users/tester"),
        abs("USERS/Tester"),
        abs("Users/tester/documents"),
    ] {
        let (failure, items) = engine_with(protected.clone());
        assert!(
            matches!(failure, Some(ScanFailure::Root(RootError::Protected(_)))),
            "{protected:?}: {failure:?}"
        );
        assert_eq!(items, 0);
    }
    // A folder below a protected one is fine (the real temp folder is).
    assert_eq!(engine_with(temp()), (None, 1));
}

#[test]
fn an_unresolvable_relative_or_failing_location_is_refused() {
    let fx = Fixture::new();
    let engine = fx.engine(&default_rule());

    fx.folders.set(Location::UserTemp, "relative/temp");
    assert_eq!(
        only(&engine.scan(&["temp-files"]).expect("scan")).failure,
        Some(ScanFailure::Root(RootError::NotAbsolute))
    );

    fx.folders.set(Location::UserTemp, temp().join("a/../.."));
    assert_eq!(
        only(&engine.scan(&["temp-files"]).expect("scan")).failure,
        Some(ScanFailure::Root(RootError::TooBroad))
    );

    let fx = Fixture::new();
    fx.folders
        .fail_protected_with(Some(PortError::Other("boom".to_owned())));
    assert_eq!(
        only(
            &fx.engine(&default_rule())
                .scan(&["temp-files"])
                .expect("scan")
        )
        .failure,
        Some(ScanFailure::Root(RootError::Unavailable("error")))
    );

    let engine = Fixture::new().engine(
        r#"
[[rule]]
id = "r"
title = "t"
description = "d"
category = "logs"
risk = "safe"
include = ["**"]
root = { location = "program_data", path = "Vendor/Logs" }
constraints = { min_age_days = 0, max_depth = 1 }
"#,
    );
    assert_eq!(
        only(&engine.scan(&["r"]).expect("scan")).failure,
        Some(ScanFailure::Root(RootError::Unavailable("not found")))
    );
}

#[test]
fn a_rule_with_a_relative_path_resolves_below_its_location() {
    let fx = Fixture::new();
    let cache = abs("Users/tester/AppData/Local/Vendor/Cache");
    fx.fs.add_file_at(cache.join("x.bin"), 4, ago(3));
    let engine = fx.engine(
        r#"
[[rule]]
id = "vendor-cache"
title = "t"
description = "d"
category = "application_cache"
risk = "safe"
include = ["*.bin"]
root = { location = "local_app_data", path = "Vendor/Cache" }
constraints = { min_age_days = 1, max_depth = 0 }
"#,
    );
    let plan = engine.scan_all();
    assert_eq!(only(&plan).root.as_deref(), Some(cache.as_path()));
    assert_eq!(only(&plan).total_bytes(), 4);
}

#[test]
fn empty_folders_are_planned_children_first_and_only_when_nothing_is_left_in_them() {
    let fx = Fixture::new();
    // `gone/inner` holds only a file that will be taken: both folders empty out.
    fx.fs
        .add_file_at(temp().join("gone/inner/x.tmp"), 1, ago(3));
    // `stays` holds a file that is too new: the folder must stay.
    fx.fs.add_file_at(
        temp().join("stays/new.tmp"),
        1,
        Timestamp::from_unix_millis(NOW - 1000),
    );
    // `empty` is empty already.
    fx.fs.add_dir(temp().join("empty"), Some(ago(4)));
    // `fresh` is empty but too new.
    fx.fs.add_dir(temp().join("fresh"), Some(ago(0)));
    // `with-link` holds only a link.
    fx.fs.add_link(temp().join("with-link/l"));
    // `keep` is excluded, even though empty.
    fx.fs.add_dir(temp().join("keep"), Some(ago(9)));
    // Folders carry a modification time that is old enough.
    fx.fs.add_dir(temp().join("gone"), Some(ago(3)));
    fx.fs.add_dir(temp().join("gone/inner"), Some(ago(3)));
    fx.fs.add_dir(temp().join("stays"), Some(ago(3)));
    fx.fs.add_dir(temp().join("with-link"), Some(ago(3)));

    let engine = fx.engine(&temp_rule(
        "r",
        r#"["**"]"#,
        r#"["keep"]"#,
        "min_age_days = 1\nmax_depth = 4\nremove_empty_dirs = true",
    ));
    let plan = engine.scan(&["r"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(
        paths(scan),
        [
            temp().join("empty"),
            temp().join("gone/inner/x.tmp"),
            temp().join("gone/inner"),
            temp().join("gone"),
        ]
    );
    assert_eq!(scan.file_count(), 1);
    assert_eq!(scan.directory_count(), 3);
    assert_eq!(scan.items[0].kind, ItemKind::EmptyDirectory);
    assert_eq!(scan.total_bytes(), 1);
}

#[test]
fn folders_are_never_planned_when_the_rule_keeps_them() {
    let fx = Fixture::new();
    fx.fs.add_dir(temp().join("empty"), Some(ago(4)));
    let engine = fx.engine(&default_rule());
    assert!(
        only(&engine.scan(&["temp-files"]).expect("scan"))
            .items
            .is_empty()
    );
}

#[test]
fn an_unreadable_folder_is_reported_and_does_not_stop_the_scan() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("locked/b.tmp"), 1, ago(2));
    fx.fs.add_file_at(temp().join("ok/c.tmp"), 1, ago(2));
    fx.fs.fail_read_dir_of(
        temp().join("locked"),
        PortError::AccessDenied("x".to_owned()),
    );
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(paths(scan), [temp().join("a.tmp"), temp().join("ok/c.tmp")]);
    assert_eq!(scan.counts.access_denied, 1);
    assert_eq!(scan.counts.unreadable, 0);
    assert_eq!(
        scan.skipped[0],
        SkippedEntry {
            path: temp().join("locked"),
            reason: SkipReason::AccessDenied,
        }
    );
}

#[test]
fn a_folder_unreadable_for_another_reason_keeps_its_error_class() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("locked/b.tmp"), 1, ago(2));
    fx.fs
        .fail_read_dir_of(temp().join("locked"), PortError::Other("boom".to_owned()));
    let plan = fx
        .engine(&default_rule())
        .scan(&["temp-files"])
        .expect("scan");
    let scan = only(&plan);
    assert_eq!(scan.counts.unreadable, 1);
    assert_eq!(scan.counts.access_denied, 0);
    assert_eq!(scan.skipped[0].reason, SkipReason::Unreadable("error"));
}

#[test]
fn skipped_details_are_capped_but_the_counts_stay_exact() {
    let fx = Fixture::new();
    let total = MAX_SKIPPED_DETAILS + 100;
    for i in 0..total {
        fx.fs
            .add_file_at(temp().join(format!("new{i}.tmp")), 1, ago(0));
    }
    let engine = fx.engine(&default_rule());
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(scan.skipped.len(), MAX_SKIPPED_DETAILS);
    assert_eq!(scan.counts.too_new, total);
    assert!(scan.items.is_empty());
}

#[test]
fn scanning_an_unknown_rule_is_an_error_and_an_empty_selection_is_an_empty_plan() {
    let fx = Fixture::new();
    let engine = fx.engine(&default_rule());
    assert_eq!(
        engine.scan(&["temp-files", "nope"]).expect_err("unknown"),
        UnknownRule("nope".to_owned())
    );
    let none: [&str; 0] = [];
    assert!(engine.scan(&none).expect("scan").rules.is_empty());
}

// ---- cleaning --------------------------------------------------------------------------------

fn engine_and_plan(fx: &Fixture, toml: &str) -> (CleanEngine, CleanPlan) {
    let engine = fx.logged_engine(toml);
    let plan = engine.scan_all();
    (engine, plan)
}

#[test]
fn cleaning_moves_exactly_the_planned_files_to_the_recycle_bin_and_reports_honestly() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1000, ago(3));
    fx.fs.add_file_at(temp().join("sub/b.tmp"), 24, ago(3));
    fx.fs.add_file_at(temp().join("a.lock"), 5, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    // A file that appears after the scan is not in the plan and must stay.
    fx.fs.add_file_at(temp().join("late.tmp"), 1, ago(5));

    let report = engine.clean(&plan.confirm(), Source::Manual);

    assert_eq!(report.moved_files(), 2);
    assert_eq!(report.moved_directories(), 0);
    assert_eq!(report.moved_bytes(), 1024);
    assert_eq!(report.skipped_count(), 0);
    assert_eq!(
        fx.fs.trashed(),
        [temp().join("a.tmp"), temp().join("sub/b.tmp")]
    );
    assert!(fx.fs.exists(&temp().join("late.tmp")));
    assert!(fx.fs.exists(&temp().join("a.lock")));
    // The text names the figure honestly: moved, not freed.
    let text = report.describe();
    assert!(
        text.contains("Moved 2 files to the Recycle Bin (1.0 KB)"),
        "{text}"
    );
    assert!(text.contains("until the Recycle Bin is emptied"), "{text}");
    assert!(!text.to_lowercase().contains("freed."), "{text}");
}

#[test]
fn an_item_that_changed_since_the_scan_is_skipped_and_left_alone() {
    let fx = Fixture::new();
    for name in ["same", "size", "time", "gone", "link", "dir"] {
        fx.fs
            .add_file_at(temp().join(format!("{name}.tmp")), 10, ago(3));
    }
    let (engine, plan) = engine_and_plan(&fx, &default_rule());

    fx.fs.add_file_at(temp().join("size.tmp"), 11, ago(3));
    fx.fs.add_file_at(temp().join("time.tmp"), 10, ago(1));
    fx.fs
        .move_to_trash(&temp(), &temp().join("gone.tmp"))
        .expect("trash");
    fx.fs.add_link(temp().join("link.tmp"));
    fx.fs.add_dir(temp().join("dir.tmp"), Some(ago(3)));

    let report = engine.clean(&plan.confirm(), Source::Manual);
    let rule = &report.rules[0];
    let reason_of = |name: &str| {
        rule.skipped
            .iter()
            .find(|s| s.path == temp().join(format!("{name}.tmp")))
            .map(|s| s.reason)
    };
    assert_eq!(reason_of("size"), Some(CleanSkipReason::Changed));
    assert_eq!(reason_of("time"), Some(CleanSkipReason::Changed));
    assert_eq!(reason_of("gone"), Some(CleanSkipReason::Gone));
    assert_eq!(reason_of("link"), Some(CleanSkipReason::Link));
    assert_eq!(reason_of("dir"), Some(CleanSkipReason::Changed));
    assert_eq!(reason_of("same"), None);
    assert_eq!(rule.moved.len(), 1);
    assert_eq!(rule.moved[0].path, temp().join("same.tmp"));
    // `gone.tmp` was trashed by the test itself, nothing else was moved.
    assert_eq!(
        fx.fs.trashed(),
        [temp().join("gone.tmp"), temp().join("same.tmp")]
    );
    for name in ["size", "time", "link", "dir"] {
        assert!(fx.fs.exists(&temp().join(format!("{name}.tmp"))));
    }
}

#[test]
fn a_folder_that_became_a_link_after_the_scan_is_not_followed_by_the_clean() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("sub/a.tmp"), 10, ago(3));
    fx.fs.add_file_at(temp().join("sub/b.tmp"), 10, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    assert_eq!(only(&plan).file_count(), 2);

    // Swapped for a junction after the scan; the target's files show below it.
    fx.fs.add_link(temp().join("sub"));

    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert!(
        report.rules[0]
            .skipped
            .iter()
            .all(|s| s.reason == CleanSkipReason::Link)
    );
    assert_eq!(report.rules[0].skipped.len(), 2);
    assert!(fx.fs.trashed().is_empty());
    assert!(fx.fs.exists(&temp().join("sub/a.tmp")));
}

#[test]
fn a_locked_file_is_skipped_reported_and_tried_exactly_once() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    fx.fs.add_file_at(temp().join("locked.tmp"), 1, ago(3));
    fx.fs.add_file_at(temp().join("z.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.fs.fail_trash_of(
        temp().join("locked.tmp"),
        PortError::AccessDenied("in use".to_owned()),
    );

    let report = engine.clean(&plan.confirm(), Source::Manual);

    assert_eq!(report.moved_files(), 2);
    assert_eq!(
        report.rules[0].skipped,
        [CleanSkip {
            path: temp().join("locked.tmp"),
            reason: CleanSkipReason::LockedOrDenied
        }]
    );
    assert_eq!(
        fx.fs
            .trash_attempts()
            .iter()
            .filter(|p| **p == temp().join("locked.tmp"))
            .count(),
        1,
        "no retry loop"
    );
    assert!(fx.fs.exists(&temp().join("locked.tmp")));
    assert!(report.describe().contains("1 item left in place"));
}

#[test]
fn other_port_failures_are_classified_without_their_text() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    fx.fs.add_file_at(temp().join("b.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.fs.fail_trash_of(
        temp().join("a.tmp"),
        PortError::Other("secret detail".to_owned()),
    );
    fx.fs.fail_trash_of(
        temp().join("b.tmp"),
        PortError::InvalidInput("outside".to_owned()),
    );
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(
        report.rules[0]
            .skipped
            .iter()
            .map(|s| s.reason)
            .collect::<Vec<_>>(),
        [CleanSkipReason::Failed("error"), CleanSkipReason::Refused]
    );
    assert!(!format!("{report:?}").contains("secret detail"));
}

#[test]
fn empty_folders_are_moved_after_their_files_and_only_while_still_empty() {
    let fx = Fixture::new();
    fx.fs.add_dir(temp().join("a"), Some(ago(3)));
    fx.fs.add_dir(temp().join("b"), Some(ago(3)));
    fx.fs.add_file_at(temp().join("a/x.tmp"), 8, ago(3));
    fx.fs.add_file_at(temp().join("b/y.tmp"), 8, ago(3));
    let toml = temp_rule(
        "r",
        r#"["**"]"#,
        "[]",
        "min_age_days = 1\nmax_depth = 2\nremove_empty_dirs = true",
    );
    let (engine, plan) = engine_and_plan(&fx, &toml);
    assert_eq!(only(&plan).items.len(), 4);
    // Something new appears in `b` after the scan.
    fx.fs.add_file_at(temp().join("b/new.dat"), 1, ago(0));

    let report = engine.clean(&plan.confirm(), Source::Manual);

    assert_eq!(report.moved_files(), 2);
    assert_eq!(report.moved_directories(), 1);
    assert_eq!(
        fx.fs.trashed(),
        [
            temp().join("a/x.tmp"),
            temp().join("a"),
            temp().join("b/y.tmp")
        ]
    );
    let skipped = &report.rules[0].skipped;
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].path, temp().join("b"));
    assert_eq!(skipped[0].reason, CleanSkipReason::NotEmpty);
    assert!(fx.fs.exists(&temp().join("b/new.dat")));
    assert!(report.describe().contains("2 files and 1 empty folder"));
}

#[test]
fn confirming_a_subset_cleans_only_those_rules() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    fx.fs.add_file_at(temp().join("b.log"), 1, ago(3));
    let toml = format!(
        "{}{}",
        temp_rule(
            "tmp",
            r#"["*.tmp"]"#,
            "[]",
            "min_age_days = 0\nmax_depth = 0"
        ),
        temp_rule(
            "logs",
            r#"["*.log"]"#,
            "[]",
            "min_age_days = 0\nmax_depth = 0"
        ),
    );
    let (engine, plan) = engine_and_plan(&fx, &toml);
    assert_eq!(plan.rules.len(), 2);

    let confirmed = plan.confirm_rules(&["logs", "unknown"]);
    assert_eq!(confirmed.rules().len(), 1);
    let report = engine.clean(&confirmed, Source::Manual);

    assert_eq!(fx.fs.trashed(), [temp().join("b.log")]);
    assert!(fx.fs.exists(&temp().join("a.tmp")));
    assert_eq!(report.rules.len(), 1);
    assert_eq!(report.rules[0].rule_id, "logs");
}

#[test]
fn a_root_that_moved_or_became_protected_since_the_scan_stops_the_clean() {
    for change in 0..2 {
        let fx = Fixture::new();
        fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
        let (engine, plan) = engine_and_plan(&fx, &default_rule());
        if change == 0 {
            // The location now points somewhere else.
            let other = abs("Users/tester/AppData/Local/Other");
            fx.fs.add_file_at(other.join("a.tmp"), 1, ago(3));
            fx.folders.set(Location::UserTemp, other);
        } else {
            // The same folder is now protected.
            fx.folders.protect(temp());
        }
        let report = engine.clean(&plan.confirm(), Source::Manual);
        assert_eq!(report.moved_files(), 0, "change {change}");
        assert_eq!(
            report.rules[0].skipped[0].reason,
            CleanSkipReason::RootChanged
        );
        assert!(fx.fs.trashed().is_empty());
    }
}

#[test]
fn a_root_that_became_a_link_stops_the_clean() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.fs.add_link(temp());
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::RootChanged
    );
}

#[test]
fn a_plan_is_only_cleaned_by_an_engine_that_still_allows_it() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    fx.fs.add_file_at(temp().join("b.log"), 1, ago(3));
    let broad = temp_rule("r", r#"["**"]"#, "[]", "min_age_days = 0\nmax_depth = 1");
    let (_, plan) = engine_and_plan(&fx, &broad);
    assert_eq!(only(&plan).items.len(), 2);

    // The same rule id, but its include list now covers only `.log` files.
    let narrow = fx.engine(&temp_rule(
        "r",
        r#"["*.log"]"#,
        "[]",
        "min_age_days = 0\nmax_depth = 1",
    ));
    let report = narrow.clean(&plan.confirm(), Source::Manual);
    assert_eq!(fx.fs.trashed(), [temp().join("b.log")]);
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::NotAllowedByRule
    );

    // A rule the engine does not have at all.
    let fx2 = Fixture::new();
    let stranger = fx2.engine(&temp_rule(
        "other",
        r#"["**"]"#,
        "[]",
        "min_age_days = 0\nmax_depth = 1",
    ));
    let report = stranger.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert_eq!(report.rules[0].skipped.len(), 2);
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::RuleUnknown
    );
}

#[test]
fn an_empty_confirmed_plan_does_nothing_and_writes_nothing() {
    let fx = Fixture::new();
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert_eq!(report.skipped_count(), 0);
    assert!(fx.activity.records().is_empty());
    assert!(fx.fs.trashed().is_empty());
}

// ---- activity ----------------------------------------------------------------------------------

#[test]
fn a_clean_writes_one_redacted_record_per_rule() {
    let fx = Fixture::new();
    fx.fs
        .add_file_at(temp().join("secret-token-4711.tmp"), 2048, ago(3));
    fx.fs.add_file_at(temp().join("locked.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.fs.fail_trash_of(
        temp().join("locked.tmp"),
        PortError::AccessDenied("password=hunter2".to_owned()),
    );

    let report = engine.clean(&plan.confirm(), Source::Automation);

    assert_eq!(report.activity_write_failures, 0);
    let records = fx.activity.records();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(record.outcome, ActivityOutcome::Applied);
    assert_eq!(record.source, Source::Automation);
    assert_eq!(record.label.as_deref(), Some("temp-files"));
    assert_eq!(record.batch, None);
    assert!(record.summary.contains("temp-files"), "{}", record.summary);
    assert!(record.summary.contains("1 file"), "{}", record.summary);
    assert!(record.summary.contains("2.0 KB"), "{}", record.summary);
    assert!(
        record.summary.contains("1 item in place"),
        "{}",
        record.summary
    );
    for forbidden in [
        "secret-token",
        "4711",
        "tester",
        "hunter2",
        "locked.tmp",
        "Temp",
    ] {
        assert!(
            !record.summary.contains(forbidden),
            "{forbidden} leaked into: {}",
            record.summary
        );
    }
    assert!(record.summary.contains("Recycle Bin is emptied"));
}

#[test]
fn a_clean_that_moved_nothing_is_recorded_as_failed() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.fs.fail_trash_of(
        temp().join("a.tmp"),
        PortError::AccessDenied("x".to_owned()),
    );
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert_eq!(fx.activity.records()[0].outcome, ActivityOutcome::Failed);
}

#[test]
fn a_failing_activity_log_never_blocks_the_clean() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    let (engine, plan) = engine_and_plan(&fx, &default_rule());
    fx.activity
        .fail_records_with(Some(PortError::Other("disk full".to_owned())));
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    assert_eq!(report.activity_write_failures, 1);
    assert_eq!(fx.fs.trashed().len(), 1);
}

#[test]
fn without_an_activity_log_nothing_is_recorded_and_nothing_fails() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(3));
    let engine = fx.engine(&default_rule());
    let report = engine.clean(&engine.scan_all().confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    assert_eq!(report.activity_write_failures, 0);
    assert!(fx.activity.records().is_empty());
}

#[test]
fn sizes_are_shown_in_human_units() {
    use super::apply::human_bytes;
    assert_eq!(human_bytes(0), "0 B");
    assert_eq!(human_bytes(1023), "1023 B");
    assert_eq!(human_bytes(1024), "1.0 KB");
    assert_eq!(human_bytes(1536), "1.5 KB");
    assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    assert_eq!(human_bytes(u64::MAX), "16777216.0 TB");
}

// ---- administrator rights --------------------------------------------------------------------

/// A rule over the temp folder, with the optional top-level fields appended.
fn rule_with(extra: &str) -> String {
    format!(
        r#"
[[rule]]
id = "r"
title = "t"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]
{extra}

[rule.root]
location = "user_temp"

[rule.constraints]
min_age_days = 7
max_depth = 3
"#
    )
}

#[test]
fn requires_admin_and_min_age_setting_are_optional_and_default_to_off() {
    let rules = CleaningRules::from_toml(&rule_with("")).expect("loads");
    let rule = rules.get("r").expect("rule");
    assert!(!rule.requires_admin);
    assert_eq!(rule.min_age_setting, None);

    let rules = CleaningRules::from_toml(&rule_with(
        "requires_admin = true\nmin_age_setting = \"clean.temp_min_age_days\"",
    ))
    .expect("loads");
    let rule = rules.get("r").expect("rule");
    assert!(rule.requires_admin);
    assert_eq!(
        rule.min_age_setting.as_deref(),
        Some("clean.temp_min_age_days")
    );
}

#[test]
fn a_malformed_requires_admin_or_settings_key_is_rejected_with_the_field() {
    for (extra, field) in [
        ("requires_admin = \"yes\"", "requires_admin"),
        ("min_age_setting = 7", "min_age_setting"),
        ("min_age_setting = \"\"", "min_age_setting"),
        ("min_age_setting = \"Clean.Temp\"", "min_age_setting"),
        ("min_age_setting = \"clean..temp\"", "min_age_setting"),
        ("min_age_setting = \"1clean\"", "min_age_setting"),
        ("min_age_setting = \"clean temp\"", "min_age_setting"),
    ] {
        let issues = issues_of(&rule_with(extra));
        assert_eq!(issues.len(), 1, "{extra}: {issues:?}");
        assert_eq!(issues[0].0.as_deref(), Some("r"));
        assert_eq!(issues[0].1, field, "{extra}");
    }
}

#[test]
fn a_denied_root_is_a_typed_failure_and_says_so_for_a_rule_that_needs_admin() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(30));
    fx.fs
        .fail_read_dir_of(temp(), PortError::AccessDenied("x".to_owned()));

    let plan = fx.engine(&rule_with("requires_admin = true")).scan_all();
    let scan = only(&plan);
    assert_eq!(scan.failure, Some(ScanFailure::RootAccessDenied));
    assert!(scan.requires_admin);
    assert!(scan.needs_administrator());
    assert!(scan.items.is_empty());

    // The same refusal on a rule that does not claim to need administrator
    // rights is a plain failure: the flag is the rule's statement.
    let plan = fx.engine(&rule_with("")).scan_all();
    let scan = only(&plan);
    assert_eq!(scan.failure, Some(ScanFailure::RootAccessDenied));
    assert!(!scan.needs_administrator());

    // Another kind of failure is not a sign of missing rights.
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(30));
    fx.fs
        .fail_read_dir_of(temp(), PortError::Other("boom".to_owned()));
    let plan = fx.engine(&rule_with("requires_admin = true")).scan_all();
    assert_eq!(
        only(&plan).failure,
        Some(ScanFailure::RootUnreadable("error"))
    );
    assert!(!only(&plan).needs_administrator());
}

#[test]
fn items_that_cannot_be_read_mark_a_partial_scan_of_an_admin_rule() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(30));
    fx.fs.add_file_at(temp().join("sub/b.tmp"), 1, ago(30));
    fx.fs
        .fail_read_dir_of(temp().join("sub"), PortError::AccessDenied("x".to_owned()));
    let plan = fx.engine(&rule_with("requires_admin = true")).scan_all();
    let scan = only(&plan);
    assert_eq!(scan.failure, None);
    assert_eq!(paths(scan), [temp().join("a.tmp")]);
    assert_eq!(scan.counts.access_denied, 1);
    assert!(scan.needs_administrator());

    // A rule that needs no rights does not claim it.
    let plan = fx.engine(&rule_with("")).scan_all();
    assert!(!only(&plan).needs_administrator());
}

#[test]
fn a_readable_admin_rule_does_not_need_administrator_rights() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 1, ago(30));
    let plan = fx.engine(&rule_with("requires_admin = true")).scan_all();
    let scan = only(&plan);
    assert_eq!(scan.file_count(), 1);
    assert!(scan.requires_admin);
    assert!(!scan.needs_administrator());
}

// ---- the minimum age setting -----------------------------------------------------------------

const AGE_KEY: &str = "clean.temp_min_age_days";

fn aged_rule() -> String {
    rule_with(&format!("min_age_setting = \"{AGE_KEY}\""))
}

/// Settings whose reads fail.
struct FailingSettings;

impl Settings for FailingSettings {
    fn get(&self, _key: &str) -> Result<Option<serde_json::Value>, PortError> {
        Err(PortError::Other("locked database".to_owned()))
    }
    fn set(&self, _key: &str, _value: &serde_json::Value) -> Result<(), PortError> {
        Ok(())
    }
    fn remove(&self, _key: &str) -> Result<bool, PortError> {
        Ok(false)
    }
}

fn settings_with(value: Option<serde_json::Value>) -> Arc<FakeSettings> {
    let settings = Arc::new(FakeSettings::new());
    if let Some(value) = value {
        settings.set(AGE_KEY, &value).expect("set");
    }
    settings
}

#[test]
fn a_rule_keeps_its_own_age_without_settings_or_without_a_stored_value() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("old.tmp"), 1, ago(8));
    fx.fs.add_file_at(temp().join("new.tmp"), 1, ago(6));

    // No settings attached at all.
    let plan = fx.engine(&aged_rule()).scan_all();
    assert_eq!(only(&plan).min_age_days, 7);
    assert_eq!(paths(only(&plan)), [temp().join("old.tmp")]);

    // Settings attached, nothing stored.
    let engine = fx.engine(&aged_rule()).with_settings(settings_with(None));
    let plan = engine.scan_all();
    assert_eq!(only(&plan).min_age_days, 7);
    assert_eq!(paths(only(&plan)), [temp().join("old.tmp")]);
}

#[test]
fn a_file_newer_than_the_configured_age_is_never_listed() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("year.tmp"), 1, ago(400));
    fx.fs.add_file_at(temp().join("exact.tmp"), 1, ago(30));
    fx.fs.add_file_at(temp().join("recent.tmp"), 1, ago(29));
    fx.fs.add_file_at(temp().join("today.tmp"), 1, ago(0));
    let settings = settings_with(Some(serde_json::json!(30)));
    let engine = fx.engine(&aged_rule()).with_settings(settings.clone());

    let plan = engine.scan_all();
    let scan = only(&plan);
    assert_eq!(scan.min_age_days, 30);
    assert_eq!(
        paths(scan),
        [temp().join("exact.tmp"), temp().join("year.tmp")]
    );
    assert_eq!(scan.counts.too_new, 2);
    assert!(
        scan.skipped
            .iter()
            .all(|entry| entry.reason == SkipReason::TooNew)
    );

    // The setting can also lower the age below the rule's own, down to a day.
    settings.set(AGE_KEY, &serde_json::json!(1)).expect("set");
    let plan = engine.scan_all();
    assert_eq!(only(&plan).min_age_days, 1);
    assert_eq!(only(&plan).file_count(), 3);
    assert!(!paths(only(&plan)).contains(&temp().join("today.tmp")));
}

#[test]
fn an_invalid_age_setting_fails_the_rule_instead_of_guessing() {
    use serde_json::json;
    for bad in [
        json!(0),
        json!(-3),
        json!(3651),
        json!(7.5),
        json!("7"),
        json!(true),
        json!(null),
        json!([7]),
    ] {
        let fx = Fixture::new();
        fx.fs.add_file_at(temp().join("old.tmp"), 1, ago(400));
        let engine = fx
            .engine(&aged_rule())
            .with_settings(settings_with(Some(bad.clone())));
        let plan = engine.scan_all();
        let scan = only(&plan);
        assert_eq!(
            scan.failure,
            Some(ScanFailure::MinAgeSettingInvalid),
            "{bad}"
        );
        assert!(scan.items.is_empty(), "{bad}");
        assert!(!scan.root_exists, "the root is not even looked at: {bad}");
    }
    // The edges are valid.
    for good in [1, 3650] {
        let fx = Fixture::new();
        let engine = fx
            .engine(&aged_rule())
            .with_settings(settings_with(Some(serde_json::json!(good))));
        assert_eq!(only(&engine.scan_all()).failure, None, "{good}");
        assert_eq!(only(&engine.scan_all()).min_age_days, good);
    }
}

#[test]
fn an_unreadable_age_setting_fails_the_rule_with_its_error_class_only() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("old.tmp"), 1, ago(400));
    let engine = fx
        .engine(&aged_rule())
        .with_settings(Arc::new(FailingSettings));
    let plan = engine.scan_all();
    let scan = only(&plan);
    assert_eq!(
        scan.failure,
        Some(ScanFailure::MinAgeSettingUnreadable("error"))
    );
    assert!(scan.items.is_empty());
    assert!(!format!("{scan:?}").contains("locked database"));
}

#[test]
fn a_rule_without_a_settings_key_ignores_the_setting() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("old.tmp"), 1, ago(8));
    fx.fs.add_file_at(temp().join("new.tmp"), 1, ago(6));
    let engine = fx
        .engine(&rule_with(""))
        .with_settings(settings_with(Some(serde_json::json!(1))));
    let plan = engine.scan_all();
    assert_eq!(only(&plan).min_age_days, 7);
    assert_eq!(paths(only(&plan)), [temp().join("old.tmp")]);
}

#[test]
fn the_configured_age_also_governs_the_folders_taken_with_remove_empty_dirs() {
    let fx = Fixture::new();
    fx.fs.add_dir(temp().join("a"), Some(ago(10)));
    fx.fs.add_file_at(temp().join("a/x.tmp"), 1, ago(10));
    let toml = format!(
        "{}\n",
        aged_rule().replace("max_depth = 3", "max_depth = 3\nremove_empty_dirs = true")
    );
    let settings = settings_with(Some(serde_json::json!(30)));
    let engine = fx.engine(&toml).with_settings(settings);
    let plan = engine.scan_all();
    assert!(only(&plan).items.is_empty());
    assert_eq!(only(&plan).counts.too_new, 1);
}

// ---- item cap, progress and cancel -----------------------------------------------------------

#[test]
fn a_rule_stops_at_the_item_cap_and_says_the_scan_is_truncated() {
    let fx = Fixture::new();
    for n in 0..5 {
        fx.fs
            .add_file_at(temp().join(format!("f{n}.tmp")), 10, ago(5));
    }
    let engine = fx.engine(&default_rule()).with_item_cap(3);
    let plan = engine.scan(&["temp-files"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(scan.items.len(), 3);
    assert!(scan.truncated);
    assert_eq!(scan.failure, None);

    // Exactly at the cap is not truncated: nothing was left out.
    let engine = fx.engine(&default_rule()).with_item_cap(5);
    let plan = engine.scan(&["temp-files"]).expect("scan");
    assert_eq!(only(&plan).items.len(), 5);
    assert!(!only(&plan).truncated);
}

#[test]
fn a_folder_that_was_not_fully_looked_at_is_not_planned_for_removal() {
    let fx = Fixture::new();
    fx.fs.add_dir(temp().join("a"), Some(ago(5)));
    fx.fs.add_file_at(temp().join("a").join("1.tmp"), 1, ago(5));
    fx.fs.add_file_at(temp().join("a").join("2.tmp"), 1, ago(5));
    let toml = temp_rule(
        "temp-files",
        r#"["**"]"#,
        "[]",
        "min_age_days = 1\nmax_depth = 3\nremove_empty_dirs = true",
    );
    let plan = fx
        .engine(&toml)
        .with_item_cap(1)
        .scan(&["temp-files"])
        .expect("scan");
    let scan = only(&plan);
    assert!(scan.truncated);
    assert_eq!(
        scan.directory_count(),
        0,
        "the folder still has files in it"
    );
}

#[test]
fn the_cap_defaults_to_the_documented_maximum_and_cannot_be_raised() {
    let fx = Fixture::new();
    let engine = fx.engine(&default_rule()).with_item_cap(usize::MAX);
    assert_eq!(engine.item_cap, MAX_ITEMS_PER_RULE);
    assert_eq!(fx.engine(&default_rule()).item_cap, MAX_ITEMS_PER_RULE);
}

struct Watcher {
    seen: std::sync::Mutex<Vec<(usize, usize)>>,
    cancel_after: usize,
}

impl Watcher {
    fn new(cancel_after: usize) -> Self {
        Self {
            seen: std::sync::Mutex::default(),
            cancel_after,
        }
    }

    fn seen(&self) -> Vec<(usize, usize)> {
        self.seen.lock().expect("lock").clone()
    }
}

impl ScanObserver for Watcher {
    fn progress(&self, done: usize, total: usize) {
        self.seen.lock().expect("lock").push((done, total));
    }

    fn is_cancelled(&self) -> bool {
        self.seen().len() >= self.cancel_after
    }
}

fn two_rules() -> String {
    format!(
        "{}{}",
        temp_rule("one", r#"["**"]"#, "[]", "min_age_days = 0\nmax_depth = 1"),
        temp_rule("two", r#"["**"]"#, "[]", "min_age_days = 0\nmax_depth = 1"),
    )
}

#[test]
fn an_observed_scan_reports_each_rule_and_returns_the_same_plan() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 7, ago(5));
    let engine = fx.engine(&two_rules());
    let watcher = Watcher::new(usize::MAX);
    let plan = engine
        .scan_observed(&["one", "two"], &watcher)
        .expect("scan");
    assert_eq!(watcher.seen(), [(0, 2), (1, 2)]);
    // The manual clock ticks on every read, so only the rules are compared.
    assert_eq!(
        plan.rules,
        engine.scan(&["one", "two"]).expect("scan").rules
    );
}

#[test]
fn cancelling_between_rules_returns_the_rules_already_scanned_as_cancelled() {
    let fx = Fixture::new();
    let engine = fx.engine(&two_rules());
    let watcher = Watcher::new(1);
    let plan = engine
        .scan_observed(&["one", "two"], &watcher)
        .expect("a cancelled scan is a partial plan");
    assert!(plan.cancelled);
    assert_eq!(plan.rules.len(), 1, "the second rule never started");
    assert!(!plan.rules[0].truncated, "nothing was left unwalked");
    assert_eq!(watcher.seen(), [(0, 2)]);
    assert!(fx.fs.trash_attempts().is_empty());
}

/// Says "cancelled" from the n-th question on, counting every call.
struct CancelAtCheck {
    asked: std::sync::atomic::AtomicUsize,
    from: usize,
}

impl ScanObserver for CancelAtCheck {
    fn progress(&self, _done: usize, _total: usize) {}

    fn is_cancelled(&self) -> bool {
        self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1 >= self.from
    }
}

#[test]
fn cancelling_inside_a_rule_keeps_what_was_found_and_marks_the_rule_partial() {
    let fx = Fixture::new();
    for n in 0..10 {
        fx.fs
            .add_file_at(temp().join(format!("f{n}.tmp")), 10, ago(5));
    }
    let engine = fx.engine(&two_rules());
    // Check 1 is before the first rule, checks 2 to 4 are before the first
    // three entries: the fourth entry's check cancels.
    let observer = CancelAtCheck {
        asked: std::sync::atomic::AtomicUsize::new(0),
        from: 5,
    };
    let plan = engine
        .scan_observed(&["one", "two"], &observer)
        .expect("scan");
    assert!(plan.cancelled);
    assert_eq!(plan.rules.len(), 1, "the second rule never started");
    let scan = &plan.rules[0];
    assert!(scan.truncated);
    assert_eq!(scan.items.len(), 3, "the entries before the cancel stay");

    // What is listed can still be cleaned, and only that.
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 3);
    assert_eq!(fx.fs.trashed().len(), 3);
}

#[test]
fn a_scan_that_is_not_cancelled_is_not_marked_partial() {
    let fx = Fixture::new();
    fx.fs.add_file_at(temp().join("a.tmp"), 7, ago(5));
    let engine = fx.engine(&two_rules());
    let plan = engine
        .scan_observed(&["one", "two"], &Watcher::new(usize::MAX))
        .expect("scan");
    assert!(!plan.cancelled);
    assert!(plan.rules.iter().all(|r| !r.truncated));
}

#[test]
fn an_observed_scan_of_an_unknown_rule_is_refused_before_anything_is_scanned() {
    let fx = Fixture::new();
    let engine = fx.engine(&two_rules());
    let watcher = Watcher::new(usize::MAX);
    assert_eq!(
        engine.scan_observed(&["one", "nope"], &watcher),
        Err(ScanError::UnknownRule(UnknownRule("nope".to_owned())))
    );
    assert!(watcher.seen().is_empty());
}

// ---- the Recycle Bin ------------------------------------------------------------------------

mod recycle_bin {
    use super::*;
    use crate::clean::fakes::FakeRecycleBin;

    fn manager(bin: &Arc<FakeRecycleBin>, log: &Arc<FakeActivityLog>) -> RecycleBinManager {
        RecycleBinManager::new(bin.clone(), Arc::new(ManualClock::new(NOW)))
            .with_activity_log(log.clone())
    }

    #[test]
    fn status_reads_the_bin_and_changes_nothing() {
        let bin = Arc::new(FakeRecycleBin::holding(12, 3_000_000));
        let log = Arc::new(FakeActivityLog::new());
        let status = manager(&bin, &log).status().expect("status");
        assert_eq!(
            status,
            RecycleBinStatus {
                item_count: 12,
                size_bytes: 3_000_000
            }
        );
        assert_eq!(bin.empty_calls(), 0);
        assert!(log.records().is_empty());
    }

    #[test]
    fn emptying_is_recorded_as_permanent_and_cannot_be_undone() {
        let bin = Arc::new(FakeRecycleBin::holding(2, 2048));
        let log = Arc::new(FakeActivityLog::new());
        let report = manager(&bin, &log).empty(Source::Manual).expect("empty");
        assert!(report.emptied);
        assert_eq!(report.before.item_count, 2);
        assert_eq!(bin.empty_calls(), 1);
        assert_eq!(bin.status().expect("status").item_count, 0);
        let records = log.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].outcome, ActivityOutcome::Applied);
        assert_eq!(records[0].label.as_deref(), Some("recycle-bin"));
        assert!(records[0].entry.is_none() && records[0].batch.is_none());
        assert!(records[0].summary.contains("permanently deleted"));
        assert!(records[0].summary.contains("cannot be undone"));
        assert!(records[0].summary.contains("2 items (2.0 KB)"));
    }

    #[test]
    fn an_empty_bin_is_not_emptied_and_not_recorded() {
        let bin = Arc::new(FakeRecycleBin::new());
        let log = Arc::new(FakeActivityLog::new());
        let report = manager(&bin, &log).empty(Source::Manual).expect("empty");
        assert!(!report.emptied);
        assert_eq!(bin.empty_calls(), 0);
        assert!(log.records().is_empty());
    }

    #[test]
    fn a_failed_emptying_is_returned_and_recorded_as_failed() {
        let bin = Arc::new(FakeRecycleBin::holding(1, 10));
        bin.fail_empty_with(Some(PortError::AccessDenied("secret detail".into())));
        let log = Arc::new(FakeActivityLog::new());
        let error = manager(&bin, &log)
            .empty(Source::Manual)
            .expect_err("fails");
        assert!(matches!(error, PortError::AccessDenied(_)));
        let records = log.records();
        assert_eq!(records[0].outcome, ActivityOutcome::Failed);
        assert!(!records[0].summary.contains("secret"));
    }

    #[test]
    fn a_bin_that_cannot_be_read_is_never_emptied_blind() {
        let bin = Arc::new(FakeRecycleBin::holding(1, 10));
        bin.fail_status_with(Some(PortError::Other("boom".into())));
        let log = Arc::new(FakeActivityLog::new());
        assert!(manager(&bin, &log).empty(Source::Manual).is_err());
        assert_eq!(bin.empty_calls(), 0);
    }

    #[test]
    fn a_failing_activity_log_does_not_stop_the_emptying_but_is_reported() {
        let bin = Arc::new(FakeRecycleBin::holding(1, 10));
        let log = Arc::new(FakeActivityLog::new());
        log.fail_records_with(Some(PortError::Other("disk full".into())));
        let report = manager(&bin, &log).empty(Source::Manual).expect("empty");
        assert!(report.emptied && report.activity_write_failed);
        assert_eq!(bin.empty_calls(), 1);
    }
}

// ---- running programs that hold a rule's files -----------------------------------------------

fn locked_rule(locked_by: &str) -> String {
    default_rule().replacen(
        "risk = \"safe\"",
        &format!("risk = \"safe\"\nlocked_by = {locked_by}"),
        1,
    )
}

#[test]
fn locked_by_is_lower_cased_and_must_name_plain_executables() {
    let rules = CleaningRules::from_toml(&locked_rule(r#"["Chrome.EXE"]"#)).expect("valid");
    assert_eq!(rules.rules()[0].locked_by, ["chrome.exe"]);
    for bad in [
        r#"["chrome"]"#,
        r#"["C:/x/chrome.exe"]"#,
        r#"["..\chrome.exe"]"#,
        r#"[".exe"]"#,
        "[1]",
        r#""chrome.exe""#,
    ] {
        assert!(
            CleaningRules::from_toml(&locked_rule(bad)).is_err(),
            "{bad} must be refused"
        );
    }
}

#[test]
fn a_scan_names_the_running_programs_of_the_rule_only() {
    let f = Fixture::new();
    let processes = Arc::new(crate::fakes::FakeProcesses::new());
    processes.add(1, "Chrome.exe", 0);
    processes.add(2, "other.exe", 0);
    let toml = locked_rule(r#"["chrome.exe", "firefox.exe"]"#);
    let with = f.engine(&toml).with_processes(processes.clone());
    assert_eq!(only(&with.scan_all()).running_programs, ["chrome.exe"]);
    // Without a process port, or for a rule that names no program, nothing is reported.
    assert!(
        only(&f.engine(&toml).scan_all())
            .running_programs
            .is_empty()
    );
    let plain = f.engine(&default_rule()).with_processes(processes);
    assert!(only(&plain.scan_all()).running_programs.is_empty());
}

// ---- items, folders, followed variables and folders the user chose ---------------------------

fn models_rule(extra: &str) -> String {
    format!(
        r#"
[[rule]]
id = "models"
title = "Models"
description = "Downloaded models."
category = "ai_model_cache"
risk = "caution"
unit = "entries"
pick = true
include = ["models--*"]
{extra}
root = {{ location = "user_profile", path = ".cache/hub" }}
constraints = {{ min_age_days = 0, max_depth = 0 }}
"#
    )
}

fn hub() -> PathBuf {
    abs("Users/tester/.cache/hub")
}

fn profile_fixture() -> Fixture {
    let f = Fixture::new();
    f.folders.set(Location::UserProfile, abs("Users/tester"));
    f
}

#[test]
fn the_new_rule_fields_load_and_are_checked() {
    let rules = CleaningRules::from_toml(&models_rule(
        r#"follow = [{ var = "HF_HUB_CACHE", source = "Hugging Face docs" }, { var = "HF_HOME", path = "hub", source = "Hugging Face docs" }]"#,
    ))
    .expect("loads");
    let rule = rules.get("models").expect("rule");
    assert_eq!(rule.unit, ItemUnit::Entries);
    assert!(rule.pick);
    assert_eq!(rule.follow.len(), 2);
    assert_eq!(rule.follow[1].path, ["hub"]);
    assert_eq!(rule.follow[1].source, "Hugging Face docs");

    // Picking items needs whole entries.
    let no_entries = models_rule("").replace("unit = \"entries\"\n", "");
    assert!(CleaningRules::from_toml(&no_entries).is_err());
    for bad in [
        r#"follow = [{ var = "HF HOME", source = "x" }]"#,
        r#"follow = [{ var = "HF_HOME" }]"#,
        r#"follow = [{ var = "HF_HOME", source = "x", path = "../up" }]"#,
        r#"follow = [{ var = "HF_HOME", source = "x", other = 1 }]"#,
        r#"follow = "HF_HOME""#,
    ] {
        assert!(
            CleaningRules::from_toml(&models_rule(bad)).is_err(),
            "{bad} must be refused"
        );
    }
    // A rule in a folder the user chose needs a relative path and cannot follow a variable.
    let chosen = |path: &str, follow: &str| {
        format!(
            r#"
[[rule]]
id = "comfy"
title = "t"
description = "d"
category = "application_cache"
risk = "safe"
include = ["**"]
{follow}
install_markers = ["main.py"]
root = {{ location = "chosen_folder", path = "{path}" }}
constraints = {{ min_age_days = 1, max_depth = 2 }}
"#
        )
    };
    assert!(CleaningRules::from_toml(&chosen("temp", "")).is_ok());
    assert!(CleaningRules::from_toml(&chosen("", "")).is_err());
    assert!(
        CleaningRules::from_toml(&chosen("temp", r#"follow = [{ var = "X", source = "s" }]"#))
            .is_err()
    );
    // LocalLow is a location rules can name.
    let low = temp_rule("low", r#"["**"]"#, "[]", "min_age_days = 1\nmax_depth = 1").replace(
        r#"location = "user_temp""#,
        "location = \"local_low\"\npath = \"NVIDIA\"",
    );
    assert_eq!(
        CleaningRules::from_toml(&low)
            .expect("loads")
            .get("low")
            .expect("rule")
            .root
            .location(),
        Location::LocalLow
    );
}

/// Two models: one with a link inside (as Hugging Face snapshots have), one plain; and things the
/// rule does not match.
fn install_models(f: &Fixture) {
    let a = hub().join("models--org--a");
    f.fs.add_file_at(a.join("blobs").join("b1"), 1000, ago(30));
    f.fs.add_file_at(a.join("blobs").join("b2"), 500, ago(10));
    f.fs.add_link(a.join("snapshots").join("rev").join("model.bin"));
    let b = hub().join("models--org--b");
    f.fs.add_file_at(b.join("blobs").join("c1"), 200, ago(1));
    f.fs.add_file_at(hub().join(".locks").join("x.lock"), 1, ago(1));
    f.fs.add_file_at(hub().join("token"), 1, ago(1));
}

#[test]
fn a_rule_with_entries_lists_each_folder_as_one_item_with_its_size() {
    let f = profile_fixture();
    install_models(&f);
    let engine = f.engine(&models_rule(""));
    let plan = engine.scan(&["models"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(scan.failure, None);
    assert!(scan.pick);
    assert_eq!(
        paths(scan),
        [hub().join("models--org--a"), hub().join("models--org--b")]
    );
    let a = &scan.items[0];
    assert_eq!(a.kind, ItemKind::Folder);
    assert_eq!((a.size_bytes, a.file_count), (1500, 2));
    assert_eq!(a.modified, Some(ago(10)));
    assert_eq!(scan.file_count(), 3);
    assert_eq!(scan.total_bytes(), 1700);
    assert_eq!(
        scan.counts.links, 1,
        "the link inside is counted, not followed"
    );
    assert_eq!(scan.counts.not_matching, 2);
    // Nothing is confirmed without picking: a rule picked by item is left out of the whole-plan
    // confirmations.
    assert!(plan.confirm().rules().is_empty());
    assert!(plan.confirm_rules(&["models"]).rules().is_empty());
}

#[test]
fn only_the_picked_items_are_moved_and_a_folder_goes_in_one_move() {
    let f = profile_fixture();
    install_models(&f);
    let engine = f.engine(&models_rule(""));
    let plan = engine.scan(&["models"]).expect("scan");
    let confirmed = plan
        .confirm_selection(&[RuleSelection {
            rule_id: "models".to_owned(),
            items: Some(vec![1]),
        }])
        .expect("confirmed");
    let report = engine.clean(&confirmed, Source::Manual);
    assert_eq!(report.skipped_count(), 0, "{report:?}");
    assert_eq!(report.moved_files(), 1);
    assert_eq!(report.moved_bytes(), 200);
    // One move for the whole folder, not one per file.
    assert_eq!(f.fs.trash_attempts(), [hub().join("models--org--b")]);
    assert!(f.fs.exists(&hub().join("models--org--a").join("blobs").join("b1")));
    assert!(!f.fs.exists(&hub().join("models--org--b")));
    assert!(f.fs.exists(&hub().join("token")));
}

#[test]
fn a_selection_must_name_real_items_of_a_rule_that_is_picked_by_item() {
    let f = profile_fixture();
    install_models(&f);
    f.fs.add_file_at(temp().join("a.tmp"), 1, ago(30));
    let both = format!(
        "{}{}",
        models_rule(""),
        default_rule().replace("temp-files", "plain")
    );
    let engine = f.engine(&both);
    let plan = engine.scan(&["models", "plain"]).expect("scan");
    let select = |rule: &str, items: Option<Vec<usize>>| {
        plan.confirm_selection(&[RuleSelection {
            rule_id: rule.to_owned(),
            items,
        }])
    };
    assert_eq!(
        select("models", None).unwrap_err(),
        SelectionError::NothingPicked("models".to_owned())
    );
    assert_eq!(
        select("models", Some(vec![])).unwrap_err(),
        SelectionError::NothingPicked("models".to_owned())
    );
    assert_eq!(
        select("models", Some(vec![7])).unwrap_err(),
        SelectionError::NoSuchItem("models".to_owned(), 7)
    );
    assert_eq!(
        select("plain", Some(vec![0])).unwrap_err(),
        SelectionError::NotPickable("plain".to_owned())
    );
    assert_eq!(
        select("ghost", None).unwrap_err(),
        SelectionError::NotInPlan("ghost".to_owned())
    );
    assert!(select("plain", None).is_ok());
    assert!(select("models", Some(vec![0, 1])).is_ok());
}

#[test]
fn a_folder_that_changed_since_the_scan_is_not_moved() {
    let f = profile_fixture();
    install_models(&f);
    let engine = f.engine(&models_rule(""));
    let plan = engine.scan(&["models"]).expect("scan");
    // A program writes a new file into the model after the scan.
    f.fs.add_file_at(
        hub().join("models--org--b").join("blobs").join("new"),
        5,
        ago(0),
    );
    let confirmed = plan
        .confirm_selection(&[RuleSelection {
            rule_id: "models".to_owned(),
            items: Some(vec![0, 1]),
        }])
        .expect("confirmed");
    let report = engine.clean(&confirmed, Source::Manual);
    assert_eq!(report.rules[0].moved.len(), 1);
    assert_eq!(report.rules[0].skipped.len(), 1);
    assert_eq!(report.rules[0].skipped[0].reason, CleanSkipReason::Changed);
    assert!(f.fs.exists(&hub().join("models--org--b")));
}

#[test]
fn a_folder_with_something_unreadable_inside_is_left_out_and_reported() {
    let f = profile_fixture();
    install_models(&f);
    f.fs.fail_read_dir_of(
        hub().join("models--org--a").join("blobs"),
        PortError::AccessDenied("denied".to_owned()),
    );
    let engine = f.engine(&models_rule(""));
    let plan = engine.scan(&["models"]).expect("scan");
    let scan = only(&plan);
    assert_eq!(paths(scan), [hub().join("models--org--b")]);
    assert_eq!(scan.counts.access_denied, 1);
}

fn chosen_rule() -> String {
    r#"
[[rule]]
id = "comfy"
title = "ComfyUI temp"
description = "Temp files."
category = "application_cache"
risk = "safe"
include = ["**"]
install_markers = ["main.py", "comfy"]
root = { location = "chosen_folder", path = "temp" }
constraints = { min_age_days = 1, max_depth = 3, remove_empty_dirs = true }
"#
    .to_owned()
}

/// Gives `install` the files and folders the chosen-folder rule expects of a real install.
fn make_install(f: &Fixture, install: &Path) {
    f.fs.add_dir(install, Some(ago(400)));
    f.fs.add_file_at(install.join("main.py"), 10, ago(400));
    f.fs.add_dir(install.join("comfy"), Some(ago(400)));
}

fn chosen_engine(f: &Fixture) -> (CleanEngine, Arc<FakeSettings>) {
    let settings = Arc::new(FakeSettings::new());
    (
        f.engine(&chosen_rule()).with_settings(settings.clone()),
        settings,
    )
}

#[test]
fn a_rule_that_needs_a_folder_waits_until_one_is_chosen() {
    let f = Fixture::new();
    let (engine, settings) = chosen_engine(&f);
    let scan = engine.scan_all();
    assert_eq!(
        only(&scan).failure,
        Some(ScanFailure::Root(RootError::FolderNotChosen))
    );
    assert_eq!(engine.chosen_folder("comfy"), Ok(None));

    let install = abs("AI/ComfyUI");
    make_install(&f, &install);
    f.fs.add_file_at(install.join("temp").join("preview.png"), 10, ago(30));
    f.fs.add_file_at(install.join("temp").join("fresh.png"), 10, ago(0));
    f.fs.add_file_at(install.join("output").join("image.png"), 10, ago(30));
    f.fs.add_file_at(install.join("models").join("m.safetensors"), 10, ago(30));
    engine.choose_folder("comfy", &install).expect("chosen");
    assert_eq!(engine.chosen_folder("comfy"), Ok(Some(install.clone())));
    assert!(
        settings
            .get(CHOSEN_FOLDERS_SETTING)
            .expect("read")
            .is_some()
    );

    let plan = engine.scan_all();
    let scan = only(&plan);
    assert_eq!(scan.failure, None);
    assert_eq!(scan.root.as_deref(), Some(install.join("temp").as_path()));
    assert_eq!(paths(scan), [install.join("temp").join("preview.png")]);
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    assert!(f.fs.exists(&install.join("output").join("image.png")));
    assert!(f.fs.exists(&install.join("models").join("m.safetensors")));

    // The choice is only forgotten: the folder is untouched.
    engine.forget_folder("comfy").expect("forgotten");
    assert_eq!(engine.chosen_folder("comfy"), Ok(None));
    assert_eq!(settings.get(CHOSEN_FOLDERS_SETTING).expect("read"), None);
    assert!(f.fs.exists(&install.join("models").join("m.safetensors")));
    engine
        .forget_folder("comfy")
        .expect("forgetting twice is fine");
}

#[test]
fn a_folder_that_could_hold_more_than_a_cache_cannot_be_chosen() {
    let f = Fixture::new();
    let (engine, _) = chosen_engine(&f);
    let refused = |path: PathBuf| engine.choose_folder("comfy", &path).unwrap_err();
    // Not there, not a folder.
    assert_eq!(refused(abs("AI/Missing")), FolderError::NotAFolder);
    f.fs.add_file(abs("AI/file.txt"), 1);
    assert_eq!(refused(abs("AI/file.txt")), FolderError::NotAFolder);
    f.fs.add_link(abs("AI/link"));
    assert_eq!(refused(abs("AI/link")), FolderError::NotAFolder);
    // Relative, too broad, protected, and the output folder of an image generator.
    assert!(matches!(
        refused(PathBuf::from("ComfyUI")),
        FolderError::Refused(RootError::NotAbsolute)
    ));
    f.fs.add_dir(abs(""), None);
    assert!(matches!(
        refused(abs("")),
        FolderError::Refused(RootError::TooBroad)
    ));
    f.fs.add_dir(abs("Users/tester"), None);
    assert!(matches!(
        refused(abs("Users/tester")),
        FolderError::Refused(RootError::TooBroad)
    ));
    f.folders.set_system_folders(vec![abs("Windows")]);
    f.fs.add_dir(abs("Windows/Servicing"), None);
    assert!(matches!(
        refused(abs("Windows/Servicing")),
        FolderError::Refused(RootError::TooBroad)
    ));
    f.fs.add_dir(abs("AI/ComfyUI/output"), None);
    assert!(matches!(
        refused(abs("AI/ComfyUI/output")),
        FolderError::Refused(RootError::OutputFolder)
    ));
    assert_eq!(engine.chosen_folder("comfy"), Ok(None));
    // A rule that needs no folder, and one that does not exist.
    let plain = f
        .engine(&default_rule())
        .with_settings(Arc::new(FakeSettings::new()));
    assert!(matches!(
        plain.choose_folder("temp-files", &abs("AI")),
        Err(FolderError::NotChoosable(_))
    ));
    assert!(matches!(
        plain.choose_folder("ghost", &abs("AI")),
        Err(FolderError::UnknownRule(_))
    ));
    // Without settings nothing can be kept.
    assert_eq!(
        f.engine(&chosen_rule())
            .choose_folder("comfy", &abs("AI/ComfyUI"))
            .unwrap_err(),
        FolderError::NoSettings
    );
}

#[test]
fn a_folder_that_does_not_look_like_the_install_is_refused_and_says_what_is_missing() {
    let f = Fixture::new();
    let (engine, _) = chosen_engine(&f);
    // Any folder with a temp subfolder used to be accepted.
    let other = abs("AI/Other");
    f.fs.add_file_at(other.join("temp").join("a.tmp"), 10, ago(30));
    assert_eq!(
        engine.choose_folder("comfy", &other).unwrap_err(),
        FolderError::Refused(RootError::NotInstall(vec![
            "main.py".to_owned(),
            "comfy".to_owned()
        ]))
    );
    // One marker present: only the other is named.
    f.fs.add_file_at(other.join("main.py"), 10, ago(30));
    assert_eq!(
        engine.choose_folder("comfy", &other).unwrap_err(),
        FolderError::Refused(RootError::NotInstall(vec!["comfy".to_owned()]))
    );
    assert_eq!(engine.chosen_folder("comfy"), Ok(None));
    // A link does not count as a marker.
    f.fs.add_link(other.join("comfy"));
    assert!(engine.choose_folder("comfy", &other).is_err());
    // A real install is accepted.
    let install = abs("AI/ComfyUI");
    make_install(&f, &install);
    engine.choose_folder("comfy", &install).expect("chosen");
}

#[test]
fn a_chosen_folder_that_stops_matching_is_not_scanned_and_nothing_in_it_is_cleaned() {
    let f = Fixture::new();
    let (engine, _) = chosen_engine(&f);
    let install = abs("AI/ComfyUI");
    make_install(&f, &install);
    f.fs.add_file_at(install.join("temp").join("a.png"), 10, ago(30));
    engine.choose_folder("comfy", &install).expect("chosen");
    let plan = engine.scan_all();
    assert_eq!(paths(only(&plan)), [install.join("temp").join("a.png")]);

    // The install is replaced by something else: the old plan is skipped, a new scan refuses.
    f.fs.move_to_trash(&install, &install.join("main.py"))
        .expect("moved away");
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 0);
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::RootChanged
    );
    assert!(f.fs.exists(&install.join("temp").join("a.png")));
    assert_eq!(
        only(&engine.scan_all()).failure,
        Some(ScanFailure::Root(RootError::NotInstall(vec![
            "main.py".to_owned()
        ])))
    );
}

#[test]
fn install_markers_are_required_for_a_chosen_folder_rule_and_must_be_plain_names() {
    let with =
        |markers: &str| chosen_rule().replace(r#"install_markers = ["main.py", "comfy"]"#, markers);
    assert!(CleaningRules::from_toml(&with("")).is_err());
    assert!(CleaningRules::from_toml(&with("install_markers = []")).is_err());
    for bad in ["../x", "a/b", r"a\\b", "..", "", "C:"] {
        assert!(
            CleaningRules::from_toml(&with(&format!("install_markers = [\"{bad}\"]"))).is_err(),
            "{bad:?} must be refused"
        );
    }
    assert!(CleaningRules::from_toml(&with(r#"install_markers = ["main.py"]"#)).is_ok());
    // A rule that works in a fixed location has none.
    let fixed = default_rule().replace(
        "include",
        "install_markers = [\"x\"]
include",
    );
    assert!(CleaningRules::from_toml(&fixed).is_err());
}

#[test]
fn output_folders_are_never_offered_even_when_the_rule_would_match_them() {
    let f = Fixture::new();
    // A rule that takes everything below `temp`, including folders called Output inside it, and a
    // stored folder that was edited to lie in an output folder.
    let (engine, settings) = chosen_engine(&f);
    let install = abs("AI/ComfyUI");
    make_install(&f, &install);
    f.fs.add_file_at(install.join("temp").join("a.png"), 10, ago(30));
    f.fs.add_file_at(
        install.join("temp").join("Output").join("kept.png"),
        10,
        ago(30),
    );
    f.fs.add_file_at(
        install
            .join("temp")
            .join("sub")
            .join("outputs")
            .join("kept.png"),
        10,
        ago(30),
    );
    engine.choose_folder("comfy", &install).expect("chosen");
    let plan = engine.scan_all();
    assert_eq!(paths(only(&plan)), [install.join("temp").join("a.png")]);

    // A plan edited to name an output file is refused at clean time, too.
    let mut tampered = plan.clone();
    tampered.rules[0].items.push(CleanItem {
        path: install.join("temp").join("Output").join("kept.png"),
        kind: ItemKind::File,
        size_bytes: 10,
        file_count: 1,
        modified: Some(ago(30)),
    });
    let report = engine.clean(&tampered.confirm(), Source::Manual);
    assert_eq!(report.rules[0].moved.len(), 1);
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::NotAllowedByRule
    );
    assert!(f.fs.exists(&install.join("temp").join("Output").join("kept.png")));

    // A stored folder that is an output folder is refused at scan time.
    settings
        .set(
            CHOSEN_FOLDERS_SETTING,
            &serde_json::json!({ "comfy": abs("AI/ComfyUI/output").to_string_lossy() }),
        )
        .expect("set");
    assert_eq!(
        only(&engine.scan_all()).failure,
        Some(ScanFailure::Root(RootError::OutputFolder))
    );
}

fn cargo_rule() -> String {
    r#"
[[rule]]
id = "cargo"
title = "Cargo"
description = "Crates."
category = "developer_cache"
risk = "caution"
include = ["registry/cache/**"]
follow = [
  { var = "CARGO_HOME", source = "The Cargo Book" },
  { var = "OTHER_HOME", path = "cache", source = "Other docs" },
]
root = { location = "user_profile", path = ".cargo" }
constraints = { min_age_days = 0, max_depth = 8 }
"#
    .to_owned()
}

fn followed_fixture() -> (Fixture, CleanEngine) {
    let f = profile_fixture();
    f.folders.protect(abs("Users/tester/AppData/Local"));
    f.folders
        .set_system_folders(vec![abs("Windows"), abs("Program Files")]);
    let engine = f.engine(&cargo_rule());
    (f, engine)
}

#[test]
fn a_documented_variable_moves_the_root_and_the_scan_says_so() {
    let (f, engine) = followed_fixture();
    f.fs.add_file_at(
        abs("Users/tester/.cargo/registry/cache/old.crate"),
        1,
        ago(5),
    );
    let default_scan = engine.scan_all();
    assert_eq!(only(&default_scan).followed_by, None);
    assert_eq!(paths(only(&default_scan)).len(), 1);

    // A top-level folder on another drive is a plausible cache folder.
    f.folders.set_env("CARGO_HOME", abs("cargo"));
    f.fs.add_file_at(abs("cargo/registry/cache/moved.crate"), 7, ago(5));
    let plan = engine.scan_all();
    let scan = only(&plan);
    assert_eq!(scan.followed_by.as_deref(), Some("CARGO_HOME"));
    assert_eq!(scan.root.as_deref(), Some(abs("cargo").as_path()));
    assert_eq!(paths(scan), [abs("cargo/registry/cache/moved.crate")]);

    // The clean checks the root again: it is the followed one, and it still moves the file.
    let report = engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    assert!(f.fs.exists(&abs("Users/tester/.cargo/registry/cache/old.crate")));

    // The variable changes after the scan: the plan is stale and nothing moves.
    f.fs.add_file_at(abs("cargo/registry/cache/again.crate"), 7, ago(5));
    let stale = engine.scan_all();
    f.folders.set_env("CARGO_HOME", abs("elsewhere/cargo"));
    let report = engine.clean(&stale.confirm(), Source::Manual);
    assert!(report.rules[0].moved.is_empty());
    assert_eq!(
        report.rules[0].skipped[0].reason,
        CleanSkipReason::RootChanged
    );
}

#[test]
fn variables_are_tried_in_order_and_add_their_own_subfolder() {
    let (f, engine) = followed_fixture();
    f.folders.set_env("OTHER_HOME", abs("tools/other"));
    assert_eq!(
        only(&engine.scan_all()).root.as_deref(),
        Some(abs("tools/other/cache").as_path())
    );
    f.folders.set_env("CARGO_HOME", abs("tools/cargo"));
    assert_eq!(
        only(&engine.scan_all()).root.as_deref(),
        Some(abs("tools/cargo").as_path()),
        "the first variable of the rule wins"
    );
}

#[test]
fn a_variable_that_is_not_a_plausible_cache_folder_is_ignored() {
    let (f, engine) = followed_fixture();
    let default_root = abs("Users/tester/.cargo");
    for bad in [
        abs(""),                           // a drive root
        abs("Users/tester"),               // the profile itself
        abs("Users/tester/AppData/Local"), // a folder above protected data
        abs("Windows"),                    // a system folder
        abs("Windows/System32/cargo"),     // inside one
        abs("Program Files/cargo"),        // inside one
        PathBuf::from("relative/cargo"),   // not absolute
        abs("tools/../Windows"),           // climbs out
    ] {
        f.folders.set_env("CARGO_HOME", bad.clone());
        let plan = engine.scan_all();
        let scan = only(&plan);
        assert_eq!(scan.followed_by, None, "{bad:?} must be ignored");
        assert_eq!(
            scan.root.as_deref(),
            Some(default_root.as_path()),
            "{bad:?}"
        );
    }
}

#[test]
fn without_a_system_folder_list_protected_variables_are_still_ignored() {
    let f = profile_fixture();
    // No system folders given (the fake then reports them unsupported).
    let engine = f.engine(&cargo_rule());
    f.folders.set_env("CARGO_HOME", abs("Users/tester"));
    assert_eq!(only(&engine.scan_all()).followed_by, None);
    f.folders.set_env("CARGO_HOME", abs("tools/cargo"));
    assert_eq!(
        only(&engine.scan_all()).followed_by.as_deref(),
        Some("CARGO_HOME")
    );
}
