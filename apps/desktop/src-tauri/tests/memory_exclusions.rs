//! The exclusion list over a real core built on the in-memory fakes and a throwaway store.
//!
//! Nothing here touches this machine: the processes the trim sees are the ones `FakeMemory` is
//! given, and no process is opened or trimmed.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::elevation::fakes::FakeElevator;
use volt_core::fakes::FakeSystem;
use volt_desktop_lib::IpcError;
use volt_desktop_lib::boost::boost_of;
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::elevation::clean_memory_elevated_of;
use volt_desktop_lib::exclusions::{
    add_memory_exclusion_of, memory_exclusions_of, remove_memory_exclusion_of,
};
use volt_desktop_lib::memory::{MemoryAreaView, clean_memory_of};
use volt_desktop_lib::wiring::{AppCore, build_core};

fn core_at(system: &FakeSystem, dir: &TempDir) -> AppCore {
    build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds")
}

#[test]
fn the_list_is_normalised_deduplicated_and_survives_a_restart() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    {
        let core = core_at(&system, &dir);
        assert!(memory_exclusions_of(&core).expect("list").is_empty());
        add_memory_exclusion_of(&core, "Game").expect("add");
        add_memory_exclusion_of(&core, r"C:\Games\GAME.EXE").expect("add again");
        let names = add_memory_exclusion_of(&core, "db.exe").expect("add");
        assert_eq!(names, ["db.exe", "game.exe"]);
    }
    let core = core_at(&system, &dir);
    assert_eq!(
        memory_exclusions_of(&core).expect("list"),
        ["db.exe", "game.exe"]
    );
    assert_eq!(
        remove_memory_exclusion_of(&core, "GAME").expect("remove"),
        ["db.exe"]
    );
    assert_eq!(
        remove_memory_exclusion_of(&core, "nothing").expect("remove"),
        ["db.exe"]
    );
}

#[test]
fn an_impossible_name_is_refused_and_changes_nothing() {
    let dir = TempDir::new().expect("temp dir");
    let core = core_at(&FakeSystem::new(), &dir);
    assert!(matches!(
        add_memory_exclusion_of(&core, "   "),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        add_memory_exclusion_of(&core, "a|b"),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(memory_exclusions_of(&core).expect("list").is_empty());
}

#[test]
fn the_memory_page_clean_skips_a_listed_process_and_reports_how_many() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .memory
        .set_processes(&[("game.exe", true), ("notes.exe", true), ("svc.exe", false)]);
    let core = core_at(&system, &dir);
    add_memory_exclusion_of(&core, "game").expect("add");

    let result = clean_memory_of(&core, &[MemoryAreaView::WorkingSets]).expect("run");
    assert_eq!(system.memory.trimmed_processes(), ["notes.exe"]);
    let area = &result.areas[0];
    assert_eq!(
        (
            area.processes_trimmed,
            area.processes_skipped,
            area.processes_excluded
        ),
        (1, 1, 1)
    );
    let records = core.activity.list(10, None).expect("activity");
    assert!(
        records[0]
            .summary
            .contains("1 left alone on the exclusion list"),
        "{}",
        records[0].summary
    );
}

#[test]
fn boost_and_so_the_tray_skip_a_listed_process_and_say_so() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .memory
        .set_processes(&[("game.exe", true), ("notes.exe", true)]);
    let core = core_at(&system, &dir);
    add_memory_exclusion_of(&core, "notes.exe").expect("add");

    // The tray runs the same `boost_of` as the Home button.
    let result = boost_of(&core).expect("boost");
    assert_eq!(system.memory.trimmed_processes(), ["game.exe"]);
    assert_eq!(result.areas[0].processes_excluded, 1);

    // Taking the program off the list makes the next run trim it.
    remove_memory_exclusion_of(&core, "notes.exe").expect("remove");
    let _ = boost_of(&core).expect("boost");
    assert_eq!(
        system.memory.trimmed_processes(),
        ["game.exe", "game.exe", "notes.exe"]
    );
}

#[test]
fn a_working_set_trim_never_goes_to_the_helper_where_the_list_would_not_apply() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::granted(system.memory.clone()));
    let core = core_at(&system, &dir).with_elevator(elevator.clone());
    let refused = clean_memory_elevated_of(
        &core,
        &[MemoryAreaView::StandbyList, MemoryAreaView::WorkingSets],
    );
    assert!(matches!(refused, Err(IpcError::InvalidInput { .. })));
    assert_eq!(elevator.prompts(), 0);
    assert!(system.memory.purged().is_empty());
}
