//! A few of the shipped cleaning rules (`catalogue/cleaning.toml`) against the real file system
//! adapter, over a temp directory.
//!
//! The Windows known folders are replaced by a test double that points the rule locations at
//! folders inside a throwaway temp directory, so the real rules run through `WinFileSystem` and
//! the clean engine without any real user data being read or changed. These tests only scan: a
//! clean would put files into the real Recycle Bin, which tests do not do by default.
//!
//! `a_read_only_scan_of_every_rule_on_this_machine` is opt-in (`--ignored`): it resolves the
//! rules against the real folders of the machine running it and prints counts and sizes per rule,
//! never names. It is read-only.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use volt_core::{
    CleanEngine, CleaningRules, KnownFolders, Location, PortError, ScanFailure, SystemClock,
};
use volt_win::{WinFileSystem, WinKnownFolders};

const RULES: &str = include_str!("../../../catalogue/cleaning.toml");

/// Locations that live inside one temp directory. Nothing outside it is ever resolved.
struct TempFolders {
    base: PathBuf,
}

impl TempFolders {
    fn new(base: &Path) -> Self {
        Self {
            base: base.to_path_buf(),
        }
    }

    fn folder(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }
}

impl KnownFolders for TempFolders {
    fn location(&self, location: Location) -> Result<PathBuf, PortError> {
        Ok(match location {
            Location::UserTemp => self.folder("Temp"),
            Location::LocalAppData => self.folder("Local"),
            Location::RoamingAppData => self.folder("Roaming"),
            Location::ProgramData => self.folder("ProgramData"),
            Location::Windows => self.folder("Windows"),
            other => return Err(PortError::Unsupported(format!("location {other}"))),
        })
    }

    fn protected_folders(&self) -> Result<Vec<PathBuf>, PortError> {
        // Nothing here may be a root or contain one; the engine's own checks still apply.
        Ok(vec![self.folder("Profile"), self.folder("Documents")])
    }
}

fn days_ago(days: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(days * 86_400)
}

fn write_file(path: &Path, modified: SystemTime) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("create parent");
    fs::write(path, b"0123456789").expect("write file");
    let file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open file");
    file.set_modified(modified).expect("set file time");
}

/// Sets the modification time of a folder (opened for attribute writes, as Windows requires).
fn set_folder_time(path: &Path, modified: SystemTime) {
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let folder: File = OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .expect("open folder");
    folder.set_modified(modified).expect("set folder time");
}

fn relative_to(base: &Path, paths: impl IntoIterator<Item = PathBuf>) -> BTreeSet<String> {
    paths
        .into_iter()
        .map(|path| {
            path.strip_prefix(base)
                .expect("inside the temp directory")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn real_rules_scan_a_real_directory_tree_and_change_nothing() {
    let dir = TempDir::new().expect("temp dir");
    // `canonicalize` resolves any short (8.3) name of the temp directory.
    let base = dir.path().canonicalize().expect("canonical path");
    let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
    let old = days_ago(30);
    let fresh = days_ago(0);

    // user-temp: old files and an emptied folder are taken, new ones and a new folder are not.
    write_file(&base.join("Temp/old.tmp"), old);
    write_file(&base.join("Temp/fresh.tmp"), fresh);
    write_file(&base.join("Temp/installer/setup.log"), old);
    fs::create_dir_all(base.join("Temp/empty-old")).expect("folder");
    fs::create_dir_all(base.join("Temp/empty-new")).expect("folder");
    // user-crash-dumps
    write_file(&base.join("Local/CrashDumps/app.1234.dmp"), old);
    write_file(&base.join("Local/CrashDumps/app.5678.dmp"), fresh);
    write_file(&base.join("Local/CrashDumps/notes.txt"), old);
    // thumbnail-cache and icon-cache share one folder and take no age into account.
    let explorer = base.join("Local/Microsoft/Windows/Explorer");
    write_file(&explorer.join("thumbcache_256.db"), fresh);
    write_file(&explorer.join("iconcache_48.db"), fresh);
    write_file(&explorer.join("RecommendationsFilterList.json"), old);
    // Folder times last: adding files changes them.
    set_folder_time(&base.join("Temp/installer"), old);
    set_folder_time(&base.join("Temp/empty-old"), old);

    let engine = CleanEngine::new(
        CleaningRules::from_toml(RULES).expect("the shipped rules"),
        Arc::new(TempFolders::new(&base)),
        Arc::new(WinFileSystem::new()),
        Arc::new(SystemClock),
    );
    let plan = engine
        .scan(&[
            "user-temp",
            "user-crash-dumps",
            "thumbnail-cache",
            "icon-cache",
        ])
        .expect("known rules");
    for scan in &plan.rules {
        assert_eq!(scan.failure, None, "{}", scan.rule_id);
        assert!(scan.root_exists, "{}", scan.rule_id);
        assert!(!scan.needs_administrator(), "{}", scan.rule_id);
    }
    let listed = |index: usize| {
        relative_to(
            &base,
            plan.rules[index].items.iter().map(|item| item.path.clone()),
        )
    };

    assert_eq!(
        listed(0),
        BTreeSet::from([
            "Temp/old.tmp".to_owned(),
            "Temp/installer/setup.log".to_owned(),
            "Temp/installer".to_owned(),
            "Temp/empty-old".to_owned(),
        ]),
        "user-temp"
    );
    assert_eq!(plan.rules[0].total_bytes(), 20);
    assert_eq!(plan.rules[0].min_age_days, 7);
    assert_eq!(
        listed(1),
        BTreeSet::from(["Local/CrashDumps/app.1234.dmp".to_owned()]),
        "user-crash-dumps"
    );
    assert_eq!(
        listed(2),
        BTreeSet::from(["Local/Microsoft/Windows/Explorer/thumbcache_256.db".to_owned()]),
        "thumbnail-cache"
    );
    assert_eq!(
        listed(3),
        BTreeSet::from(["Local/Microsoft/Windows/Explorer/iconcache_48.db".to_owned()]),
        "icon-cache"
    );

    // The scan is read-only: every entry is still there.
    for kept in [
        "Temp/old.tmp",
        "Temp/fresh.tmp",
        "Temp/installer/setup.log",
        "Temp/empty-old",
        "Temp/empty-new",
        "Local/CrashDumps/app.1234.dmp",
        "Local/CrashDumps/app.5678.dmp",
        "Local/CrashDumps/notes.txt",
        "Local/Microsoft/Windows/Explorer/thumbcache_256.db",
        "Local/Microsoft/Windows/Explorer/iconcache_48.db",
    ] {
        assert!(base.join(kept).exists(), "{kept} must still exist");
    }
}

#[test]
fn a_machine_wide_rule_over_a_missing_location_is_nothing_to_clean() {
    let dir = TempDir::new().expect("temp dir");
    let base = dir.path().canonicalize().expect("canonical path");
    let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
    let engine = CleanEngine::new(
        CleaningRules::from_toml(RULES).expect("the shipped rules"),
        Arc::new(TempFolders::new(&base)),
        Arc::new(WinFileSystem::new()),
        Arc::new(SystemClock),
    );
    let plan = engine.scan_all();
    assert!(plan.is_empty());
    for scan in &plan.rules {
        assert_eq!(scan.failure, None, "{}", scan.rule_id);
        assert!(!scan.root_exists, "{}", scan.rule_id);
        assert!(!scan.needs_administrator(), "{}", scan.rule_id);
    }
}

/// Read-only, opt in with
/// `cargo test -p volt-win --test cleaning_rules -- --ignored --nocapture`.
///
/// Scans every shipped rule on the machine running the test and prints, per rule, the counts and
/// the total size, and whether the scan was held back for want of administrator rights. It prints
/// no file or folder name and moves nothing.
#[test]
#[ignore = "reads the real folders of this machine (read-only)"]
fn a_read_only_scan_of_every_rule_on_this_machine() {
    let engine = CleanEngine::new(
        CleaningRules::from_toml(RULES).expect("the shipped rules"),
        Arc::new(WinKnownFolders::new()),
        Arc::new(WinFileSystem::new()),
        Arc::new(SystemClock),
    );
    let plan = engine.scan_all();
    for scan in &plan.rules {
        let failure = scan
            .failure
            .as_ref()
            .map_or_else(|| "none".to_owned(), ToString::to_string);
        eprintln!(
            "{:<28} admin-rule={:<5} root_exists={:<5} needs_admin={:<5} files={:<6} folders={:<4} \
             bytes={:<12} too_new={:<5} denied={:<4} unreadable={:<4} failure={}",
            scan.rule_id,
            scan.requires_admin,
            scan.root_exists,
            scan.needs_administrator(),
            scan.file_count(),
            scan.directory_count(),
            scan.total_bytes(),
            scan.counts.too_new,
            scan.counts.access_denied,
            scan.counts.unreadable,
            failure,
        );
        // Whatever the machine looks like, a refusal is typed: never an untyped failure for a
        // rule that needs administrator rights.
        if scan.requires_admin {
            assert!(
                !matches!(scan.failure, Some(ScanFailure::RootUnreadable(_))),
                "{}: an unreadable root must be classified: {:?}",
                scan.rule_id,
                scan.failure
            );
        }
    }
}
