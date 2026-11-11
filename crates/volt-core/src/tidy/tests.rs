//! Tests of the tidy-up finder against in-memory fakes. Nothing here touches the real file
//! system.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::shortcut::{DRIVE_FIXED, build};
use super::*;
use crate::activity::ActivityOutcome;
use crate::clean::fakes::FakeKnownFolders;
use crate::fakes::{FakeActivityLog, FakeFileSystem, ManualClock};

const NOW: i64 = 1_800_000_000_000;
const LARGE: usize = 100_000;

fn abs(rest: &str) -> PathBuf {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    Path::new(base).join(rest)
}

struct Quiet;
impl TidyObserver for Quiet {
    fn progress(&self, _: &TidyProgress) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct StopAfter(Cell<u32>);
impl TidyObserver for StopAfter {
    fn progress(&self, _: &TidyProgress) {}
    fn is_cancelled(&self) -> bool {
        let left = self.0.get();
        self.0.set(left.saturating_sub(1));
        left == 0
    }
}

struct Fixture {
    fs: Arc<FakeFileSystem>,
    activity: Arc<FakeActivityLog>,
    finder: TidyFinder,
}

fn fixture() -> Fixture {
    let fs = Arc::new(FakeFileSystem::new());
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set_system_folders(vec![
        abs("Windows"),
        abs("Program Files"),
        abs("ProgramData"),
    ]);
    let activity = Arc::new(FakeActivityLog::new());
    let finder = TidyFinder::new(fs.clone(), folders, Arc::new(ManualClock::new(NOW)))
        .with_activity_log(activity.clone());
    Fixture {
        fs,
        activity,
        finder,
    }
}

/// Bytes of `len` that differ by `seed`.
fn data(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|n| seed.wrapping_add(u8::try_from(n % 251).unwrap_or(0)))
        .collect()
}

fn options() -> TidyOptions {
    TidyOptions {
        min_duplicate_bytes: 1,
    }
}

fn scan_of(f: &Fixture, roots: &[&str]) -> TidyScan {
    let roots: Vec<PathBuf> = roots.iter().map(|r| abs(r)).collect();
    f.finder.scan(&roots, &options(), &Quiet).expect("scan")
}

fn group_paths(scan: &TidyScan) -> Vec<Vec<PathBuf>> {
    scan.duplicates
        .iter()
        .map(|g| g.files.iter().map(|f| f.path.clone()).collect())
        .collect()
}

#[test]
fn groups_by_size_then_partial_hash_then_full_hash() {
    let f = fixture();
    let same = data(1, LARGE);
    f.fs.add_file_with_content(abs("d/a.bin"), &same);
    f.fs.add_file_with_content(abs("d/b.bin"), &same);
    // Same size and same first 64 KiB, different at the end: only the full hash tells.
    let mut tail = same.clone();
    tail[LARGE - 1] ^= 0xFF;
    f.fs.add_file_with_content(abs("d/tail.bin"), &tail);
    // Same size, different from the first byte: the partial hash is enough.
    f.fs.add_file_with_content(abs("d/head.bin"), &data(9, LARGE));
    // A size nobody else has.
    f.fs.add_file_with_content(abs("d/alone.bin"), &data(1, LARGE + 1));

    let scan = scan_of(&f, &["d"]);
    assert_eq!(
        group_paths(&scan),
        vec![vec![abs("d/a.bin"), abs("d/b.bin")]]
    );
    assert_eq!(scan.duplicates[0].size_bytes, LARGE as u64);
    assert_eq!(scan.duplicates[0].wasted_bytes(), LARGE as u64);
    assert_eq!(
        f.fs.bytes_read_of(&abs("d/alone.bin")),
        0,
        "a unique size is never read"
    );
    assert_eq!(
        f.fs.bytes_read_of(&abs("d/head.bin")),
        65_536,
        "partial only"
    );
    assert_eq!(
        f.fs.bytes_read_of(&abs("d/tail.bin")),
        65_536 + LARGE as u64,
        "partial, then full"
    );
    assert_eq!(scan.files_seen, 5);
}

#[test]
fn small_files_are_not_compared_by_default_and_empty_files_never() {
    let f = fixture();
    f.fs.add_file_with_content(abs("d/a.txt"), b"same");
    f.fs.add_file_with_content(abs("d/b.txt"), b"same");
    f.fs.add_file_with_content(abs("d/e1"), b"");
    f.fs.add_file_with_content(abs("d/e2"), b"");
    let by_default = f
        .finder
        .scan(&[abs("d")], &TidyOptions::default(), &Quiet)
        .expect("scan");
    assert!(by_default.duplicates.is_empty());
    let small = scan_of(&f, &["d"]);
    assert_eq!(
        group_paths(&small),
        vec![vec![abs("d/a.txt"), abs("d/b.txt")]]
    );
}

#[test]
fn hard_links_to_one_file_are_not_duplicates() {
    let f = fixture();
    let bytes = data(3, 5000);
    f.fs.add_file_with_content(abs("d/one.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/two.bin"), &bytes);
    f.fs.set_identity(abs("d/one.bin"), 7, 42, 2);
    f.fs.set_identity(abs("d/two.bin"), 7, 42, 2);
    let linked = scan_of(&f, &["d"]);
    assert!(linked.duplicates.is_empty());
    assert_eq!(linked.hard_links_folded, 1);

    // A real copy beside them makes a group of two, with the second name noted.
    f.fs.add_file_with_content(abs("d/copy.bin"), &bytes);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(scan.duplicates.len(), 1);
    assert_eq!(scan.duplicates[0].files.len(), 2);
    assert_eq!(scan.duplicates[0].hard_links, 1);
}

#[test]
fn a_cloud_placeholder_is_never_read() {
    let f = fixture();
    let bytes = data(4, 5000);
    f.fs.add_file_with_content(abs("d/real.bin"), &bytes);
    f.fs.add_placeholder(abs("d/cloud.bin"), 5000);
    let scan = scan_of(&f, &["d"]);
    assert!(scan.duplicates.is_empty());
    assert_eq!(scan.cloud_skipped, 1);
    assert_eq!(f.fs.placeholder_reads(), 0);
}

#[test]
fn links_and_system_folders_are_left_out() {
    let f = fixture();
    let bytes = data(5, 5000);
    f.fs.add_file_with_content(abs("d/a.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/b.bin"), &bytes);
    f.fs.add_link(abs("d/junction"));
    f.fs.add_file_with_content(abs("d/junction/c.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/$RECYCLE.BIN/d.bin"), &bytes);
    f.fs.add_file_with_content(abs("Windows/e.bin"), &bytes);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(
        group_paths(&scan),
        vec![vec![abs("d/a.bin"), abs("d/b.bin")]]
    );
    assert_eq!(scan.links, 1);
    assert!(scan.excluded >= 1);
    assert!(
        !f.fs
            .read_dir_calls()
            .iter()
            .any(|dir| dir.starts_with(abs("d/junction")) && *dir != abs("d/junction")),
        "a link is never entered"
    );
    // A system folder, or one inside it, cannot be chosen.
    for root in [abs("Windows"), abs("Program Files/App")] {
        f.fs.add_dir(root.clone(), None);
        assert_eq!(
            f.finder.scan(&[root], &options(), &Quiet).unwrap_err(),
            LargeFilesError::SystemFolder
        );
    }
    // And a system folder inside a chosen one is not entered.
    f.fs.add_file_with_content(abs("d/Program Files/x.bin"), &bytes);
    f.fs.add_dir(abs("Program Files"), None);
    let wide = f.finder.scan(&[abs("")], &options(), &Quiet).expect("scan");
    assert!(
        wide.duplicates
            .iter()
            .flat_map(|g| &g.files)
            .all(|file| !file.path.starts_with(abs("Windows"))
                && !file.path.starts_with(abs("Program Files")))
    );
}

#[test]
fn a_folder_inside_another_chosen_one_is_not_scanned_twice() {
    let f = fixture();
    let bytes = data(6, 5000);
    f.fs.add_file_with_content(abs("d/a.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/sub/b.bin"), &bytes);
    f.fs.add_file_with_content(abs("other/c.bin"), &bytes);
    let scan = f
        .finder
        .scan(
            &[abs("d/sub"), abs("d"), abs("d"), abs("other")],
            &options(),
            &Quiet,
        )
        .expect("scan");
    assert_eq!(scan.roots, vec![abs("d"), abs("other")]);
    assert_eq!(scan.duplicates.len(), 1);
    assert_eq!(scan.duplicates[0].files.len(), 3, "each file listed once");
}

#[test]
fn bad_roots_are_refused() {
    let f = fixture();
    f.fs.add_file(abs("file.txt"), 1);
    let scan = |roots: &[PathBuf]| f.finder.scan(roots, &options(), &Quiet).unwrap_err();
    assert_eq!(
        scan(&[PathBuf::from("relative")]),
        LargeFilesError::NotAbsolute
    );
    assert_eq!(scan(&[abs("missing")]), LargeFilesError::NotFound);
    assert_eq!(scan(&[abs("file.txt")]), LargeFilesError::NotDirectory);
    assert_eq!(scan(&[]), LargeFilesError::NotFound);
}

#[test]
fn a_cancelled_scan_stops_and_changes_nothing() {
    let f = fixture();
    let bytes = data(7, 5000);
    for n in 0..6 {
        f.fs.add_file_with_content(abs(&format!("d/s{n}/a.bin")), &bytes);
    }
    for stop in [0, 3, 8] {
        let error = f
            .finder
            .scan(&[abs("d")], &options(), &StopAfter(Cell::new(stop)))
            .unwrap_err();
        assert_eq!(error, LargeFilesError::Cancelled);
    }
    assert!(f.fs.trashed().is_empty());
}

#[test]
fn empty_folders_are_the_topmost_of_each_empty_tree() {
    let f = fixture();
    f.fs.add_dir(abs("d/a/b/c"), None); // a holds only b, which holds only c.
    f.fs.add_dir(abs("d/keep/inner"), None); // keep has a file below: only inner is empty.
    f.fs.add_file(abs("d/keep/note.txt"), 0);
    f.fs.add_dir(abs("d/linked/x"), None);
    f.fs.add_link(abs("d/linked/lnk"));
    f.fs.add_dir(abs("d/deep/er/est"), None);
    f.fs.add_file(abs("d/deep/er/file.txt"), 1);
    let scan = scan_of(&f, &["d"]);
    let listed: Vec<(PathBuf, u32)> = scan
        .empty_folders
        .iter()
        .map(|e| (e.path.clone(), e.nested))
        .collect();
    assert_eq!(
        listed,
        vec![
            (abs("d/a"), 2),
            (abs("d/deep/er/est"), 0),
            (abs("d/keep/inner"), 0),
            (abs("d/linked/x"), 0),
        ]
    );
    assert!(scan.empty_folders.iter().all(|e| e.root == abs("d")));
}

#[test]
fn a_scanned_folder_that_is_empty_is_not_offered_but_what_is_inside_is() {
    let f = fixture();
    f.fs.add_dir(abs("d/only"), None);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(
        scan.empty_folders
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>(),
        vec![abs("d/only")]
    );
    let nothing = fixture();
    nothing.fs.add_dir(abs("d"), None);
    assert!(scan_of(&nothing, &["d"]).empty_folders.is_empty());
}

#[test]
fn an_unreadable_folder_is_never_called_empty() {
    let f = fixture();
    f.fs.add_dir(abs("d/a/b"), None);
    f.fs.add_dir(abs("d/z"), None);
    f.fs.fail_read_dir_of(abs("d/a/b"), PortError::AccessDenied("no".to_owned()));
    let scan = scan_of(&f, &["d"]);
    assert_eq!(
        scan.empty_folders
            .iter()
            .map(|e| e.path.clone())
            .collect::<Vec<_>>(),
        vec![abs("d/z")]
    );
    assert_eq!(scan.unreadable, 1);
}

fn shortcut(f: &Fixture, name: &str, target: &str, drive: u32, unicode: bool) {
    let bytes = build(target, drive, unicode);
    f.fs.add_file_with_content(abs(name), &bytes);
}

#[test]
fn a_shortcut_is_broken_only_when_its_target_is_missing_on_a_connected_disk() {
    let f = fixture();
    f.fs.add_file(abs("Tools/app.exe"), 10);
    let here = abs("Tools/app.exe").display().to_string();
    let gone = abs("Tools/gone.exe").display().to_string();
    shortcut(&f, "d/ok.lnk", &here, DRIVE_FIXED, false);
    shortcut(&f, "d/broken.lnk", &gone, DRIVE_FIXED, false);
    shortcut(&f, "d/wide.LNK", &gone, DRIVE_FIXED, true);
    // A removable or network drive comes and goes: never judged.
    shortcut(&f, "d/usb.lnk", &gone, 2, false);
    shortcut(&f, "d/net.lnk", &gone, 4, false);
    // No local path in the file at all.
    f.fs.add_file_with_content(abs("d/app.lnk"), &[0_u8; 200]);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(
        scan.broken_shortcuts
            .iter()
            .map(|s| (s.path.clone(), s.target.clone()))
            .collect::<Vec<_>>(),
        vec![
            (abs("d/broken.lnk"), gone.clone()),
            (abs("d/wide.LNK"), gone),
        ]
    );
    assert_eq!(scan.shortcuts_seen, 6);
    assert_eq!(scan.shortcuts_unchecked, 3);
}

#[cfg(windows)]
#[test]
fn a_shortcut_to_a_drive_that_is_not_connected_is_not_broken() {
    let f = fixture();
    shortcut(&f, "d/away.lnk", "E:\\Photos\\a.jpg", DRIVE_FIXED, false);
    let scan = scan_of(&f, &["d"]);
    assert!(scan.broken_shortcuts.is_empty());
    assert_eq!(scan.shortcuts_unchecked, 1);
}

fn two_copies(f: &Fixture) -> TidyScan {
    let bytes = data(8, 5000);
    f.fs.add_file_with_content(abs("d/a.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/b.bin"), &bytes);
    f.fs.add_file_with_content(abs("d/c.bin"), &bytes);
    scan_of(f, &["d"])
}

fn pick(group: usize, files: &[usize]) -> DuplicatePick {
    DuplicatePick {
        group,
        files: files.to_vec(),
    }
}

#[test]
fn trash_moves_the_chosen_copies_and_keeps_the_rest() {
    let f = fixture();
    let scan = two_copies(&f);
    let result =
        f.finder
            .trash_duplicates(&scan, &[pick(0, &[1, 2, 2]), pick(9, &[0])], Source::Manual);
    assert_eq!(result.moved.len(), 2);
    assert_eq!(result.moved_bytes(), 10_000);
    assert!(result.skipped.is_empty());
    assert_eq!(f.fs.trashed(), vec![abs("d/b.bin"), abs("d/c.bin")]);
    assert!(f.fs.exists(&abs("d/a.bin")));
    let records = f.activity.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert!(!records[0].summary.contains("bin"), "no path in the log");
}

#[test]
fn trash_never_removes_every_copy_of_a_group() {
    let f = fixture();
    let scan = two_copies(&f);
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[0, 1, 2])], Source::Manual);
    assert!(result.moved.is_empty());
    assert_eq!(result.skipped.len(), 3);
    assert!(
        result
            .skipped
            .iter()
            .all(|(_, reason)| *reason == CleanSkipReason::NoCopyKept)
    );
    assert!(f.fs.trashed().is_empty());
    assert_eq!(f.activity.records()[0].outcome, ActivityOutcome::Failed);
}

#[test]
fn trash_checks_the_kept_copy_again_before_removing_the_others() {
    // Its content changed behind an unchanged listing.
    let f = fixture();
    let scan = two_copies(&f);
    f.fs.set_content(abs("d/a.bin"), &data(99, 5000));
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[1, 2])], Source::Manual);
    assert!(result.moved.is_empty());
    assert!(
        result
            .skipped
            .iter()
            .all(|(_, reason)| *reason == CleanSkipReason::KeptCopyLost)
    );
    assert!(f.fs.trashed().is_empty());
}

#[test]
fn trash_uses_another_kept_copy_when_one_is_lost_and_leaves_changed_copies() {
    let f = fixture();
    let scan = two_copies(&f);
    // The first copy changed; the third is still good and is the one that stays.
    f.fs.set_content(abs("d/a.bin"), &data(99, 5000));
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[0, 1])], Source::Manual);
    assert_eq!(result.moved.len(), 2, "c.bin still matches");
    assert_eq!(f.fs.trashed(), vec![abs("d/a.bin"), abs("d/b.bin")]);

    // A chosen copy that was touched after the scan is left where it is.
    let f = fixture();
    let scan = two_copies(&f);
    f.fs.add_file_at(abs("d/b.bin"), 5000, Timestamp::from_unix_millis(5));
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[1, 2])], Source::Manual);
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].1, CleanSkipReason::Changed);
    assert_eq!(f.fs.trashed(), vec![abs("d/c.bin")]);
}

#[test]
fn trash_leaves_a_locked_copy_and_says_so() {
    let f = fixture();
    let scan = two_copies(&f);
    f.fs.fail_trash_of(abs("d/b.bin"), PortError::AccessDenied("locked".to_owned()));
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[1, 2])], Source::Manual);
    assert_eq!(result.moved.len(), 1);
    assert_eq!(
        result.skipped,
        vec![(abs("d/b.bin"), CleanSkipReason::LockedOrDenied)]
    );
}

#[test]
fn a_failing_activity_log_does_not_undo_the_move() {
    let f = fixture();
    let scan = two_copies(&f);
    f.activity
        .fail_records_with(Some(PortError::Other("disk full".to_owned())));
    let result = f
        .finder
        .trash_duplicates(&scan, &[pick(0, &[1])], Source::Manual);
    assert!(result.activity_write_failed);
    assert_eq!(result.moved.len(), 1);
}

#[test]
fn empty_folders_are_checked_again_before_they_move() {
    let f = fixture();
    f.fs.add_dir(abs("d/a/b"), None);
    f.fs.add_dir(abs("d/c"), None);
    f.fs.add_dir(abs("d/e"), None);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(scan.empty_folders.len(), 3);
    // Something was put into c after the scan; e is gone.
    f.fs.add_file(abs("d/c/new.txt"), 1);
    f.fs.add_dir(abs("d/e/f"), None);
    f.fs.add_file(abs("d/e/f/g.txt"), 1);
    let result = f
        .finder
        .trash_empty_folders(&scan, &[0, 1, 2, 7], Source::Manual);
    assert_eq!(f.fs.trashed(), vec![abs("d/a")]);
    assert_eq!(
        result.skipped,
        vec![
            (abs("d/c"), CleanSkipReason::NotEmpty),
            (abs("d/e"), CleanSkipReason::NotEmpty)
        ]
    );
    assert_eq!(f.activity.records().len(), 1);
}

#[test]
fn a_broken_shortcut_whose_target_is_back_is_left_alone() {
    let f = fixture();
    let gone = abs("Tools/gone.exe").display().to_string();
    shortcut(&f, "d/a.lnk", &gone, DRIVE_FIXED, false);
    shortcut(&f, "d/b.lnk", &gone, DRIVE_FIXED, false);
    let scan = scan_of(&f, &["d"]);
    assert_eq!(scan.broken_shortcuts.len(), 2);
    f.fs.add_file(abs("Tools/gone.exe"), 1);
    let result = f.finder.trash_shortcuts(&scan, &[0, 1], Source::Manual);
    assert!(result.moved.is_empty());
    assert!(
        result
            .skipped
            .iter()
            .all(|(_, reason)| *reason == CleanSkipReason::Changed)
    );

    // Still missing: moved, with its size.
    let f = fixture();
    shortcut(&f, "d/a.lnk", &gone, DRIVE_FIXED, false);
    let scan = scan_of(&f, &["d"]);
    let result = f.finder.trash_shortcuts(&scan, &[0], Source::Manual);
    assert_eq!(f.fs.trashed(), vec![abs("d/a.lnk")]);
    assert_eq!(result.moved_bytes(), scan.broken_shortcuts[0].size_bytes);
}
