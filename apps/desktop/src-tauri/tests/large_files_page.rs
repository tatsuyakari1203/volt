//! The large files finder at the IPC boundary: scan, filter inputs, move to the Recycle Bin and
//! the activity record, all over the in-memory fakes. Nothing touches the real file system.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::FakeSystem;
use volt_desktop_lib::IpcError;
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::large_files::{FileGroupView, LargeFilesProgress, scan_with, trash_with};
use volt_desktop_lib::wiring::{AppCore, CleaningSetup, build_core_with_cleaning};

fn abs(rest: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    path.extend(rest.split('/'));
    path
}

fn core(system: &FakeSystem, dir: &TempDir) -> AppCore {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set_system_folders(vec![abs("Windows"), abs("Program Files")]);
    build_core_with_cleaning(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders,
        },
    )
    .expect("core builds")
}

fn quiet(_: LargeFilesProgress) {}

#[test]
fn a_scan_lists_the_biggest_files_and_leaves_the_windows_folder_out() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.file_system.add_file(abs("Data/film.mkv"), 4000);
    system.file_system.add_file(abs("Data/notes.txt"), 10);
    system.file_system.add_file(abs("Windows/big.dll"), 9000);
    let core = core(&system, &dir);
    let root = abs("Data").display().to_string();
    let scan = scan_with(&core, Some(&root), &quiet).expect("scan");
    assert!(!scan.cancelled);
    assert_eq!(scan.files.len(), 2);
    assert_eq!(scan.files[0].group, FileGroupView::Video);
    assert!((scan.files[0].size_bytes - 4000.0).abs() < f64::EPSILON);

    let windows = abs("Windows").display().to_string();
    assert!(matches!(
        scan_with(&core, Some(&windows), &quiet),
        Err(IpcError::InvalidInput { .. })
    ));
}

#[test]
fn moving_files_uses_the_recycle_bin_path_records_activity_and_renumbers_the_list() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.file_system.add_file(abs("Data/a.iso"), 300);
    system.file_system.add_file(abs("Data/b.zip"), 200);
    system.file_system.add_file(abs("Data/c.bin"), 100);
    let core = core(&system, &dir);
    let root = abs("Data").display().to_string();
    let scan = scan_with(&core, Some(&root), &quiet).expect("scan");

    let done = trash_with(&core, &scan.scan_id, &[0, 2]).expect("moved");
    assert_eq!(done.moved_files, 2);
    assert!((done.moved_bytes - 400.0).abs() < f64::EPSILON);
    assert_eq!(done.files.len(), 1);
    assert_eq!(done.files[0].index, 0);
    assert_eq!(
        system.file_system.trashed(),
        vec![abs("Data/a.iso"), abs("Data/c.bin")]
    );
    let records = core.activity.list(10, None).expect("records");
    assert!(
        records
            .iter()
            .any(|r| r.label.as_deref() == Some("large_files")),
        "{records:?}"
    );

    // Rows, scan ids and empty choices are checked before anything moves.
    assert!(matches!(
        trash_with(&core, &scan.scan_id, &[5]),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        trash_with(&core, &scan.scan_id, &[]),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        trash_with(&core, "large-999", &[0]),
        Err(IpcError::NotFound { .. })
    ));
    assert_eq!(system.file_system.trashed().len(), 2);
}
