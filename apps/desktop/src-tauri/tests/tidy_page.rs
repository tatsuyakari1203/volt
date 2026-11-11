//! The tidy-up finder at the IPC boundary: scan, the lists the UI gets, moves to the Recycle Bin
//! and the activity record, all over the in-memory fakes. Nothing touches the real file system.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::FakeSystem;
use volt_desktop_lib::IpcError;
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::tidy::{
    DuplicatePickInput, TidyProgressEvent, scan_with, trash_broken_shortcuts_with,
    trash_duplicates_with, trash_empty_folders_with,
};
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

fn quiet(_: TidyProgressEvent) {}

fn big(seed: u8) -> Vec<u8> {
    vec![seed; 2 << 20]
}

fn roots(rest: &str) -> Vec<String> {
    vec![abs(rest).display().to_string()]
}

#[test]
fn a_scan_lists_duplicates_empty_folders_and_leaves_the_windows_folder_out() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .file_system
        .add_file_with_content(abs("Data/a.bin"), &big(1));
    system
        .file_system
        .add_file_with_content(abs("Data/b.bin"), &big(1));
    system
        .file_system
        .add_file_with_content(abs("Data/c.bin"), &big(2));
    system
        .file_system
        .add_file_with_content(abs("Windows/d.bin"), &big(1));
    system.file_system.add_dir(abs("Data/empty"), None);
    let core = core(&system, &dir);

    let scan = scan_with(&core, &roots("Data"), &quiet).expect("scan");
    assert!(!scan.cancelled);
    assert_eq!(scan.lists.duplicates.len(), 1);
    let group = &scan.lists.duplicates[0];
    assert_eq!(group.files.len(), 2);
    assert!((group.wasted_bytes - f64::from(2_u32 << 20)).abs() < f64::EPSILON);
    assert_eq!(scan.lists.empty_folders.len(), 1);
    assert!(scan.lists.empty_folders[0].path.ends_with("empty"));

    assert!(matches!(
        scan_with(&core, &roots("Windows"), &quiet),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        scan_with(&core, &[], &quiet),
        Err(IpcError::InvalidInput { .. })
    ));
}

#[test]
fn duplicates_move_to_the_recycle_bin_but_never_every_copy() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    for name in ["a", "b", "c"] {
        system
            .file_system
            .add_file_with_content(abs(&format!("Data/{name}.bin")), &big(1));
    }
    let core = core(&system, &dir);
    let scan = scan_with(&core, &roots("Data"), &quiet).expect("scan");
    let pick = |files: &[u32]| {
        vec![DuplicatePickInput {
            group: 0,
            files: files.to_vec(),
        }]
    };

    // Every copy chosen: nothing moves, and the reason is said.
    let refused = trash_duplicates_with(&core, &scan.scan_id, &pick(&[0, 1, 2])).expect("result");
    assert_eq!(refused.moved, 0);
    assert_eq!(refused.skipped_total, 3);
    assert!(system.file_system.trashed().is_empty());

    let done = trash_duplicates_with(&core, &scan.scan_id, &pick(&[1, 2])).expect("moved");
    assert_eq!(done.moved, 2);
    assert!((done.moved_bytes - f64::from(4_u32 << 20)).abs() < f64::EPSILON);
    assert!(done.lists.duplicates.is_empty(), "a lone copy is no group");
    assert_eq!(
        system.file_system.trashed(),
        vec![abs("Data/b.bin"), abs("Data/c.bin")]
    );
    let records = core.activity.list(10, None).expect("records");
    assert!(
        records
            .iter()
            .any(|r| r.label.as_deref() == Some("tidy_duplicates")),
        "{records:?}"
    );

    // Choices are checked before anything moves.
    for bad in [pick(&[9]), Vec::new(), pick(&[])] {
        assert!(matches!(
            trash_duplicates_with(&core, &scan.scan_id, &bad),
            Err(IpcError::InvalidInput { .. })
        ));
    }
    assert!(matches!(
        trash_duplicates_with(&core, "tidy-999", &pick(&[1])),
        Err(IpcError::NotFound { .. })
    ));
}

#[test]
fn empty_folders_and_broken_shortcuts_move_and_leave_their_lists() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.file_system.add_dir(abs("Data/one"), None);
    system.file_system.add_dir(abs("Data/two"), None);
    let core = core(&system, &dir);
    let scan = scan_with(&core, &roots("Data"), &quiet).expect("scan");
    assert_eq!(scan.lists.empty_folders.len(), 2);

    let done = trash_empty_folders_with(&core, &scan.scan_id, &[0]).expect("moved");
    assert_eq!(done.moved, 1);
    assert_eq!(done.lists.empty_folders.len(), 1);
    assert_eq!(done.lists.empty_folders[0].index, 0);
    assert!(matches!(
        trash_empty_folders_with(&core, &scan.scan_id, &[4]),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        trash_broken_shortcuts_with(&core, &scan.scan_id, &[0]),
        Err(IpcError::InvalidInput { .. })
    ));
    assert_eq!(system.file_system.trashed().len(), 1);
}
