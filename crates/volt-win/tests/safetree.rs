//! The handle-based tree walker (`volt_win::safetree`) against real folders.
//!
//! Every test builds its own tree inside a throwaway temp directory and walks only that. Nothing
//! outside it is ever read or changed, and no test needs administrator rights: the walk uses the
//! same calls the elevated helper uses, and the elevated CI job repeats the cases that need a
//! second volume or an access list only an administrator can make.
//!
//! The first group is what scan and apply do on an ordinary tree. The `races` module is the race
//! harness of the decision: each case swaps something between the scan and the delete, or exactly
//! at the point where an attacker would, and states what it proved: a delete never lands outside
//! the root, and never on anything but the scanned object.

#![cfg(windows)]

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use tempfile::TempDir;
use volt_core::clean::machine::MACHINE_MAX_ENTRIES;
use volt_core::{CleaningRule, CleaningRules, Timestamp};
use volt_win::safetree::{
    self, Control, Limits, NoProbe, Policy, Probe, Stop, TreeError, TreeScan,
};

const RULE: &str = r#"
[[rule]]
id = "walk"
title = "Walk"
description = "A rule for the walker tests."
category = "temporary_files"
risk = "caution"
include = ["**"]
exclude = ["CBS"]
requires_admin = true
permanent = true

[rule.root]
location = "windows"
path = "Temp"

[rule.constraints]
min_age_days = 7
max_depth = 6
remove_empty_dirs = true
"#;

fn rule() -> CleaningRule {
    CleaningRules::from_toml(RULE)
        .expect("rules")
        .get("walk")
        .cloned()
        .expect("rule")
}

fn now() -> Timestamp {
    Timestamp::from_unix_millis(
        i64::try_from(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("fits"),
    )
}

fn policy() -> Policy {
    Policy {
        rule: rule(),
        min_age_days: 7,
        now: now(),
        skip_user_writable: false,
        pending_restart: Vec::new(),
    }
}

/// A file with `size` bytes whose last write was `days` ago.
fn file_aged(path: &Path, days: u64, size: usize) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    let mut file = File::create(path).expect("create");
    file.write_all(&vec![b'x'; size]).expect("write");
    file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .expect("set the time");
}

/// A folder whose last write was `days` ago (set after its content is made).
fn dir_aged(path: &Path, days: u64) {
    fs::create_dir_all(path).expect("dir");
    let handle = OpenOptions::new()
        .write(true)
        // FILE_FLAG_BACKUP_SEMANTICS: a folder can be opened as a handle.
        .custom_flags(0x0200_0000)
        .open(path)
        .expect("open the folder");
    handle
        .set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
        .expect("set the time");
}

fn scan_of(root: &Path, policy: &Policy) -> Result<TreeScan, TreeError> {
    safetree::scan(root, policy, &Limits::default(), &Control::unbounded())
}

fn apply_of(root: &Path, scan: &TreeScan, policy: &Policy) -> safetree::ApplyResult {
    safetree::apply(root, scan, policy, &Control::unbounded(), &NoProbe).expect("apply")
}

fn junction(link: &Path, target: &Path) {
    let output = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("run mklink");
    assert!(
        output.status.success(),
        "mklink failed: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

fn temp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp");
    // The real path (no 8.3 short names), as a rule's root would be.
    let root = dir.path().canonicalize().expect("canonical");
    let root = PathBuf::from(root.to_string_lossy().trim_start_matches(r"\\?\"));
    (dir, root.join("tree"))
}

// ---------------------------------------------------------------------------------------------

#[test]
fn a_scan_counts_what_a_clean_would_delete_and_apply_deletes_exactly_that() {
    let (_guard, root) = temp();
    file_aged(&root.join("old1.tmp"), 30, 100);
    file_aged(&root.join("new.tmp"), 0, 50);
    file_aged(&root.join("sub").join("old2.tmp"), 30, 200);
    file_aged(&root.join("sub").join("deep").join("old3.tmp"), 30, 300);
    dir_aged(&root.join("empty"), 30);
    file_aged(&root.join("keep").join("recent.tmp"), 1, 10);
    // Folders are aged last: making a file in one moves its time.
    dir_aged(&root.join("sub").join("deep"), 30);
    dir_aged(&root.join("sub"), 30);
    dir_aged(&root.join("keep"), 30);

    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");
    assert_eq!(scan.files(), 3);
    assert_eq!(scan.bytes(), 600);
    // `deep` and `sub` are emptied by the files, and `empty` already is; `keep` holds a new file.
    assert_eq!(scan.folders(), 3);
    assert_eq!(scan.skips().too_new, 2, "new.tmp and keep/recent.tmp");
    assert!(!scan.truncated());
    assert_eq!(scan.stopped(), None);
    // A scan changes nothing.
    assert!(root.join("old1.tmp").exists());

    let done = apply_of(&root, &scan, &policy);
    assert_eq!((done.files, done.folders, done.bytes), (3, 3, 600));
    assert_eq!(done.skipped.total(), 0, "{:?}", done.skipped);
    for gone in ["old1.tmp", "sub", "empty"] {
        assert!(!root.join(gone).exists(), "{gone} should be gone");
    }
    for kept in ["new.tmp", "keep"] {
        assert!(root.join(kept).exists(), "{kept} should stay");
    }
    assert!(root.join("keep").join("recent.tmp").exists());
}

#[test]
fn an_excluded_folder_is_left_alone_with_everything_in_it() {
    let (_guard, root) = temp();
    file_aged(&root.join("CBS").join("old.log"), 30, 10);
    file_aged(&root.join("other").join("old.log"), 30, 10);
    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");
    assert_eq!(scan.files(), 1);
    apply_of(&root, &scan, &policy);
    assert!(root.join("CBS").join("old.log").exists());
    assert!(!root.join("other").join("old.log").exists());
}

#[test]
fn a_junction_inside_the_root_is_counted_and_never_followed() {
    let (guard, root) = temp();
    let outside = guard.path().join("outside");
    file_aged(&outside.join("victim.tmp"), 30, 10);
    file_aged(&root.join("mine.tmp"), 30, 10);
    junction(&root.join("link"), &outside);

    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");
    assert_eq!(scan.files(), 1, "only the file that is really in the root");
    assert_eq!(scan.skips().links, 1);
    let done = apply_of(&root, &scan, &policy);
    assert_eq!(done.files, 1);
    assert!(outside.join("victim.tmp").exists());
    assert!(root.join("link").exists(), "the link itself stays");
}

#[test]
fn a_root_that_is_a_junction_or_is_reached_through_one_is_refused() {
    let (guard, root) = temp();
    let real = guard.path().join("real");
    file_aged(&real.join("sub").join("old.tmp"), 30, 10);
    fs::create_dir_all(&root).expect("root");
    junction(&root.join("as_root"), &real);

    let policy = policy();
    assert_eq!(
        scan_of(&root.join("as_root"), &policy).err(),
        Some(TreeError::RootRefused),
        "the root is a junction"
    );
    assert_eq!(
        scan_of(&root.join("as_root").join("sub"), &policy).err(),
        Some(TreeError::RootRefused),
        "a junction is on the way to the root"
    );
    assert!(real.join("sub").join("old.tmp").exists());
}

#[test]
fn a_root_that_does_not_exist_or_is_a_file_is_reported() {
    let (guard, root) = temp();
    let policy = policy();
    assert_eq!(
        scan_of(&root.join("nothing"), &policy).err(),
        Some(TreeError::RootMissing)
    );
    let file = guard.path().join("a-file");
    file_aged(&file, 30, 1);
    assert_eq!(scan_of(&file, &policy).err(), Some(TreeError::RootRefused));
    assert_eq!(
        scan_of(Path::new("relative\\path"), &policy).err(),
        Some(TreeError::RootRefused)
    );
}

#[test]
fn a_rule_that_takes_whole_entries_is_not_served() {
    let text = RULE.replace(
        "requires_admin = true",
        "requires_admin = true\nunit = \"entries\"",
    );
    let entries = CleaningRules::from_toml(&text)
        .expect("rules")
        .get("walk")
        .cloned()
        .expect("rule");
    let (_guard, root) = temp();
    fs::create_dir_all(&root).expect("root");
    let policy = Policy {
        rule: entries,
        ..policy()
    };
    assert_eq!(
        scan_of(&root, &policy).err(),
        Some(TreeError::UnsupportedRule)
    );
}

#[test]
fn the_caps_on_items_entries_and_depth_truncate_the_scan() {
    let (_guard, root) = temp();
    for index in 0..10 {
        file_aged(&root.join(format!("old{index}.tmp")), 30, 1);
    }
    let policy = policy();
    let few = Limits {
        max_items: 4,
        ..Limits::default()
    };
    let scan = safetree::scan(&root, &policy, &few, &Control::unbounded()).expect("scan");
    assert_eq!(scan.files(), 4);
    assert!(scan.truncated());

    let narrow = Limits {
        max_entries: 5,
        ..Limits::default()
    };
    let scan = safetree::scan(&root, &policy, &narrow, &Control::unbounded()).expect("scan");
    assert!(scan.truncated());
    assert!(scan.files() <= 5);
    assert_eq!(MACHINE_MAX_ENTRIES, Limits::default().max_entries);

    // A level deeper than the rule's own depth is not looked into, and says so.
    file_aged(
        &root
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("e")
            .join("f")
            .join("g")
            .join("deep.tmp"),
        30,
        1,
    );
    let shallow = Limits {
        max_depth: 2,
        ..Limits::default()
    };
    let scan = safetree::scan(&root, &policy, &shallow, &Control::unbounded()).expect("scan");
    assert!(scan.skips().too_deep >= 1);
    assert_eq!(scan.files(), 10, "nothing below depth 2 was planned");
}

#[test]
fn a_walk_that_is_cancelled_or_out_of_time_stops_and_says_why() {
    let (_guard, root) = temp();
    for index in 0..20 {
        file_aged(&root.join(format!("old{index}.tmp")), 30, 1);
    }
    let policy = policy();
    let control = Control::unbounded();
    control.cancel_flag().store(true, Ordering::SeqCst);
    let scan = safetree::scan(&root, &policy, &Limits::default(), &control).expect("scan");
    assert_eq!(scan.stopped(), Some(Stop::Cancelled));
    assert_eq!(scan.files(), 0);

    let late = Control::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Duration::ZERO,
    );
    let scan = safetree::scan(&root, &policy, &Limits::default(), &late).expect("scan");
    assert_eq!(scan.stopped(), Some(Stop::TimedOut));

    // An apply that is stopped deletes nothing of what it had not reached.
    let full = scan_of(&root, &policy).expect("scan");
    let done = safetree::apply(&root, &full, &policy, &late, &NoProbe).expect("apply");
    assert_eq!(done.stopped, Some(Stop::TimedOut));
    assert_eq!(done.files, 0);
    assert!(root.join("old0.tmp").exists());
}

#[test]
fn a_file_that_changed_after_the_scan_is_left_in_place() {
    let (_guard, root) = temp();
    file_aged(&root.join("grew.tmp"), 30, 10);
    file_aged(&root.join("touched.tmp"), 30, 10);
    file_aged(&root.join("same.tmp"), 30, 10);
    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");

    let mut grown = OpenOptions::new()
        .append(true)
        .open(root.join("grew.tmp"))
        .expect("open");
    grown.write_all(b"more").expect("append");
    drop(grown);
    File::options()
        .write(true)
        .open(root.join("touched.tmp"))
        .expect("open")
        .set_modified(SystemTime::now())
        .expect("touch");

    let done = apply_of(&root, &scan, &policy);
    assert_eq!(done.files, 1, "only same.tmp");
    assert_eq!(done.skipped.changed, 2);
    assert!(root.join("grew.tmp").exists() && root.join("touched.tmp").exists());
    assert!(!root.join("same.tmp").exists());
}

#[test]
fn a_file_another_program_holds_open_is_skipped_and_a_read_only_file_is_deleted() {
    let (_guard, root) = temp();
    file_aged(&root.join("held.tmp"), 30, 10);
    file_aged(&root.join("readonly.tmp"), 30, 10);
    let mut permissions = fs::metadata(root.join("readonly.tmp"))
        .expect("metadata")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(root.join("readonly.tmp"), permissions).expect("read-only");
    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");

    // Open without sharing for delete: no one else may delete or rename it while this is held.
    let _held = OpenOptions::new()
        .read(true)
        .share_mode(1 | 2)
        .open(root.join("held.tmp"))
        .expect("hold it");
    let done = apply_of(&root, &scan, &policy);
    assert_eq!(done.files, 1);
    assert_eq!(done.skipped.locked, 1);
    assert!(root.join("held.tmp").exists());
    assert!(!root.join("readonly.tmp").exists());
}

#[test]
fn a_silent_rule_skips_a_folder_a_non_administrator_can_write_to() {
    let (_guard, root) = temp();
    file_aged(&root.join("sub").join("old.tmp"), 30, 10);
    dir_aged(&root.join("sub"), 30);
    // The temp directory belongs to the person running the test, who is not SYSTEM or an
    // administrator group, so what is in it is "writable by a non-administrator".
    let silent = Policy {
        skip_user_writable: true,
        ..policy()
    };
    let scan = scan_of(&root, &silent).expect("scan");
    assert_eq!(scan.files(), 0);
    assert_eq!(scan.skips().user_writable, 1);
    assert!(
        scan.root_writable_by_users(),
        "so a clean would be confirmed"
    );

    // The same tree through a rule a person confirms: nothing is skipped for it.
    let confirmed = scan_of(&root, &policy()).expect("scan");
    assert_eq!(confirmed.files(), 1);
    assert!(root.join("sub").join("old.tmp").exists());
}

#[test]
fn a_file_windows_has_queued_for_the_next_restart_is_left_for_windows() {
    let (_guard, root) = temp();
    file_aged(&root.join("queued.tmp"), 30, 10);
    file_aged(&root.join("free.tmp"), 30, 10);
    file_aged(&root.join("later.tmp"), 30, 10);
    file_aged(&root.join("dir").join("inner.tmp"), 30, 10);
    dir_aged(&root.join("dir"), 30);
    let base = root.to_string_lossy().to_lowercase();
    let queue = |names: &[&str]| -> Policy {
        Policy {
            pending_restart: names.iter().map(|name| format!("{base}\\{name}")).collect(),
            ..policy()
        }
    };

    // Queued when the scan is made: the file, and the folder that holds a queued file, stay.
    let at_scan = queue(&["queued.tmp", "dir\\inner.tmp"]);
    let scan = scan_of(&root, &at_scan).expect("scan");
    assert_eq!(
        (scan.files(), scan.folders()),
        (2, 0),
        "free.tmp and later.tmp"
    );
    assert_eq!(scan.skips().pending_restart, 2);

    // Queued after the scan, before the apply: left for Windows too.
    let at_apply = queue(&["queued.tmp", "dir\\inner.tmp", "later.tmp"]);
    let done = apply_of(&root, &scan, &at_apply);
    assert_eq!(done.files, 1);
    assert_eq!(done.skipped.pending_restart, 1);
    assert!(!root.join("free.tmp").exists());
    for kept in ["queued.tmp", "later.tmp", "dir"] {
        assert!(root.join(kept).exists(), "{kept}");
    }
}

#[test]
fn an_apply_for_a_different_folder_than_the_scan_is_refused() {
    let (guard, root) = temp();
    file_aged(&root.join("old.tmp"), 30, 10);
    let other = guard.path().join("other");
    file_aged(&other.join("old.tmp"), 30, 10);
    let policy = policy();
    let scan = scan_of(&root, &policy).expect("scan");
    let refused = safetree::apply(&other, &scan, &policy, &Control::unbounded(), &NoProbe);
    assert_eq!(refused.err(), Some(TreeError::RootRefused));
    assert!(other.join("old.tmp").exists());
}

/// Probe-free helper shared by the race cases.
mod races {
    use std::sync::Mutex;

    use super::*;

    /// A probe that runs a closure at the two points an attacker would use.
    struct Hook<B, D> {
        before_open: B,
        before_delete: D,
    }

    impl<B, D> Probe for Hook<B, D>
    where
        B: Fn(&str) + Send + Sync,
        D: Fn(&str) + Send + Sync,
    {
        fn enabled(&self) -> bool {
            true
        }

        fn before_open(&self, item: &str) {
            (self.before_open)(item);
        }

        fn before_delete(&self, item: &str) {
            (self.before_delete)(item);
        }
    }

    fn hook<B: Fn(&str) + Send + Sync, D: Fn(&str) + Send + Sync>(
        before_open: B,
        before_delete: D,
    ) -> Hook<B, D> {
        Hook {
            before_open,
            before_delete,
        }
    }

    fn nothing(_: &str) {}

    /// The outside folder the swaps point at: it holds a file with the same name as the scanned one.
    fn outside(guard: &TempDir) -> PathBuf {
        let outside = guard.path().join("outside");
        file_aged(&outside.join("victim.tmp"), 30, 10);
        file_aged(&outside.join("sub").join("victim.tmp"), 30, 10);
        outside
    }

    fn untouched(outside: &Path) {
        for name in ["victim.tmp", "sub\\victim.tmp"] {
            let path = outside.join(name);
            assert!(path.exists(), "{} must still exist", path.display());
            assert_eq!(fs::metadata(&path).expect("metadata").len(), 10);
        }
    }

    fn run(root: &Path, scan: &TreeScan, probe: &dyn Probe) -> safetree::ApplyResult {
        safetree::apply(root, scan, &policy(), &Control::unbounded(), probe).expect("apply")
    }

    /// Junction swap, between scan and apply: the folder the scan went through is replaced by a
    /// junction to a folder outside the root that holds a file of the same name.
    #[test]
    fn a_folder_swapped_for_a_junction_after_the_scan_is_not_entered() {
        let (guard, root) = temp();
        let outside = outside(&guard);
        file_aged(&root.join("sub").join("victim.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        fs::remove_dir_all(root.join("sub")).expect("remove");
        junction(&root.join("sub"), &outside);

        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.links, 1, "{:?}", done.skipped);
        untouched(&outside);
    }

    /// Junction swap at the worst moment: the scanned FILE is replaced by a junction to the
    /// outside folder after its parent is held and before it is opened.
    #[test]
    fn a_file_swapped_for_a_junction_right_before_it_is_opened_is_not_followed() {
        let (guard, root) = temp();
        let outside = outside(&guard);
        file_aged(&root.join("victim.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        let target = root.join("victim.tmp");
        let swapped = hook(
            |item| {
                if item == "victim.tmp" {
                    fs::remove_file(root.join("victim.tmp")).expect("remove");
                    junction(&root.join("victim.tmp"), &outside);
                }
            },
            nothing,
        );
        let done = run(&root, &scan, &swapped);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.total(), 1, "{:?}", done.skipped);
        assert!(target.exists(), "the junction itself is not ours to delete");
        untouched(&outside);
    }

    /// Mount point: a volume mount point is the same kind of reparse point as a junction (the same
    /// tag, `IO_REPARSE_TAG_MOUNT_POINT`), which the walker refuses by its attribute. The elevated CI
    /// job mounts a real second volume to prove it end to end; this unelevated case proves the
    /// check on the tag every mount point carries.
    #[test]
    fn a_mount_point_inside_the_root_is_a_link_and_is_never_entered() {
        let (guard, root) = temp();
        let outside = outside(&guard);
        fs::create_dir_all(&root).expect("root");
        junction(&root.join("mounted"), &outside);
        file_aged(&root.join("mine.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");
        assert_eq!(scan.skips().links, 1);
        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.files, 1);
        untouched(&outside);
    }

    /// Hard link: a scanned file that gets a second name after the scan (the second name may be
    /// anywhere, in a folder the attacker owns) is not deleted, so a name that points into the
    /// root can never be used to reach, or to be mistaken for, something outside it.
    #[test]
    fn a_file_with_a_second_name_is_left_alone() {
        let (guard, root) = temp();
        let outside = guard.path().join("outside");
        fs::create_dir_all(&outside).expect("outside");
        file_aged(&root.join("linked.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        fs::hard_link(root.join("linked.tmp"), outside.join("alias.tmp")).expect("hard link");
        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.hard_links, 1, "{:?}", done.skipped);
        assert!(root.join("linked.tmp").exists() && outside.join("alias.tmp").exists());
    }

    /// Hard link, other direction: the scanned file is replaced by a hard link to a file outside
    /// the root. It is not the object the scan saw, so it is left alone and the outside file keeps
    /// every name.
    #[test]
    fn a_file_replaced_by_a_hard_link_to_an_outside_file_is_not_deleted() {
        let (guard, root) = temp();
        let outside = outside(&guard);
        file_aged(&root.join("victim.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        let swapped = hook(
            |item| {
                if item == "victim.tmp" {
                    fs::remove_file(root.join("victim.tmp")).expect("remove");
                    fs::hard_link(outside.join("victim.tmp"), root.join("victim.tmp"))
                        .expect("hard link");
                }
            },
            nothing,
        );
        let done = run(&root, &scan, &swapped);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.total(), 1, "{:?}", done.skipped);
        untouched(&outside);
        assert!(root.join("victim.tmp").exists());
    }

    /// Directory-to-file swap: a scanned folder is replaced by a FILE of the same name. The file
    /// is not what was planned and is left alone.
    #[test]
    fn a_folder_swapped_for_a_file_is_not_deleted() {
        let (_guard, root) = temp();
        dir_aged(&root.join("empty"), 30);
        let scan = scan_of(&root, &policy()).expect("scan");
        assert_eq!(scan.folders(), 1);

        fs::remove_dir(root.join("empty")).expect("remove");
        file_aged(&root.join("empty"), 30, 10);
        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.folders, 0);
        assert_eq!(done.skipped.changed, 1, "{:?}", done.skipped);
        assert!(root.join("empty").is_file());
    }

    /// File-to-directory swap: a scanned file is replaced by a FOLDER of the same name holding a
    /// file. Nothing in the new folder is touched.
    #[test]
    fn a_file_swapped_for_a_folder_is_not_deleted_and_nothing_inside_it_is_touched() {
        let (_guard, root) = temp();
        file_aged(&root.join("thing.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        fs::remove_file(root.join("thing.tmp")).expect("remove");
        file_aged(&root.join("thing.tmp").join("inner.tmp"), 30, 10);
        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.changed, 1, "{:?}", done.skipped);
        assert!(root.join("thing.tmp").join("inner.tmp").exists());
    }

    /// Ancestor rename, between scan and apply: the folder the scan went through is renamed away
    /// and another folder with the same name and a same-named file is put in its place. The new one
    /// is a different object, so the planned file in it is not deleted; the renamed one is not
    /// reached by name either.
    #[test]
    fn a_folder_renamed_away_and_replaced_is_not_taken_for_the_scanned_one() {
        let (_guard, root) = temp();
        file_aged(&root.join("a").join("x.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        fs::rename(root.join("a"), root.join("a-moved")).expect("rename");
        file_aged(&root.join("a").join("x.tmp"), 30, 10);
        let done = run(&root, &scan, &NoProbe);
        assert_eq!(done.files, 0);
        assert_eq!(done.skipped.changed, 1, "{:?}", done.skipped);
        assert!(root.join("a").join("x.tmp").exists());
        assert!(root.join("a-moved").join("x.tmp").exists());
    }

    /// Ancestor rename DURING the apply: while a folder on the way is held, neither it, nor the
    /// root, nor the folder above the root can be renamed, so the path the handle was opened
    /// through cannot be redirected mid-walk.
    #[test]
    fn no_folder_on_the_way_can_be_renamed_while_the_walk_holds_it() {
        let (guard, root) = temp();
        file_aged(&root.join("a").join("b").join("x.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        let attempts: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());
        let held = hook(
            |item| {
                if item != "a/b/x.tmp" {
                    return;
                }
                let base = guard.path();
                for (name, from, to) in [
                    ("parent folder", root.join("a"), root.join("a-renamed")),
                    (
                        "grandparent",
                        root.join("a").join("b"),
                        root.join("a").join("b2"),
                    ),
                    ("root", root.clone(), base.join("tree-renamed")),
                    (
                        "above the root",
                        base.to_path_buf(),
                        base.with_file_name("renamed-above"),
                    ),
                ] {
                    let renamed = fs::rename(&from, &to).is_ok();
                    attempts
                        .lock()
                        .expect("lock")
                        .push((name.to_owned(), renamed));
                    if renamed {
                        // Put it back so the rest of the case still runs on the same tree.
                        fs::rename(&to, &from).expect("undo");
                    }
                }
            },
            nothing,
        );
        let done = run(&root, &scan, &held);
        let attempts = attempts.lock().expect("lock").clone();
        assert_eq!(attempts.len(), 4, "the hook ran: {attempts:?}");
        for (name, renamed) in &attempts {
            assert!(!renamed, "the {name} was renamed while the walk held it");
        }
        assert_eq!(done.files, 1, "{:?}", done.skipped);
        assert!(!root.join("a").join("b").join("x.tmp").exists());
    }

    /// The delete goes through the handle that was checked: replacing the name between the check
    /// and the delete is impossible, because that handle blocks other deletes and renames of it.
    #[test]
    fn the_checked_file_cannot_be_renamed_or_replaced_before_it_is_deleted() {
        let (_guard, root) = temp();
        file_aged(&root.join("x.tmp"), 30, 10);
        file_aged(&root.join("decoy.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        let attempts: Mutex<Vec<bool>> = Mutex::new(Vec::new());
        let held = hook(nothing, |item| {
            if item == "x.tmp" {
                let renamed = fs::rename(root.join("x.tmp"), root.join("x-moved.tmp")).is_ok();
                let replaced = fs::rename(root.join("decoy.tmp"), root.join("x.tmp")).is_ok();
                attempts.lock().expect("lock").extend([renamed, replaced]);
            }
        });
        let done = run(&root, &scan, &held);
        assert_eq!(*attempts.lock().expect("lock"), [false, false]);
        assert_eq!(done.files, 2, "{:?}", done.skipped);
        assert!(!root.join("x.tmp").exists());
    }

    /// A hostile root: the root itself is swapped for a junction between the scan and the apply.
    #[test]
    fn a_root_swapped_for_a_junction_after_the_scan_is_refused() {
        let (guard, root) = temp();
        let outside = outside(&guard);
        file_aged(&root.join("victim.tmp"), 30, 10);
        let scan = scan_of(&root, &policy()).expect("scan");

        fs::remove_dir_all(&root).expect("remove");
        junction(&root, &outside);
        let refused = safetree::apply(&root, &scan, &policy(), &Control::unbounded(), &NoProbe);
        assert_eq!(refused.err(), Some(TreeError::RootRefused));
        untouched(&outside);
    }
}
