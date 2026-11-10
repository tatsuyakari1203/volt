//! Boost over a real core built on the in-memory fakes and a throwaway store. Nothing here touches
//! this machine: the memory, the file system and the known folders are all fakes.

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::FakeSystem;
use volt_core::{Availability, Location, MemoryArea, Timestamp};
use volt_desktop_lib::boost::{BoostConfigView, boost_config_of, boost_of, set_boost_config_of};
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::memory::{AreaStatus, MemoryAreaView, OperationState};
use volt_desktop_lib::wiring::{AppCore, CleaningSetup, build_core_with_cleaning};

fn abs(rest: &str) -> PathBuf {
    let mut path = PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" });
    path.extend(rest.split('/'));
    path
}

fn core_with(system: &FakeSystem, dir: &TempDir) -> AppCore {
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserTemp, abs("Users/tester/AppData/Local/Temp"));
    folders.set(Location::LocalAppData, abs("Users/tester/AppData/Local"));
    folders.set(
        Location::RoamingAppData,
        abs("Users/tester/AppData/Roaming"),
    );
    folders.set(Location::ProgramData, abs("ProgramData"));
    folders.set(Location::Windows, abs("Windows"));
    folders.protect(abs("Users/tester"));
    folders.protect(abs("Windows"));
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

fn add_temp_file(system: &FakeSystem, name: &str, size: u64) {
    // Modified in 1970: old enough for any minimum age.
    system.file_system.add_file_at(
        abs("Users/tester/AppData/Local/Temp").join(name),
        size,
        Timestamp::from_unix_millis(1_000_000),
    );
}

#[test]
fn the_default_boost_trims_working_sets_scans_and_cleans_nothing() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    add_temp_file(&system, "a.tmp", 4096);
    let core = core_with(&system, &dir);

    let result = boost_of(&core).expect("boost");
    assert_eq!(system.memory.purged(), [MemoryArea::WorkingSets]);
    assert_eq!(result.areas.len(), 1);
    assert_eq!(result.areas[0].status, AreaStatus::Done);
    assert!(result.available_before.is_some() && result.available_after.is_some());
    let junk = result.junk.expect("junk scan");
    assert_eq!(junk.files, 1);
    assert!((junk.bytes - 4096.0).abs() < 0.5);
    assert!(result.moved_to_bin.is_none());
    assert!(system.file_system.trashed().is_empty());
    assert!(result.recorded);
}

#[test]
fn the_opt_in_moves_the_temp_files_and_the_choice_is_saved() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    add_temp_file(&system, "a.tmp", 4096);
    let core = core_with(&system, &dir);

    assert_eq!(
        boost_config_of(&core).expect("config").areas,
        [MemoryAreaView::WorkingSets]
    );
    let saved = set_boost_config_of(
        &core,
        &BoostConfigView {
            areas: vec![MemoryAreaView::StandbyList],
            move_temp_to_recycle_bin: true,
        },
    )
    .expect("saved");
    assert!(saved.move_temp_to_recycle_bin);
    assert_eq!(boost_config_of(&core).expect("config"), saved);

    system
        .memory
        .make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
    let result = boost_of(&core).expect("boost");
    let moved = result.moved_to_bin.expect("cleaned");
    assert_eq!(moved.files, 1);
    assert!((moved.bytes - 4096.0).abs() < 0.5);
    assert_eq!(system.file_system.trashed().len(), 1);
    assert!(system.memory.purged().is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].area, MemoryAreaView::StandbyList);
    assert_eq!(result.skipped[0].state, OperationState::NeedsAdministrator);
}
