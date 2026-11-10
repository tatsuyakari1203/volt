//! Tests of the large files finder against in-memory fakes. Nothing here
//! touches the real file system.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::clean::fakes::FakeKnownFolders;
use crate::fakes::{FakeActivityLog, FakeFileSystem, ManualClock};

const NOW: i64 = 1_800_000_000_000;

fn abs(rest: &str) -> PathBuf {
    let base = if cfg!(windows) { "C:\\" } else { "/" };
    Path::new(base).join(rest)
}

struct Quiet;
impl LargeFilesObserver for Quiet {
    fn progress(&self, _: u64, _: u64) {}
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct StopAfter(Cell<u32>);
impl LargeFilesObserver for StopAfter {
    fn progress(&self, _: u64, _: u64) {}
    fn is_cancelled(&self) -> bool {
        let left = self.0.get();
        self.0.set(left.saturating_sub(1));
        left == 0
    }
}

struct Fixture {
    fs: Arc<FakeFileSystem>,
    activity: Arc<FakeActivityLog>,
    finder: LargeFilesFinder,
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
    let finder = LargeFilesFinder::new(fs.clone(), folders, Arc::new(ManualClock::new(NOW)))
        .with_activity_log(activity.clone());
    Fixture {
        fs,
        activity,
        finder,
    }
}

fn paths(scan: &LargeFilesScan) -> Vec<PathBuf> {
    scan.files.iter().map(|f| f.path.clone()).collect()
}

#[test]
fn lists_the_biggest_files_first_and_keeps_only_the_top_n() {
    let f = fixture();
    for (name, size) in [
        ("a.bin", 10),
        ("b.bin", 50),
        ("sub/c.bin", 30),
        ("d.bin", 20),
    ] {
        f.fs.add_file(abs(&format!("data/{name}")), size);
    }
    let scan = f.finder.scan(&abs("data"), 3, &Quiet).expect("scan");
    assert_eq!(
        paths(&scan),
        vec![abs("data/b.bin"), abs("data/sub/c.bin"), abs("data/d.bin")]
    );
    assert_eq!(scan.files_seen, 4);
    assert_eq!(scan.folders_seen, 2);
}

#[test]
fn groups_files_by_extension() {
    let group = |n: &str| FileGroup::of(std::ffi::OsStr::new(n));
    assert_eq!(group("Movie.MKV"), FileGroup::Video);
    assert_eq!(group("a.7z"), FileGroup::Archive);
    assert_eq!(group("win.iso"), FileGroup::DiskImage);
    assert_eq!(group("app.msi"), FileGroup::Installer);
    assert_eq!(group("VoltSetup.exe"), FileGroup::Installer);
    assert_eq!(group("game.exe"), FileGroup::Other);
    assert_eq!(group("noext"), FileGroup::Other);
}

#[test]
fn leaves_out_system_folders_system_files_and_links() {
    let f = fixture();
    f.fs.add_file(abs("Windows/big.dll"), 900);
    f.fs.add_file(abs("Program Files/app/big.bin"), 900);
    f.fs.add_file(abs("ProgramData/big.bin"), 900);
    f.fs.add_file(abs("pagefile.sys"), 900);
    f.fs.add_file(abs("HIBERFIL.SYS"), 900);
    f.fs.add_file(abs("$Recycle.Bin/x"), 900);
    f.fs.add_link(abs("junction"));
    f.fs.add_file(abs("junction/behind.bin"), 900);
    f.fs.add_file(abs("ok.bin"), 5);
    let scan = f.finder.scan(&abs(""), 10, &Quiet).expect("scan");
    assert_eq!(paths(&scan), vec![abs("ok.bin")]);
    assert_eq!(scan.links, 1);
    assert_eq!(scan.excluded, 6);
}

#[test]
fn refuses_a_system_folder_a_file_and_a_missing_folder() {
    let f = fixture();
    f.fs.add_file(abs("Windows/System32/x.dll"), 1);
    f.fs.add_file(abs("a.bin"), 1);
    assert_eq!(
        f.finder.scan(&abs("windows/system32"), 5, &Quiet),
        Err(LargeFilesError::SystemFolder)
    );
    assert_eq!(
        f.finder.scan(&abs("a.bin"), 5, &Quiet),
        Err(LargeFilesError::NotDirectory)
    );
    assert_eq!(
        f.finder.scan(&abs("nope"), 5, &Quiet),
        Err(LargeFilesError::NotFound)
    );
    assert_eq!(
        f.finder.scan(Path::new("relative"), 5, &Quiet),
        Err(LargeFilesError::NotAbsolute)
    );
}

#[test]
fn refuses_to_scan_when_the_system_folders_are_unknown() {
    let fs = Arc::new(FakeFileSystem::new());
    fs.add_dir(abs("data"), None);
    let finder = LargeFilesFinder::new(
        fs,
        Arc::new(FakeKnownFolders::new()),
        Arc::new(ManualClock::new(NOW)),
    );
    assert!(matches!(
        finder.scan(&abs("data"), 5, &Quiet),
        Err(LargeFilesError::SystemFoldersUnavailable(_))
    ));
}

#[test]
fn counts_unreadable_folders_and_goes_on() {
    let f = fixture();
    f.fs.add_file(abs("data/locked/x.bin"), 100);
    f.fs.add_file(abs("data/y.bin"), 7);
    f.fs.fail_read_dir_of(abs("data/locked"), PortError::AccessDenied("x".into()));
    let scan = f.finder.scan(&abs("data"), 5, &Quiet).expect("scan");
    assert_eq!(paths(&scan), vec![abs("data/y.bin")]);
    assert_eq!(scan.unreadable, 1);
}

#[test]
fn can_be_cancelled() {
    let f = fixture();
    f.fs.add_file(abs("data/a/x.bin"), 1);
    f.fs.add_file(abs("data/b/x.bin"), 1);
    assert_eq!(
        f.finder.scan(&abs("data"), 5, &StopAfter(Cell::new(1))),
        Err(LargeFilesError::Cancelled)
    );
}

#[test]
fn trash_moves_only_the_chosen_files_and_records_it() {
    let f = fixture();
    f.fs.add_file(abs("data/a.bin"), 30);
    f.fs.add_file(abs("data/b.bin"), 20);
    f.fs.add_file(abs("data/c.bin"), 10);
    let scan = f.finder.scan(&abs("data"), 5, &Quiet).expect("scan");
    let result = f.finder.trash(&scan, &[0, 2, 2, 99], Source::Manual);
    assert_eq!(f.fs.trashed(), vec![abs("data/a.bin"), abs("data/c.bin")]);
    assert_eq!(result.moved.len(), 2);
    assert_eq!(result.moved_bytes(), 40);
    assert!(result.skipped.is_empty());
    assert!(f.fs.exists(&abs("data/b.bin")));
    let records = f.activity.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert!(!records[0].summary.contains("data"), "no path in the log");
}

#[test]
fn trash_leaves_changed_gone_and_locked_files_alone() {
    let f = fixture();
    f.fs.add_file(abs("data/a.bin"), 30);
    f.fs.add_file(abs("data/b.bin"), 20);
    f.fs.add_file(abs("data/c.bin"), 10);
    let scan = f.finder.scan(&abs("data"), 5, &Quiet).expect("scan");
    f.fs.add_file(abs("data/a.bin"), 31); // grew since the scan
    f.fs.fail_trash_of(abs("data/b.bin"), PortError::AccessDenied("in use".into()));
    let result = f.finder.trash(&scan, &[0, 1, 2], Source::Manual);
    assert_eq!(f.fs.trashed(), vec![abs("data/c.bin")]);
    assert_eq!(
        result.skipped,
        vec![
            (abs("data/a.bin"), CleanSkipReason::Changed),
            (abs("data/b.bin"), CleanSkipReason::LockedOrDenied),
        ]
    );
    // c.bin is in the Recycle Bin now: a second try finds it gone.
    let gone = f.finder.trash(&scan, &[2], Source::Manual);
    assert_eq!(gone.skipped[0].1, CleanSkipReason::Gone);
    assert_eq!(f.activity.records()[0].outcome, ActivityOutcome::Applied);
    assert_eq!(f.activity.records()[1].outcome, ActivityOutcome::Failed);
}

#[test]
fn a_failing_activity_log_does_not_undo_the_move() {
    let f = fixture();
    f.fs.add_file(abs("data/a.bin"), 30);
    let scan = f.finder.scan(&abs("data"), 5, &Quiet).expect("scan");
    f.activity
        .fail_records_with(Some(PortError::Other("disk full".into())));
    let result = f.finder.trash(&scan, &[0], Source::Manual);
    assert_eq!(result.moved.len(), 1);
    assert!(result.activity_write_failed);
}

#[test]
fn never_trashes_through_a_link() {
    let f = fixture();
    f.fs.add_file(abs("data/real/a.bin"), 30);
    let scan = f.finder.scan(&abs("data"), 5, &Quiet).expect("scan");
    f.fs.add_link(abs("data/real"));
    f.fs.add_file(abs("data/real/a.bin"), 30);
    let result = f.finder.trash(&scan, &[0], Source::Manual);
    assert!(f.fs.trashed().is_empty());
    assert_eq!(result.skipped[0].1, CleanSkipReason::Refused);
}
