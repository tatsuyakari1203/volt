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
    folders.set(Location::UserProfile, abs("Users/tester"));
    folders.set(Location::LocalLow, abs("Users/tester/AppData/LocalLow"));
    for protected in [
        "Users/tester",
        "Users/tester/Documents",
        "Users/tester/AppData/Local",
        "Users/tester/AppData/Roaming",
        "Users/tester/AppData/LocalLow",
        "ProgramData",
        "Windows",
        "Program Files",
    ] {
        folders.protect(abs(protected));
    }
    folders
}

/// Rules whose items are plain files below a named location: the ones the generic fixtures below
/// cover. Rules that take whole folders, are picked by item or work in a folder the user adds
/// have their own tests (`items_and_folders` further down).
fn is_standard(rule: &CleaningRule) -> bool {
    rule.unit == volt_core::ItemUnit::Files && !rule.needs_chosen_folder()
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

/// A Chromium profile folder tree: two profiles, caches to take, the user's own data to keep.
fn chromium_tree() -> Tree {
    vec![
        ("Default/Cache/Cache_Data/f_000001", Taken),
        ("Default/Cache/Cache_Data/index", Taken),
        ("Default/Cache/a/b/last", Taken),
        ("Profile 1/Code Cache/js/abc_0", Taken),
        ("Default/GPUCache/data_0", Taken),
        ("Default/DawnGraphiteCache/data_0", Taken),
        ("Profile 1/DawnWebGPUCache/data_1", Taken),
        ("Default/DawnCache/data_2", Taken),
        ("GrShaderCache/data_0", Taken),
        ("GraphiteDawnCache/data_0", Taken),
        ("ShaderCache/GPUCache/data_0", Taken),
        ("Default/Service Worker/CacheStorage/origin/entry", Excluded),
        ("component_crx_cache/package", NotMatching),
        ("Default/GPUCache/limit", TakenAtAgeLimit),
        ("Default/GPUCache/fresh", TooNew),
        ("Default/Cookies", NotMatching),
        ("Default/Network/Cookies", Excluded),
        ("Default/History", NotMatching),
        ("Default/Login Data", NotMatching),
        ("Default/Extensions/ext1/Cache/blob", Excluded),
        ("Local State", NotMatching),
        ("Default/Cache/link", Link),
        ("Default/Cache/a/b/c/d/deep", TooDeep),
    ]
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
        "chrome-cache" | "edge-cache" | "brave-cache" | "vivaldi-cache" => chromium_tree(),
        "firefox-cache" => vec![
            ("abcd1234.default-release/cache2/entries/ABCDEF", Taken),
            ("abcd1234.default-release/cache2/index", Taken),
            ("abcd1234.default-release/cache2/doomed/old", Taken),
            ("efgh5678.dev-edition/cache2/entries/FEDCBA", Taken),
            ("abcd1234.default-release/cache2/limit", TakenAtAgeLimit),
            ("abcd1234.default-release/cache2/fresh", TooNew),
            ("abcd1234.default-release/cookies.sqlite", NotMatching),
            ("abcd1234.default-release/places.sqlite", NotMatching),
            ("abcd1234.default-release/key4.db", NotMatching),
            (
                "abcd1234.default-release/storage/default/site/ls/data.sqlite",
                Excluded,
            ),
            ("abcd1234.default-release/cache2/link", Link),
            ("abcd1234.default-release/cache2/a/b/c/d/deep", TooDeep),
        ],
        _ => return shader_fixture(id),
    })
}

/// The shader cache rules.
fn shader_fixture(id: &str) -> Option<Tree> {
    Some(match id {
        "directx-shader-cache" => vec![
            ("a1b2c3d4/1.idx", Taken),
            ("a1b2c3d4/1.bin", Taken),
            ("e5f6a7b8/2.bin", Taken),
            ("loose.bin", Taken),
            ("limit.bin", TakenAtAgeLimit),
            ("fresh.bin", TooNew),
            ("link", Link),
            ("x/y/z/w/deep.bin", TooDeep),
        ],
        "nvidia-shader-cache" => vec![
            ("DXCache/0002a9db011577a1.nvph", Taken),
            ("DXCache/0002a9db12df60d8.toc", Taken),
            ("GLCache/ab/cd/shader.bin", Taken),
            ("DXCache/limit.nvph", TakenAtAgeLimit),
            ("GLCache/fresh.bin", TooNew),
            ("OptixCache/optix.db", NotMatching),
            ("warp/kernel.bin", NotMatching),
            ("DXCache/link", Link),
            ("DXCache/a/b/c/deep.nvph", TooDeep),
        ],
        "nvidia-driver-shader-cache" => vec![
            ("DXCache/0002a9db011577a1.nvph", Taken),
            ("GLCache/ab/cd/shader.bin", Taken),
            ("DXCache/limit.nvph", TakenAtAgeLimit),
            ("GLCache/fresh.bin", TooNew),
            ("Settings/profile.bin", NotMatching),
            ("DXCache/link", Link),
            ("DXCache/a/b/c/deep.nvph", TooDeep),
        ],
        "nvidia-compute-cache" => vec![
            ("0/1.bin", Taken),
            ("5/2/3.bin", Taken),
            ("limit.bin", TakenAtAgeLimit),
            ("fresh.bin", TooNew),
            ("link", Link),
            ("a/b/c/d/deep.bin", TooDeep),
        ],
        "amd-shader-cache" => vec![
            ("DxCache/0a1b.bin", Taken),
            ("DxcCache/sub/2c3d.bin", Taken),
            ("GLCache/4e5f.bin", Taken),
            ("DxCache/limit.bin", TakenAtAgeLimit),
            ("GLCache/fresh.bin", TooNew),
            ("Settings/profile.xml", NotMatching),
            ("VkCache/vk.bin", NotMatching),
            ("DxCache/link", Link),
            ("DxCache/a/b/c/deep.bin", TooDeep),
        ],
        _ => return app_fixture(id),
    })
}

/// The chat app cache rules.
fn app_fixture(id: &str) -> Option<Tree> {
    Some(match id {
        "discord-cache" => vec![
            ("Cache/Cache_Data/f_000001", Taken),
            ("Code Cache/js/abc_0", Taken),
            ("GPUCache/data_0", Taken),
            ("ShaderCache/index", Taken),
            ("DawnGraphiteCache/data_1", Taken),
            ("GPUPersistentCache/DawnGraphiteCache/x", Taken),
            ("GPUCache/limit", TakenAtAgeLimit),
            ("GPUCache/fresh", TooNew),
            ("Local Storage/leveldb/000003.log", Excluded),
            ("Network/Cookies", Excluded),
            ("settings.json", NotMatching),
            ("discord_asset_cache/voice_filters/f", NotMatching),
            ("Cache/link", Link),
            ("Cache/a/b/c/d/deep", TooDeep),
        ],
        "telegram-cache" => vec![
            ("tdata/user_data/cache/0/f1", Taken),
            ("tdata/user_data/media_cache/1/f2", Taken),
            ("tdata/user_data#2/cache/0/f3", Taken),
            ("tdata/user_data/cache/limit", TakenAtAgeLimit),
            ("tdata/user_data/cache/fresh", TooNew),
            ("tdata/D877F783D5D3EF8C/map1", Excluded),
            ("tdata/key_datas", Excluded),
            ("tdata/user_data/emoji/e1", NotMatching),
            ("Telegram.exe", NotMatching),
            ("tdata/user_data/cache/link", Link),
            ("tdata/user_data/cache/a/b/c/deep", TooDeep),
        ],
        "teams-cache" => vec![
            ("EBWebView/Default/Cache/Cache_Data/f_000001", Taken),
            ("EBWebView/Default/Code Cache/js/abc_0", Taken),
            ("EBWebView/Profile 1/GPUCache/data_0", Taken),
            ("EBWebView/Default/GPUCache/limit", TakenAtAgeLimit),
            ("EBWebView/Default/GPUCache/fresh", TooNew),
            ("EBWebView/Default/Network/Cookies", Excluded),
            ("EBWebView/Default/Local Storage/leveldb/x", Excluded),
            ("EBWebView/Default/History", NotMatching),
            ("settings.json", NotMatching),
            ("EBWebView/Default/Cache/link", Link),
            ("EBWebView/Default/Cache/a/b/c/deep", TooDeep),
        ],
        "teams-classic-cache" => vec![
            ("Cache/f_000001", Taken),
            ("Code Cache/js/abc_0", Taken),
            ("GPUCache/data_0", Taken),
            ("GPUCache/limit", TakenAtAgeLimit),
            ("GPUCache/fresh", TooNew),
            ("databases/db/x", Excluded),
            ("Local Storage/leveldb/x", Excluded),
            ("Cookies", NotMatching),
            ("Cache/link", Link),
            ("Cache/a/b/c/d/deep", TooDeep),
        ],
        "zalo-cache" => vec![
            ("Cache/Cache_Data/f_000001", Taken),
            ("Code Cache/js/abc_0", Taken),
            ("GPUCache/data_0", Taken),
            ("DawnCache/data_1", Taken),
            ("GPUCache/limit", TakenAtAgeLimit),
            ("GPUCache/fresh", TooNew),
            ("Database/messages.db", Excluded),
            ("media/1/photo.jpg", Excluded),
            ("Local Storage/leveldb/x", Excluded),
            ("resp_cache/404/x", NotMatching),
            ("Cache/link", Link),
            ("Cache/a/b/c/d/deep", TooDeep),
        ],
        _ => return dev_fixture(id),
    })
}

/// A path far below any developer cache rule's depth limit.
const VERY_DEEP: &str = "a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/deep.bin";

/// The developer tool cache rules. A rule that takes `**` has no non-matching entries; the ones
/// that pick folders out of a shared root also hold what the tool keeps there (never listed).
fn dev_fixture(id: &str) -> Option<Tree> {
    Some(match id {
        "npm-cache"
        | "pnpm-store"
        | "pnpm-metadata-cache"
        | "pip-cache"
        | "nuget-http-cache"
        | "nuget-packages"
        | "gradle-caches"
        | "yarn-cache"
        | "yarn-berry-cache" => vec![
            ("a/b/package.tgz", Taken),
            ("index-v5/ab/cd/entry", Taken),
            ("loose.bin", Taken),
            ("limit.bin", TakenAtAgeLimit),
            ("fresh.bin", TooNew),
            ("link", Link),
            (VERY_DEEP, TooDeep),
        ],
        "cargo-cache" => vec![
            (
                "registry/cache/index.crates.io-1949cf8c6b5b557f/serde-1.0.0.crate",
                Taken,
            ),
            (
                "registry/src/index.crates.io-1949cf8c6b5b557f/serde-1.0.0/src/lib.rs",
                Taken,
            ),
            ("git/checkouts/repo-abc123/1a2b3c/Cargo.toml", Taken),
            ("registry/cache/limit.crate", TakenAtAgeLimit),
            ("registry/src/fresh.rs", TooNew),
            (
                "registry/index/index.crates.io-1949cf8c6b5b557f/.cache/se/rd/serde",
                NotMatching,
            ),
            ("git/db/repo-abc123/HEAD", NotMatching),
            ("bin/cargo-nextest.exe", NotMatching),
            ("credentials.toml", NotMatching),
            ("config.toml", NotMatching),
            ("registry/cache/link", Link),
            (
                "registry/src/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/deep.rs",
                TooDeep,
            ),
        ],
        _ => return None,
    })
}

#[test]
fn every_rule_scans_and_cleans_its_fixture_exactly() {
    for rule in rules().rules().iter().filter(|rule| is_standard(rule)) {
        let tree = fixture(&rule.id).unwrap_or_else(|| panic!("no fixture for rule {}", rule.id));
        check_fixture(rule, &tree);
    }
}

/// Every rule that has a fixture; a fixture for a rule that does not exist is a leftover.
const KNOWN_RULES: [&str; 40] = [
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
    "chrome-cache",
    "edge-cache",
    "brave-cache",
    "vivaldi-cache",
    "firefox-cache",
    "directx-shader-cache",
    "nvidia-shader-cache",
    "nvidia-compute-cache",
    "amd-shader-cache",
    "discord-cache",
    "telegram-cache",
    "teams-cache",
    "teams-classic-cache",
    "zalo-cache",
    "npm-cache",
    "pnpm-store",
    "pnpm-metadata-cache",
    "pip-cache",
    "cargo-cache",
    "nuget-http-cache",
    "nuget-packages",
    "gradle-caches",
    "yarn-cache",
    "yarn-berry-cache",
    "nvidia-driver-shader-cache",
    "huggingface-hub-cache",
    "torch-hub-cache",
    "ollama-models",
    "comfyui-temp",
];

#[test]
fn every_rule_has_a_fixture_and_every_fixture_covers_what_the_rule_allows() {
    let rules = rules();
    let ids: BTreeSet<&str> = rules.rules().iter().map(|rule| rule.id.as_str()).collect();
    // A fixture for a rule that does not exist is a leftover: it fails too.
    for known in KNOWN_RULES {
        assert!(ids.contains(known), "fixture for {known} but no such rule");
    }

    for rule in rules.rules().iter().filter(|rule| is_standard(rule)) {
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
    for rule in rules
        .rules()
        .iter()
        .filter(|rule| !rule.needs_chosen_folder())
    {
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
        // A rule that waits for a folder the user adds is not scanned yet, and says so.
        let waiting = scan.rule_id == "comfyui-temp";
        let expected = waiting.then_some(ScanFailure::Root(
            volt_core::clean::RootError::FolderNotChosen,
        ));
        assert_eq!(scan.failure, expected, "{}", scan.rule_id);
        assert!(!scan.root_exists, "{}", scan.rule_id);
        assert!(!scan.needs_administrator(), "{}", scan.rule_id);
    }
}

// ---- browser caches ---------------------------------------------------------------------------

const BROWSER_RULES: [&str; 5] = [
    "chrome-cache",
    "edge-cache",
    "brave-cache",
    "vivaldi-cache",
    "firefox-cache",
];
/// Cache folders directly inside a profile folder.
const CACHE_FOLDERS: [&str; 7] = [
    "cache",
    "code cache",
    "gpucache",
    "cache2",
    "dawncache",
    "dawngraphitecache",
    "dawnwebgpucache",
];
/// GPU cache folders directly in a Chromium "User Data" folder, beside the profile folders.
const TOP_LEVEL_CACHES: [&str; 3] = ["grshadercache", "graphitedawncache", "shadercache"];

/// Whether `parts` (lower case, below the root) is a file inside a cache folder.
fn in_cache_folder(parts: &[String]) -> bool {
    (parts.len() >= 3 && CACHE_FOLDERS.contains(&parts[1].as_str()))
        || (parts.len() >= 2 && TOP_LEVEL_CACHES.contains(&parts[0].as_str()))
}

/// What a browser keeps next to its cache, in Chromium and Firefox profiles alike. A clean must
/// never list any of it.
const USER_DATA: [&str; 24] = [
    "Cookies",
    "History",
    "Login Data",
    "Web Data",
    "Bookmarks",
    "Preferences",
    "Favicons",
    "Top Sites",
    "Sessions",
    "Local Storage",
    "IndexedDB",
    "Extensions",
    "Local Extension Settings",
    "Network",
    "Service Worker",
    "cookies.sqlite",
    "places.sqlite",
    "key4.db",
    "logins.json",
    "formhistory.sqlite",
    "prefs.js",
    "sessionstore-backups",
    "extensions",
    "storage",
];

/// A realistic set of profiles for a rule: cache entries to take, and every kind of user data
/// (as files and as folders with files inside) beside them. Returns the root.
fn realistic_profiles(world: &World, rule: &CleaningRule) -> PathBuf {
    let root = world.root_of(rule);
    let firefox = rule.id == "firefox-cache";
    let profiles: &[&str] = if firefox {
        &["abcd1234.default-release", "efgh5678.dev-edition"]
    } else {
        &["Default", "Profile 1", "Guest Profile"]
    };
    let caches: &[&str] = if firefox {
        &["cache2/entries/ABCDEF", "cache2/index"]
    } else {
        &[
            "Cache/Cache_Data/f_000001",
            "Code Cache/js/abc_0",
            "GPUCache/data_0",
            "DawnGraphiteCache/data_0",
        ]
    };
    for profile in profiles {
        for cache in caches {
            let path = below(&root, &format!("{profile}/{cache}"));
            world.fs.add_file_at(path, 100, ago(30 * DAY));
        }
        for data in USER_DATA {
            let path = below(&root, &format!("{profile}/{data}/entry.bin"));
            world.fs.add_file_at(path, 100, ago(30 * DAY));
        }
    }
    if !firefox {
        for top in ["GrShaderCache/data_0", "ShaderCache/GPUCache/data_0"] {
            world.fs.add_file_at(below(&root, top), 100, ago(30 * DAY));
        }
    }
    world
        .fs
        .add_file_at(below(&root, "Local State"), 10, ago(30 * DAY));
    root
}

/// The path below the root as lower-case components.
fn parts_below(root: &Path, path: &Path) -> Vec<String> {
    path.strip_prefix(root)
        .expect("inside the root")
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

#[test]
fn browser_rules_never_list_cookies_history_logins_sessions_bookmarks_or_extensions() {
    for id in BROWSER_RULES {
        let world = World::new();
        let rule = world.engine.rules().get(id).expect("rule").clone();
        let root = realistic_profiles(&world, &rule);
        let plan = world.engine.scan(&[id]).expect("scan");
        let scan = &plan.rules[0];
        assert!(scan.items.len() >= 4, "{id}: caches are found per profile");
        for item in &scan.items {
            let parts = parts_below(&root, &item.path);
            // Always a file inside <profile>/<cache folder>, never user data.
            assert!(
                in_cache_folder(&parts),
                "{id}: {parts:?} is not inside a cache folder"
            );
        }
        // A clean moves nothing else either, and leaves every user data file in place.
        let report = world.engine.clean(&plan.confirm(), Source::Manual);
        assert_eq!(report.skipped_count(), 0, "{id}");
        for trashed in world.fs.trashed() {
            let parts = parts_below(&root, &trashed);
            assert!(in_cache_folder(&parts), "{id}: {parts:?} was moved");
        }
        assert!(
            world.fs.exists(&below(&root, "Local State")),
            "{id}: files in the root stay"
        );
        let user_data = world.fs.trashed().iter().any(|path| {
            USER_DATA
                .iter()
                .any(|name| parts_below(&root, path).contains(&name.to_lowercase()))
        });
        assert!(!user_data, "{id}: user data was moved");
    }
}

#[test]
fn browser_rules_are_ordinary_safe_rules_in_the_browser_group() {
    let rules = rules();
    for id in BROWSER_RULES {
        let rule = rules.get(id).expect("rule");
        assert_eq!(
            rule.category,
            volt_core::CleanCategory::BrowserCache,
            "{id}"
        );
        assert!(!rule.requires_admin, "{id}");
        assert_eq!(rule.root.location(), Location::LocalAppData, "{id}");
        assert!(
            !rule.locked_by.is_empty(),
            "{id}: a browser keeps its cache open"
        );
        assert!(rule.constraints.min_age_days >= 1, "{id}");
    }
}

#[test]
fn a_running_browser_is_named_by_the_scan_and_a_stopped_one_is_not() {
    let world = World::new();
    let processes = Arc::new(volt_core::fakes::FakeProcesses::new());
    let engine = CleanEngine::new(
        rules(),
        world.folders.clone(),
        world.fs.clone(),
        Arc::new(ManualClock::new(NOW)),
    )
    .with_processes(processes.clone());

    let plan = engine.scan(&BROWSER_RULES).expect("scan");
    assert!(
        plan.rules
            .iter()
            .all(|scan| scan.running_programs.is_empty())
    );

    processes.add(10, "Chrome.EXE", 0);
    processes.add(11, "firefox.exe", 0);
    processes.add(12, "notepad.exe", 0);
    let plan = engine.scan(&BROWSER_RULES).expect("scan");
    let running = |id: &str| {
        plan.rules
            .iter()
            .find(|scan| scan.rule_id == id)
            .expect("scan")
            .running_programs
            .clone()
    };
    assert_eq!(running("chrome-cache"), ["chrome.exe"]);
    assert_eq!(running("firefox-cache"), ["firefox.exe"]);
    assert!(running("edge-cache").is_empty());
    assert!(running("brave-cache").is_empty());
    assert!(running("vivaldi-cache").is_empty());
}

// ---- shader caches and chat app caches --------------------------------------------------------

const SHADER_RULES: [&str; 5] = [
    "directx-shader-cache",
    "nvidia-shader-cache",
    "nvidia-driver-shader-cache",
    "nvidia-compute-cache",
    "amd-shader-cache",
];
const CHAT_RULES: [&str; 5] = [
    "discord-cache",
    "telegram-cache",
    "teams-cache",
    "teams-classic-cache",
    "zalo-cache",
];

#[test]
fn shader_and_chat_rules_are_safe_user_rules_that_say_what_is_rebuilt() {
    let rules = rules();
    for id in SHADER_RULES.iter().chain(CHAT_RULES.iter()) {
        let rule = rules.get(id).expect("rule");
        let expected = if SHADER_RULES.contains(id) {
            volt_core::CleanCategory::ShaderCache
        } else {
            volt_core::CleanCategory::ChatAppCache
        };
        assert_eq!(rule.category, expected, "{id}");
        assert!(!rule.requires_admin, "{id}");
        assert!(rule.constraints.min_age_days >= 1, "{id}");
        assert!(
            rule.description.to_lowercase().contains("afterwards"),
            "{id}: the description says what happens afterwards"
        );
        if CHAT_RULES.contains(id) {
            assert!(
                !rule.locked_by.is_empty(),
                "{id}: a running app holds files"
            );
            assert!(!rule.exclude.is_empty(), "{id}: user data is excluded too");
        }
    }
}

/// One chat app: the cache files a clean may list, and what the app keeps next to them that a
/// clean must never list (messages, databases, received or saved files, login and session state).
struct ChatProfile {
    id: &'static str,
    caches: &'static [&'static str],
    protected: &'static [&'static str],
}

const CHAT_PROFILES: [ChatProfile; 5] = [
    ChatProfile {
        id: "zalo-cache",
        caches: &[
            "Cache/Cache_Data/f_000001",
            "Code Cache/js/abc_0",
            "GPUCache/data_0",
            "DawnCache/data_1",
        ],
        protected: &[
            "Database/messages.db",
            "Database/index/1.ldb",
            "media/1234/photo.jpg",
            "media/1234/received.pdf",
            "Partitions/zalo/Cookies",
            "Partitions/zalo/Local Storage/leveldb/000003.log",
            "Local Storage/leveldb/000003.log",
            "Session Storage/000003.log",
            "Network/Cookies",
            "pin-vault-data/vault.bin",
            "cal/calendar.db",
            "trust_path/trust.bin",
            "resp_cache/404/entry",
            "blob_storage/blob",
            "Preferences",
        ],
    },
    ChatProfile {
        id: "discord-cache",
        caches: &[
            "Cache/Cache_Data/f_000001",
            "Code Cache/js/abc_0",
            "GPUCache/data_0",
            "ShaderCache/index",
            "DawnGraphiteCache/data_1",
            "GPUPersistentCache/DawnGraphiteCache/x",
        ],
        protected: &[
            "Local Storage/leveldb/000003.log",
            "Session Storage/000003.log",
            "IndexedDB/https_discord.com_0.indexeddb.leveldb/000003.log",
            "Network/Cookies",
            "Network/Trust Tokens",
            "Service Worker/Database/000003.log",
            "Shared Dictionary/db",
            "WebStorage/QuotaManager",
            "settings.json",
            "sentry/session.json",
            "discord_asset_cache/voice_filters/f",
            "component_crx_cache/metadata.json",
            "logs/discord.log",
        ],
    },
    ChatProfile {
        id: "telegram-cache",
        caches: &[
            "tdata/user_data/cache/0/f1",
            "tdata/user_data/cache/12/a/f2",
            "tdata/user_data/media_cache/1/f3",
            "tdata/user_data#2/cache/0/f4",
        ],
        protected: &[
            "tdata/D877F783D5D3EF8C/map1",
            "tdata/D877F783D5D3EF8Cs",
            "tdata/key_datas",
            "tdata/settingss",
            "tdata/usertag",
            "tdata/user_data/emoji/e1",
            "tdata/user_data/stickers/s1",
            "tdata/emoji/e2",
            "tdata/countries",
            "tdata/shortcuts-default.json",
            "Telegram.exe",
            "Updater.exe",
            "log.txt",
        ],
    },
    ChatProfile {
        id: "teams-cache",
        caches: &[
            "EBWebView/Default/Cache/Cache_Data/f_000001",
            "EBWebView/Default/Code Cache/js/abc_0",
            "EBWebView/Default/GPUCache/data_0",
            "EBWebView/Profile 1/Cache/Cache_Data/f_000002",
        ],
        protected: &[
            "EBWebView/Default/Network/Cookies",
            "EBWebView/Default/Local Storage/leveldb/000003.log",
            "EBWebView/Default/Session Storage/000003.log",
            "EBWebView/Default/IndexedDB/https_teams.microsoft.com_0.indexeddb.leveldb/000003.log",
            "EBWebView/Default/Service Worker/Database/000003.log",
            "EBWebView/Default/History",
            "EBWebView/Default/Login Data",
            "EBWebView/Local State",
            "tenants/tenant.json",
            "settings.json",
        ],
    },
    ChatProfile {
        id: "teams-classic-cache",
        caches: &["Cache/f_000001", "Code Cache/js/abc_0", "GPUCache/data_0"],
        protected: &[
            "Cookies",
            "databases/db/x",
            "IndexedDB/https_teams.microsoft.com_0.indexeddb.leveldb/000003.log",
            "Local Storage/leveldb/000003.log",
            "Session Storage/000003.log",
            "Network/Cookies",
            "blob_storage/b",
            "storage.json",
            "desktop-config.json",
            "settings.json",
        ],
    },
];

#[test]
fn chat_rules_list_only_cache_folders_and_never_messages_files_or_login_state() {
    for profile in &CHAT_PROFILES {
        let id = profile.id;
        let world = World::new();
        let rule = world.engine.rules().get(id).expect("rule").clone();
        let root = world.root_of(&rule);
        for cache in profile.caches {
            world
                .fs
                .add_file_at(below(&root, cache), 100, ago(30 * DAY));
        }
        for kept in profile.protected {
            world.fs.add_file_at(below(&root, kept), 100, ago(30 * DAY));
        }

        let plan = world.engine.scan(&[id]).expect("scan");
        let scan = &plan.rules[0];
        let listed: BTreeSet<PathBuf> = scan.items.iter().map(|item| item.path.clone()).collect();
        let expected: BTreeSet<PathBuf> = profile
            .caches
            .iter()
            .map(|cache| below(&root, cache))
            .collect();
        assert_eq!(listed, expected, "{id}: exactly the cache files are listed");

        let report = world.engine.clean(&plan.confirm(), Source::Manual);
        assert_eq!(report.skipped_count(), 0, "{id}");
        let trashed: BTreeSet<PathBuf> = world.fs.trashed().into_iter().collect();
        assert_eq!(trashed, expected, "{id}: a clean moves only cache files");
        for kept in profile.protected {
            assert!(
                world.fs.exists(&below(&root, kept)),
                "{id}: {kept} was touched"
            );
        }
    }
}

#[test]
fn a_running_chat_app_is_named_by_the_scan() {
    let world = World::new();
    let processes = Arc::new(volt_core::fakes::FakeProcesses::new());
    let engine = CleanEngine::new(
        rules(),
        world.folders.clone(),
        world.fs.clone(),
        Arc::new(ManualClock::new(NOW)),
    )
    .with_processes(processes.clone());
    processes.add(10, "Zalo.EXE", 0);
    processes.add(11, "Discord.exe", 0);
    let plan = engine.scan(&CHAT_RULES).expect("scan");
    let running = |id: &str| {
        plan.rules
            .iter()
            .find(|scan| scan.rule_id == id)
            .expect("scan")
            .running_programs
            .clone()
    };
    assert_eq!(running("zalo-cache"), ["zalo.exe"]);
    assert_eq!(running("discord-cache"), ["discord.exe"]);
    assert!(running("telegram-cache").is_empty());
}

// ---- developer tool caches ----------------------------------------------------------------------

const DEV_RULES: [&str; 10] = [
    "npm-cache",
    "pnpm-store",
    "pnpm-metadata-cache",
    "pip-cache",
    "cargo-cache",
    "nuget-http-cache",
    "nuget-packages",
    "gradle-caches",
    "yarn-cache",
    "yarn-berry-cache",
];

#[test]
fn developer_rules_are_user_rules_in_their_own_group_that_say_what_is_downloaded_again() {
    let rules = rules();
    let in_group: BTreeSet<&str> = rules
        .rules()
        .iter()
        .filter(|rule| rule.category == volt_core::CleanCategory::DeveloperCache)
        .map(|rule| rule.id.as_str())
        .collect();
    assert_eq!(in_group, BTreeSet::from(DEV_RULES));
    for id in DEV_RULES {
        let rule = rules.get(id).expect("rule");
        assert!(!rule.requires_admin, "{id}");
        assert!(rule.constraints.min_age_days >= 1, "{id}");
        assert!(
            !rule.root.relative().is_empty(),
            "{id}: never a whole location"
        );
        let text = rule.description.to_lowercase();
        assert!(text.contains("afterwards"), "{id}: says what happens next");
        assert!(
            text.contains("again"),
            "{id}: says what is downloaded again"
        );
        assert!(
            text.contains("followed") || text.contains("not touched"),
            "{id}: says whether relocated caches are followed"
        );
    }
}

#[test]
fn developer_rules_never_list_credentials_configuration_or_installed_programs() {
    // What each tool keeps in or beside its cache at the default location. `../` climbs out of
    // the rule's root to where the tool keeps its own files.
    let kept: [(&str, &[&str]); 10] = [
        (
            "npm-cache",
            &[
                "../.npmrc",
                "../npm/npm.cmd",
                "../npm-cache/_logs/debug.log",
            ],
        ),
        (
            "pnpm-store",
            &["../global/5/package.json", "../pnpm.exe", "../rc"],
        ),
        ("pnpm-metadata-cache", &["../pnpm/store/v10/files/ab/cd"]),
        ("pip-cache", &["../pip.ini", "../../pip/pip.ini"]),
        (
            "cargo-cache",
            &[
                "credentials.toml",
                "credentials",
                "config.toml",
                "bin/cargo.exe",
                "bin/rustfmt.exe",
                ".crates.toml",
                ".crates2.json",
                "registry/index/index.crates.io-1949cf8c6b5b557f/config.json",
                "git/db/repo-abc123/objects/pack/p1",
            ],
        ),
        (
            "nuget-http-cache",
            &["../plugins-cache/p1", "../NuGet.Config"],
        ),
        ("nuget-packages", &["../NuGet.Config", "../../nuget.config"]),
        (
            "gradle-caches",
            &[
                "../gradle.properties",
                "../wrapper/dists/gradle-8.5-bin/x/bin/gradle.bat",
                "../init.d/a.gradle",
                "../daemon/8.5/daemon.log",
            ],
        ),
        (
            "yarn-cache",
            &[
                "../Berry/global/state.json",
                "../Config/.yarnrc",
                "../../.yarnrc",
            ],
        ),
        (
            "yarn-berry-cache",
            &["../global/state.json", "../../Config/.yarnrc"],
        ),
    ];
    for (id, protected) in kept {
        let world = World::new();
        let rule = world.engine.rules().get(id).expect("rule").clone();
        let root = world.root_of(&rule);
        let tree = dev_fixture(id).expect("fixture");
        let taken: BTreeSet<PathBuf> = tree
            .iter()
            .filter(|(_, expect)| *expect == Taken)
            .map(|(relative, _)| below(&root, relative))
            .collect();
        for path in &taken {
            world.fs.add_file_at(path, 100, ago(30 * DAY));
        }
        let mut guarded = Vec::new();
        for relative in protected {
            let mut path = root.clone();
            for part in relative.split('/') {
                if part == ".." {
                    path.pop();
                } else {
                    path.push(part);
                }
            }
            world.fs.add_file_at(&path, 100, ago(30 * DAY));
            guarded.push(path);
        }
        let plan = world.engine.scan(&[id]).expect("scan");
        let listed: BTreeSet<PathBuf> = plan.rules[0]
            .items
            .iter()
            .map(|item| item.path.clone())
            .collect();
        assert_eq!(listed, taken, "{id}: only cache files are listed");
        for path in &guarded {
            assert!(!listed.contains(path), "{id}: {} is listed", path.display());
        }
        let report = world.engine.clean(&plan.confirm(), Source::Manual);
        assert_eq!(report.skipped_count(), 0, "{id}");
        for path in &guarded {
            assert!(world.fs.exists(path), "{id}: {} was moved", path.display());
        }
    }
}

#[test]
fn a_tool_that_is_not_installed_is_skipped_without_a_failure() {
    let world = World::new();
    let plan = world.engine.scan(&DEV_RULES).expect("scan");
    assert_eq!(plan.rules.len(), DEV_RULES.len());
    for scan in &plan.rules {
        assert_eq!(scan.failure, None, "{}", scan.rule_id);
        assert!(!scan.root_exists, "{}", scan.rule_id);
        assert!(scan.items.is_empty(), "{}", scan.rule_id);
    }
}

// ---- items and folders: AI model caches, relocated tool caches, folders the user adds --------

/// The rules that follow a documented environment variable, with the variables and the folder
/// each adds (checked against the tools' own documentation, named in the rule's `source`).
const FOLLOWED: [(&str, &[(&str, &str)]); 11] = [
    ("npm-cache", &[("npm_config_cache", "_cacache")]),
    ("pnpm-store", &[("PNPM_HOME", "store")]),
    ("pip-cache", &[("PIP_CACHE_DIR", "")]),
    ("cargo-cache", &[("CARGO_HOME", "")]),
    ("nuget-http-cache", &[("NUGET_HTTP_CACHE_PATH", "")]),
    ("nuget-packages", &[("NUGET_PACKAGES", "")]),
    ("gradle-caches", &[("GRADLE_USER_HOME", "caches")]),
    ("yarn-cache", &[("YARN_CACHE_FOLDER", "")]),
    (
        "huggingface-hub-cache",
        &[("HF_HUB_CACHE", ""), ("HF_HOME", "hub")],
    ),
    ("torch-hub-cache", &[("TORCH_HOME", "hub")]),
    ("pnpm-metadata-cache", &[]),
];

#[test]
fn rules_follow_exactly_the_documented_variables_and_name_their_source() {
    let rules = rules();
    for (id, expected) in FOLLOWED {
        let rule = rules.get(id).expect("rule");
        let found: Vec<(&str, String)> = rule
            .follow
            .iter()
            .map(|follow| (follow.var.as_str(), follow.path.join("/")))
            .collect();
        let expected: Vec<(&str, String)> = expected
            .iter()
            .map(|(var, path)| (*var, (*path).to_owned()))
            .collect();
        assert_eq!(found, expected, "{id}");
        for follow in &rule.follow {
            assert!(
                follow.source.len() >= 20,
                "{id}: {} names where the tool documents it",
                follow.var
            );
        }
    }
    // Nothing else follows a variable: a rule that does needs an entry above, with its source.
    for rule in rules.rules() {
        assert_eq!(
            !rule.follow.is_empty(),
            FOLLOWED
                .iter()
                .any(|(id, vars)| *id == rule.id && !vars.is_empty()),
            "{}",
            rule.id
        );
    }
}

#[test]
fn every_followed_cache_is_found_through_its_variable_and_a_bad_value_is_ignored() {
    for (id, vars) in FOLLOWED {
        let Some(&(var, path)) = vars.first() else {
            continue;
        };
        let world = World::new();
        let rule = world.engine.rules().get(id).expect("rule").clone();
        let default_root = world.root_of(&rule);
        world
            .folders
            .set_system_folders(vec![abs("Windows"), abs("Program Files")]);
        // A folder on another drive: where the tool keeps its cache now.
        world.folders.set_env(var, abs("Caches/tool"));
        let mut moved = abs("Caches/tool");
        if !path.is_empty() {
            moved.extend(path.split('/'));
        }
        let plan = world.engine.scan(&[id]).expect("scan");
        assert_eq!(plan.rules[0].root.as_deref(), Some(moved.as_path()), "{id}");
        assert_eq!(plan.rules[0].followed_by.as_deref(), Some(var), "{id}");
        // A value that is a drive root, the profile or a system folder is ignored.
        for bad in [
            abs(""),
            abs("Users/tester"),
            abs("Windows"),
            abs("Program Files/tool"),
        ] {
            world.folders.set_env(var, bad.clone());
            let plan = world.engine.scan(&[id]).expect("scan");
            assert_eq!(plan.rules[0].followed_by, None, "{id}: {bad:?}");
            assert_eq!(
                plan.rules[0].root.as_deref(),
                Some(default_root.as_path()),
                "{id}: {bad:?}"
            );
        }
    }
}

#[test]
fn the_nvidia_per_driver_cache_lives_in_local_low() {
    let rules = rules();
    let rule = rules.get("nvidia-driver-shader-cache").expect("rule");
    assert_eq!(rule.root.location(), Location::LocalLow);
    assert_eq!(rule.root.relative(), ["NVIDIA", "PerDriverVersion"]);
    assert!(!rule.requires_admin);
}

const AI_RULES: [&str; 3] = ["huggingface-hub-cache", "torch-hub-cache", "ollama-models"];

#[test]
fn model_caches_are_picked_one_by_one_and_never_selected_for_the_user() {
    let rules = rules();
    for id in AI_RULES {
        let rule = rules.get(id).expect("rule");
        assert_eq!(
            rule.category,
            volt_core::CleanCategory::AiModelCache,
            "{id}"
        );
        assert_eq!(rule.unit, volt_core::ItemUnit::Entries, "{id}");
        assert!(rule.pick, "{id}");
        assert_eq!(rule.risk, volt_core::Risk::Caution, "{id}");
        assert!(!rule.requires_admin, "{id}");
        let text = rule.description.to_lowercase();
        assert!(text.contains("again"), "{id}: says it is downloaded again");
    }
}

fn model_world() -> (World, PathBuf) {
    let world = World::new();
    let profile = abs("Users/tester");
    (world, profile)
}

#[test]
fn hugging_face_models_are_listed_per_repository_with_their_size_and_one_can_be_taken() {
    let (world, profile) = model_world();
    let hub = profile.join(".cache").join("huggingface").join("hub");
    // Two repositories (one with a symlinked snapshot, as Windows developer mode makes), a lock
    // folder and a file that are not models.
    world.fs.add_file_at(
        hub.join("models--black-forest-labs--FLUX.1-dev/blobs/aaa"),
        6000,
        ago(30 * DAY),
    );
    world.fs.add_link(
        hub.join("models--black-forest-labs--FLUX.1-dev/snapshots/rev1/flux.safetensors"),
    );
    world
        .fs
        .add_file_at(hub.join("datasets--org--data/blobs/bbb"), 300, ago(2 * DAY));
    world
        .fs
        .add_file_at(hub.join(".locks/models--x/lock"), 1, ago(2 * DAY));
    world
        .fs
        .add_file_at(hub.join("version.txt"), 1, ago(2 * DAY));
    // The access token lives beside the hub, in the folder above: out of reach.
    world
        .fs
        .add_file_at(profile.join(".cache/huggingface/token"), 40, ago(2 * DAY));

    let plan = world.engine.scan(&["huggingface-hub-cache"]).expect("scan");
    let scan = &plan.rules[0];
    assert_eq!(scan.failure, None);
    let names: Vec<String> = scan
        .items
        .iter()
        .map(|item| {
            item.path
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "datasets--org--data",
            "models--black-forest-labs--FLUX.1-dev"
        ]
    );
    assert_eq!(scan.items[1].size_bytes, 6000);
    assert!(
        plan.confirm().rules().is_empty(),
        "nothing is taken by default"
    );

    let confirmed = plan
        .confirm_selection(&[volt_core::RuleSelection {
            rule_id: "huggingface-hub-cache".to_owned(),
            items: Some(vec![1]),
        }])
        .expect("confirmed");
    let report = world.engine.clean(&confirmed, Source::Manual);
    assert_eq!(report.skipped_count(), 0, "{report:?}");
    assert_eq!(
        world.fs.trashed(),
        [hub.join("models--black-forest-labs--FLUX.1-dev")],
        "the repository goes in one move"
    );
    assert!(world.fs.exists(&hub.join("datasets--org--data/blobs/bbb")));
    assert!(world.fs.exists(&profile.join(".cache/huggingface/token")));
    assert!(world.fs.exists(&hub.join(".locks/models--x/lock")));
}

#[test]
fn torch_hub_and_ollama_list_their_stores_and_leave_keys_and_settings_alone() {
    let (world, profile) = model_world();
    let torch = profile.join(".cache").join("torch").join("hub");
    world
        .fs
        .add_file_at(torch.join("checkpoints/resnet50.pth"), 900, ago(20 * DAY));
    world.fs.add_file_at(
        torch.join("pytorch_vision_main/hubconf.py"),
        5,
        ago(20 * DAY),
    );
    world
        .fs
        .add_file_at(torch.join("trusted_list"), 5, ago(20 * DAY));
    let ollama = profile.join(".ollama");
    world
        .fs
        .add_file_at(ollama.join("models/blobs/sha256-abc"), 4000, ago(20 * DAY));
    world.fs.add_file_at(
        ollama.join("models/manifests/registry/library/llama/latest"),
        2,
        ago(20 * DAY),
    );
    world
        .fs
        .add_file_at(ollama.join("id_ed25519"), 2, ago(20 * DAY));
    world
        .fs
        .add_file_at(ollama.join("history"), 2, ago(20 * DAY));

    let plan = world
        .engine
        .scan(&["torch-hub-cache", "ollama-models"])
        .expect("scan");
    let listed = |index: usize| -> Vec<String> {
        plan.rules[index]
            .items
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .expect("name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    };
    assert_eq!(
        listed(0),
        ["checkpoints", "pytorch_vision_main"],
        "trusted_list stays"
    );
    assert_eq!(
        listed(1),
        ["models"],
        "one item, not the keys or the history"
    );
    assert_eq!(plan.rules[1].items[0].size_bytes, 4002);
    assert_eq!(plan.rules[1].items[0].file_count, 2);
}

#[test]
fn comfyui_temp_needs_a_folder_the_user_adds_and_never_offers_output() {
    let world = World::new();
    let before = world.engine.scan(&["comfyui-temp"]).expect("scan");
    assert_eq!(
        before.rules[0].failure,
        Some(ScanFailure::Root(
            volt_core::clean::RootError::FolderNotChosen
        ))
    );
    let rule = world
        .engine
        .rules()
        .get("comfyui-temp")
        .expect("rule")
        .clone();
    assert!(rule.needs_chosen_folder());

    let comfy = abs("AI/ComfyUI");
    world.fs.add_dir(&comfy, Some(ago(400 * DAY)));
    world.fs.add_file_at(
        comfy.join("temp/ComfyUI_temp_abc_00001_.png"),
        50,
        ago(5 * DAY),
    );
    world
        .fs
        .add_file_at(comfy.join("temp/fresh.png"), 50, ago(0));
    world
        .fs
        .add_file_at(comfy.join("temp/output/mine.png"), 50, ago(5 * DAY));
    world
        .fs
        .add_file_at(comfy.join("output/ComfyUI_00001_.png"), 50, ago(5 * DAY));
    world.fs.add_file_at(
        comfy.join("models/checkpoints/m.safetensors"),
        50,
        ago(5 * DAY),
    );
    world
        .fs
        .add_file_at(comfy.join("custom_nodes/x/__init__.py"), 5, ago(5 * DAY));
    // Without the files of an install the folder is refused, naming what is missing.
    let refused = world
        .engine
        .choose_folder("comfyui-temp", &comfy)
        .expect_err("not an install yet");
    assert!(
        matches!(&refused, volt_core::clean::FolderError::Refused(
            volt_core::clean::RootError::NotInstall(missing)) if missing.contains(&"main.py".to_owned())),
        "{refused:?}"
    );
    for file in ["main.py", "nodes.py"] {
        world.fs.add_file_at(comfy.join(file), 5, ago(5 * DAY));
    }
    world
        .fs
        .add_file_at(comfy.join("comfy/model_base.py"), 5, ago(5 * DAY));
    world
        .engine
        .choose_folder("comfyui-temp", &comfy)
        .expect("chosen");

    let plan = world.engine.scan(&["comfyui-temp"]).expect("scan");
    let scan = &plan.rules[0];
    assert_eq!(scan.failure, None);
    assert_eq!(
        scan.items
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>(),
        [comfy.join("temp/ComfyUI_temp_abc_00001_.png")]
    );
    let report = world.engine.clean(&plan.confirm(), Source::Manual);
    assert_eq!(report.moved_files(), 1);
    for kept in [
        "output/ComfyUI_00001_.png",
        "temp/output/mine.png",
        "temp/fresh.png",
        "models/checkpoints/m.safetensors",
        "custom_nodes/x/__init__.py",
    ] {
        assert!(world.fs.exists(&comfy.join(kept)), "{kept} must stay");
    }

    // The output folder itself cannot be added.
    world.fs.add_dir(comfy.join("output"), None);
    assert!(
        world
            .engine
            .choose_folder("comfyui-temp", &comfy.join("output"))
            .is_err()
    );
}
