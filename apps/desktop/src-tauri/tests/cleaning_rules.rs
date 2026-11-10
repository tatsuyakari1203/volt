//! The shipped cleaning rules (`catalogue/cleaning.toml`), one fixture tree per rule.
//!
//! Every rule in the embedded file has a fixture here: a tree built with the in-memory fakes of
//! `volt-core` that holds entries the rule must take and entries it must leave alone (too new, on
//! the exclude list, matching no include pattern, too deep, a link, a folder that is not empty,
//! and files outside the root). For each rule the test scans the tree, compares the plan with the
//! expectation, then cleans the plan against the fake file system and checks that exactly the
//! expected entries were moved and everything else is still there.
//!
//! `every_rule_has_a_fixture_and_every_fixture_covers_what_the_rule_allows` fails when a rule is
//! added to the file without a fixture, or when a fixture does not cover a kind of entry the rule's
//! shape makes possible (for example a rule with a minimum age must have a too-new entry).
//!
//! Nothing here touches the real file system or the real settings: all of it is in memory.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::{FakeFileSystem, FakeSettings, ManualClock};
use volt_core::{
    CleanEngine, CleaningRule, CleaningRules, KnownFolders, Location, PortError, RuleScan,
    ScanFailure, Settings, SkipReason, Source, Timestamp,
};
use volt_desktop_lib::catalogue::EMBEDDED_CLEANING_RULES;

const NOW: i64 = 1_800_000_000_000;
const DAY: i64 = 86_400_000;

/// What the rule must do with one entry of a fixture tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// An old file the rule takes.
    Taken,
    /// A file exactly as old as the minimum age: taken.
    TakenAtAgeLimit,
    /// A file modified just now, for a rule with no minimum age: taken.
    TakenNew,
    /// A file a minute younger than the minimum age: left alone.
    TooNew,
    /// An old file matching an exclude pattern: left alone.
    Excluded,
    /// An old file matching no include pattern: left alone.
    NotMatching,
    /// An old file below the rule's maximum depth: left alone.
    TooDeep,
    /// A link (junction or symlink): left alone and never entered.
    Link,
    /// An old folder that is empty or emptied by the plan: taken after its content.
    DirTaken,
    /// A new empty folder: left alone.
    EmptyDirTooNew,
    /// An old folder that still holds something that is not taken: left alone.
    DirKept,
}

use Expect::{
    DirKept, DirTaken, EmptyDirTooNew, Excluded, Link, NotMatching, Taken, TakenAtAgeLimit,
    TakenNew, TooDeep, TooNew,
};

type Tree = Vec<(&'static str, Expect)>;

fn abs(rest: &str) -> PathBuf {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    let mut path = PathBuf::from(base);
    path.extend(rest.split('/'));
    path
}

fn below(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(relative.split('/'));
    path
}

fn folders() -> Arc<FakeKnownFolders> {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, abs("Users/tester/AppData/Local/Temp"));
    folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
    folders.set(
        Location::RoamingAppData,
        abs("Users/tester/AppData/Roaming"),
    );
    folders.set(Location::ProgramData, abs("ProgramData"));
    folders.set(Location::Windows, abs("Windows"));
    for protected in [
        "Users/tester",
        "Users/tester/Documents",
        "Users/tester/AppData/Local",
        "Users/tester/AppData/Roaming",
        "ProgramData",
        "Windows",
        "Program Files",
    ] {
        folders.protect(abs(protected));
    }
    folders
}

fn rules() -> CleaningRules {
    CleaningRules::from_toml(EMBEDDED_CLEANING_RULES).expect("the embedded rules are valid")
}

struct World {
    fs: Arc<FakeFileSystem>,
    settings: Arc<FakeSettings>,
    folders: Arc<FakeKnownFolders>,
    engine: CleanEngine,
}

impl World {
    fn new() -> Self {
        let fs = Arc::new(FakeFileSystem::new());
        let settings = Arc::new(FakeSettings::new());
        let folders = folders();
        let engine = CleanEngine::new(
            rules(),
            folders.clone(),
            fs.clone(),
            Arc::new(ManualClock::new(NOW)),
        )
        .with_settings(settings.clone());
        Self {
            fs,
            settings,
            folders,
            engine,
        }
    }

    fn root_of(&self, rule: &CleaningRule) -> PathBuf {
        let mut root = self
            .folders
            .location(rule.root.location())
            .expect("location");
        root.extend(rule.root.relative());
        root
    }
}

fn ago(millis: i64) -> Timestamp {
    Timestamp::from_unix_millis(NOW - millis)
}

/// Puts one entry of a fixture tree into the fake file system.
fn install(fs: &FakeFileSystem, root: &Path, relative: &str, expect: Expect, min_age_days: u32) {
    let path = below(root, relative);
    let limit = i64::from(min_age_days) * DAY;
    match expect {
        Link => {
            fs.add_link(&path);
            // What a real junction would show below it: never reached by a scan.
            fs.add_file_at(path.join("inner.tmp"), 7, ago(400 * DAY));
        }
        DirTaken | DirKept => fs.add_dir(&path, Some(ago(400 * DAY))),
        EmptyDirTooNew => fs.add_dir(&path, Some(ago(limit - 60_000))),
        Taken | Excluded | NotMatching | TooDeep => fs.add_file_at(&path, 10, ago(400 * DAY)),
        TakenAtAgeLimit => fs.add_file_at(&path, 10, ago(limit)),
        TakenNew => fs.add_file_at(&path, 10, ago(0)),
        TooNew => fs.add_file_at(&path, 10, ago(limit - 60_000)),
    }
}

/// Entries that lie outside the root and look like what the rule takes: a file in the root's
/// parent folder and one in a sibling folder that starts with the root's name.
fn outside_entries(root: &Path, tree: &Tree) -> Vec<PathBuf> {
    let first = tree
        .iter()
        .find(|(_, expect)| *expect == Taken)
        .expect("a fixture has a taken file")
        .0;
    let parent = root.parent().expect("a root has a parent");
    let name = root.file_name().expect("a root has a name");
    let last = first.rsplit('/').next().expect("a name");
    let mut sibling = parent.join(format!("{}-sibling", name.to_string_lossy()));
    sibling.extend(first.split('/'));
    vec![parent.join(last), sibling]
}

/// Scans the rule alone over its fixture, checks the plan, cleans it against the fake and checks
/// what was moved.
fn check_fixture(rule: &CleaningRule, tree: &Tree) {
    let world = World::new();
    let root = world.root_of(rule);
    let min_age = rule.constraints.min_age_days;
    for (relative, expect) in tree {
        install(&world.fs, &root, relative, *expect, min_age);
    }
    let outside = outside_entries(&root, tree);
    for path in &outside {
        world.fs.add_file_at(path, 10, ago(400 * DAY));
    }

    let plan = world.engine.scan(&[rule.id.as_str()]).expect("scan");
    let scan = &plan.rules[0];
    let id = &rule.id;
    assert_eq!(scan.failure, None, "{id}");
    assert!(scan.root_exists, "{id}: the fixture root exists");
    assert_eq!(scan.root.as_deref(), Some(root.as_path()), "{id}");
    assert_eq!(scan.min_age_days, min_age, "{id}");
    assert!(
        !scan.needs_administrator(),
        "{id}: a readable tree needs no rights"
    );

    let expected: BTreeSet<PathBuf> = tree
        .iter()
        .filter(|(_, expect)| matches!(expect, Taken | TakenAtAgeLimit | TakenNew | DirTaken))
        .map(|(relative, _)| below(&root, relative))
        .collect();
    let listed: BTreeSet<PathBuf> = scan.items.iter().map(|item| item.path.clone()).collect();
    assert_eq!(
        listed, expected,
        "{id}: the plan lists exactly what the rule must take"
    );
    assert_eq!(
        scan.items.len(),
        listed.len(),
        "{id}: no entry is listed twice"
    );

    for item in &scan.items {
        assert!(
            item.path.starts_with(&root),
            "{id}: {} is outside the root",
            item.path.display()
        );
    }
    for path in &outside {
        assert!(
            !listed.contains(path),
            "{id}: {} lies outside the root",
            path.display()
        );
    }
    for (relative, expect) in tree {
        let path = below(&root, relative);
        let skipped = |reason| {
            scan.skipped
                .iter()
                .any(|entry| entry.path == path && entry.reason == reason)
        };
        match expect {
            TooNew | EmptyDirTooNew => {
                assert!(
                    skipped(SkipReason::TooNew),
                    "{id}: {relative} is reported as too new"
                );
            }
            Link => {
                assert!(
                    skipped(SkipReason::Link),
                    "{id}: {relative} is reported as a link"
                );
                assert!(
                    !world.fs.read_dir_calls().contains(&path),
                    "{id}: the link {relative} was entered"
                );
            }
            _ => {}
        }
    }

    // Clean the plan against the fake: exactly the listed entries move, everything else stays.
    let report = world.engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.skipped_count(), 0, "{id}: {report:?}");
    let trashed: BTreeSet<PathBuf> = world.fs.trashed().into_iter().collect();
    assert_eq!(
        trashed, expected,
        "{id}: a clean moves exactly the planned entries"
    );
    for (relative, expect) in tree {
        let path = below(&root, relative);
        let should_stay = !matches!(expect, Taken | TakenAtAgeLimit | TakenNew | DirTaken);
        assert_eq!(
            world.fs.exists(&path),
            should_stay,
            "{id}: {relative} ({expect:?})"
        );
    }
    for path in &outside {
        assert!(
            world.fs.exists(path),
            "{id}: {} outside the root was touched",
            path.display()
        );
    }
}

// ---- the fixtures ---------------------------------------------------------------------------

/// A folder of error report folders (user and machine-wide copies share the shape).
fn wer_tree() -> Tree {
    vec![
        ("Report_1", DirTaken),
        ("Report_1/Report.wer", Taken),
        ("Report_1/memory.hdmp", Taken),
        ("Report_2", DirKept),
        ("Report_2/Report.wer", TooNew),
        ("Report_3", DirTaken),
        ("Report_3/Report.wer", TakenAtAgeLimit),
        ("Report_4", EmptyDirTooNew),
        ("loose.wer", Taken),
        ("link", Link),
        ("a/b/c/d/e/deep.wer", TooDeep),
    ]
}

/// The two Explorer cache rules share one folder, so each sees the other's files as strangers.
fn explorer_cache_tree(prefix: &'static str, other: &'static str) -> Tree {
    let tree: Vec<(String, Expect)> = vec![
        (format!("{prefix}_256.db"), Taken),
        (format!("{prefix}_idx.db"), Taken),
        (format!("{prefix}_sr.db"), TakenNew),
        (format!("{other}_256.db"), NotMatching),
        ("RecommendationsFilterList.json".to_owned(), NotMatching),
        (format!("{prefix}_link.db"), Link),
        (format!("sub/{prefix}_1.db"), TooDeep),
    ];
    // The fixture tree holds `&'static str`: the few names are leaked once per test run.
    tree.into_iter()
        .map(|(name, expect)| (&*Box::leak(name.into_boxed_str()), expect))
        .collect()
}

fn fixture(id: &str) -> Option<Tree> {
    Some(match id {
        "user-temp" | "windows-temp" => {
            let mut tree = vec![
                ("report.tmp", Taken),
                ("installer", DirTaken),
                ("installer/setup.log", Taken),
                ("limit.tmp", TakenAtAgeLimit),
                ("fresh.tmp", TooNew),
                ("empty-old", DirTaken),
                ("empty-new", EmptyDirTooNew),
                ("mixed", DirKept),
                ("mixed/new.tmp", TooNew),
                ("junction", Link),
                ("m1/m2/m3/m4/m5/m6/last.tmp", Taken),
                ("l1/l2/l3/l4/l5/l6/l7/too-deep.tmp", TooDeep),
            ];
            if id == "windows-temp" {
                // The folder Windows uses to service updates is never touched.
                tree.push(("CBS/servicing.log", Excluded));
                tree.push(("cbs/other.log", Excluded));
            }
            tree
        }
        "user-wer-archive" | "user-wer-queue" | "machine-wer-archive" | "machine-wer-queue" => {
            wer_tree()
        }
        "user-crash-dumps" => vec![
            ("app.1234.dmp", Taken),
            ("sub/app.5678.dmp", Taken),
            ("APP.UPPER.DMP", Taken),
            ("limit.dmp", TakenAtAgeLimit),
            ("fresh.dmp", TooNew),
            ("notes.txt", NotMatching),
            ("app.dmp.txt", NotMatching),
            ("link.dmp", Link),
            ("a/b/c/deep.dmp", TooDeep),
        ],
        "thumbnail-cache" => explorer_cache_tree("thumbcache", "iconcache"),
        "icon-cache" => explorer_cache_tree("iconcache", "thumbcache"),
        "windows-update-logs" => vec![
            ("WindowsUpdate.20240101.1.etl", Taken),
            ("limit.etl", TakenAtAgeLimit),
            ("fresh.etl", TooNew),
            ("readme.txt", NotMatching),
            ("trace.etl.bak", NotMatching),
            ("link.etl", Link),
            ("sub/nested.etl", TooDeep),
        ],
        "delivery-optimization-cache" => vec![
            ("content-1", Taken),
            ("sub/blob", Taken),
            ("d1/d2/d3/d4/last", Taken),
            ("limit", TakenAtAgeLimit),
            ("fresh", TooNew),
            ("link", Link),
            ("e1/e2/e3/e4/e5/too-deep", TooDeep),
        ],
        _ => return None,
    })
}

#[test]
fn every_rule_scans_and_cleans_its_fixture_exactly() {
    for rule in rules().rules() {
        let tree = fixture(&rule.id).unwrap_or_else(|| panic!("no fixture for rule {}", rule.id));
        check_fixture(rule, &tree);
    }
}

#[test]
fn every_rule_has_a_fixture_and_every_fixture_covers_what_the_rule_allows() {
    let rules = rules();
    let ids: BTreeSet<&str> = rules.rules().iter().map(|rule| rule.id.as_str()).collect();
    // A fixture for a rule that does not exist is a leftover: it fails too.
    for known in [
        "user-temp",
        "windows-temp",
        "user-wer-archive",
        "user-wer-queue",
        "machine-wer-archive",
        "machine-wer-queue",
        "user-crash-dumps",
        "thumbnail-cache",
        "icon-cache",
        "windows-update-logs",
        "delivery-optimization-cache",
    ] {
        assert!(ids.contains(known), "fixture for {known} but no such rule");
    }

    for rule in rules.rules() {
        let id = &rule.id;
        let tree = fixture(id)
            .unwrap_or_else(|| panic!("rule {id} has no fixture: add one to cleaning_rules.rs"));
        let has = |wanted: Expect| tree.iter().any(|(_, expect)| *expect == wanted);
        let min_age = rule.constraints.min_age_days;
        assert!(
            has(Taken),
            "{id}: the fixture needs an entry the rule takes"
        );
        assert!(has(Link), "{id}: the fixture needs a link");
        assert!(
            has(TooDeep),
            "{id}: the fixture needs an entry below the depth limit"
        );
        if rule.include.iter().all(|pattern| pattern.as_str() != "**") {
            assert!(
                has(NotMatching),
                "{id}: the fixture needs an entry matching no include"
            );
        }
        if !rule.exclude.is_empty() {
            assert!(has(Excluded), "{id}: the fixture needs an excluded entry");
        }
        if min_age > 0 {
            assert!(has(TooNew), "{id}: the fixture needs a too-new entry");
            assert!(
                has(TakenAtAgeLimit),
                "{id}: the fixture needs an entry at the age limit"
            );
        } else {
            assert!(
                has(TakenNew),
                "{id}: with no minimum age a brand-new file is taken"
            );
        }
        if rule.constraints.remove_empty_dirs {
            assert!(
                has(DirTaken),
                "{id}: the fixture needs a folder that is taken"
            );
            assert!(
                has(EmptyDirTooNew),
                "{id}: the fixture needs a too-new empty folder"
            );
            assert!(
                has(DirKept),
                "{id}: the fixture needs a folder that is kept"
            );
        }
        // The fixture must also be honest about the rule: no taken folder when folders stay.
        if !rule.constraints.remove_empty_dirs {
            assert!(
                !has(DirTaken) && !has(EmptyDirTooNew) && !has(DirKept),
                "{id}"
            );
        }
    }
}

// ---- the minimum age setting ------------------------------------------------------------------

const AGE_KEY: &str = "clean.temp_min_age_days";

#[test]
fn exactly_the_temp_rules_use_the_configurable_age() {
    let rules = rules();
    let with_setting: BTreeSet<&str> = rules
        .rules()
        .iter()
        .filter(|rule| rule.min_age_setting.is_some())
        .map(|rule| rule.id.as_str())
        .collect();
    assert_eq!(with_setting, BTreeSet::from(["user-temp", "windows-temp"]));
    for rule in rules
        .rules()
        .iter()
        .filter(|rule| rule.min_age_setting.is_some())
    {
        assert_eq!(rule.min_age_setting.as_deref(), Some(AGE_KEY));
    }
}

#[test]
fn a_file_newer_than_the_configured_age_is_never_listed_by_the_temp_rules() {
    for id in ["user-temp", "windows-temp"] {
        let world = World::new();
        let rule = world.engine.rules().get(id).expect("rule").clone();
        let root = world.root_of(&rule);
        world
            .fs
            .add_file_at(below(&root, "year.tmp"), 1, ago(400 * DAY));
        world
            .fs
            .add_file_at(below(&root, "thirty-one.tmp"), 1, ago(31 * DAY));
        world
            .fs
            .add_file_at(below(&root, "ten.tmp"), 1, ago(10 * DAY));
        world.fs.add_file_at(below(&root, "today.tmp"), 1, ago(0));

        // The rule's own age (7 days) takes the 10 day old file.
        let plan = world.engine.scan(&[id]).expect("scan");
        assert_eq!(plan.rules[0].min_age_days, 7);
        assert_eq!(plan.rules[0].file_count(), 3, "{id}");

        // A month in the settings leaves it, and everything newer, alone.
        world
            .settings
            .set(AGE_KEY, &serde_json::json!(30))
            .expect("set");
        let plan = world.engine.scan(&[id]).expect("scan");
        let scan = &plan.rules[0];
        assert_eq!(scan.min_age_days, 30);
        let listed: BTreeSet<PathBuf> = scan.items.iter().map(|item| item.path.clone()).collect();
        assert_eq!(
            listed,
            BTreeSet::from([below(&root, "year.tmp"), below(&root, "thirty-one.tmp")]),
            "{id}"
        );

        // A value that is not a usable number of days stops the rule instead of guessing.
        world
            .settings
            .set(AGE_KEY, &serde_json::json!(0))
            .expect("set");
        let plan = world.engine.scan(&[id]).expect("scan");
        assert_eq!(
            plan.rules[0].failure,
            Some(ScanFailure::MinAgeSettingInvalid)
        );
        assert!(plan.rules[0].items.is_empty());
    }
}

#[test]
fn the_other_rules_ignore_the_temp_age_setting() {
    let world = World::new();
    world
        .settings
        .set(AGE_KEY, &serde_json::json!(1))
        .expect("set");
    let plan = world.engine.scan_all();
    for scan in &plan.rules {
        let rule = world.engine.rules().get(&scan.rule_id).expect("rule");
        let expected = if rule.min_age_setting.is_some() {
            1
        } else {
            rule.constraints.min_age_days
        };
        assert_eq!(scan.min_age_days, expected, "{}", scan.rule_id);
    }
}

// ---- administrator rights ---------------------------------------------------------------------

#[test]
fn a_refused_root_says_it_needs_administrator_rights_only_for_the_rules_that_do() {
    let rules = rules();
    for rule in rules.rules() {
        let world = World::new();
        let root = world.root_of(rule);
        world.fs.add_dir(&root, Some(ago(400 * DAY)));
        world
            .fs
            .fail_read_dir_of(&root, PortError::AccessDenied("denied".to_owned()));
        let plan = world.engine.scan(&[rule.id.as_str()]).expect("scan");
        let scan: &RuleScan = &plan.rules[0];
        assert_eq!(
            scan.failure,
            Some(ScanFailure::RootAccessDenied),
            "{}",
            rule.id
        );
        assert!(scan.items.is_empty(), "{}", rule.id);
        assert_eq!(scan.requires_admin, rule.requires_admin, "{}", rule.id);
        assert_eq!(
            scan.needs_administrator(),
            rule.requires_admin,
            "{}",
            rule.id
        );
    }
}

#[test]
fn a_missing_root_is_nothing_to_clean_and_never_a_sign_of_missing_rights() {
    let world = World::new();
    let plan = world.engine.scan_all();
    assert!(plan.is_empty());
    for scan in &plan.rules {
        assert_eq!(scan.failure, None, "{}", scan.rule_id);
        assert!(!scan.root_exists, "{}", scan.rule_id);
        assert!(!scan.needs_administrator(), "{}", scan.rule_id);
    }
}
