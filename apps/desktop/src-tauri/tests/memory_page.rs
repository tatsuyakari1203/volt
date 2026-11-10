//! The Memory commands over a real core built on the in-memory fakes and a throwaway store.
//!
//! Nothing here touches this machine's memory: the port is `FakeMemory`.

use tempfile::TempDir;
use volt_core::fakes::FakeSystem;
use volt_core::{Availability, MemoryArea, PortError};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::memory::{
    AreaStatus, MemoryAreaView, OperationState, clean_memory_of, memory_state_of,
};
use volt_desktop_lib::wiring::{AppCore, build_core};

fn core_with(system: &FakeSystem, dir: &TempDir) -> AppCore {
    build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds")
}

#[test]
fn the_state_lists_memory_by_list_and_every_operation_with_its_reason() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .memory
        .make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
    system
        .memory
        .make_unavailable(MemoryArea::FileCache, Availability::Unsupported);
    let core = core_with(&system, &dir);

    let state = memory_state_of(&core).expect("state");
    let lists = state.lists.expect("lists");
    assert!(state.problem.is_none());
    let sum = lists.in_use_bytes + lists.standby_bytes + lists.modified_bytes + lists.free_bytes;
    assert!((sum - lists.total_bytes).abs() < 1.0);

    let areas: Vec<_> = state.operations.iter().map(|op| op.area).collect();
    assert_eq!(
        areas,
        [
            MemoryAreaView::WorkingSets,
            MemoryAreaView::FileCache,
            MemoryAreaView::ModifiedList,
            MemoryAreaView::StandbyListLowPriority,
            MemoryAreaView::StandbyList,
        ]
    );
    let by = |area| {
        state
            .operations
            .iter()
            .find(|op| op.area == area)
            .expect("op")
    };
    assert_eq!(
        by(MemoryAreaView::WorkingSets).state,
        OperationState::Available
    );
    assert!(by(MemoryAreaView::WorkingSets).reason.is_none());
    let standby = by(MemoryAreaView::StandbyList);
    assert_eq!(standby.state, OperationState::NeedsAdministrator);
    assert!(
        standby
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("administrator"))
    );
    assert_eq!(
        by(MemoryAreaView::FileCache).state,
        OperationState::Unsupported
    );
    // Reading the state purged nothing.
    assert!(system.memory.purged().is_empty());
}

#[test]
fn a_run_reports_before_and_after_per_operation_and_records_a_non_undoable_activity() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .memory
        .make_unavailable(MemoryArea::ModifiedList, Availability::NeedsAdministrator);
    let core = core_with(&system, &dir);

    let result = clean_memory_of(
        &core,
        &[
            MemoryAreaView::StandbyList,
            MemoryAreaView::ModifiedList,
            MemoryAreaView::WorkingSets,
        ],
    )
    .expect("run");

    assert!(result.recorded);
    let (before, after) = (result.before.expect("before"), result.after.expect("after"));
    assert!(after.free_bytes > before.free_bytes);
    let statuses: Vec<_> = result.areas.iter().map(|a| (a.area, a.status)).collect();
    assert_eq!(
        statuses,
        [
            (MemoryAreaView::WorkingSets, AreaStatus::Done),
            (MemoryAreaView::ModifiedList, AreaStatus::NeedsAdministrator),
            (MemoryAreaView::StandbyList, AreaStatus::Done),
        ]
    );
    assert_eq!(result.areas[0].processes_trimmed, 2);
    assert_eq!(result.areas[0].processes_skipped, 1);

    // Only what was asked for ran, and the activity log (the store) says it cannot be undone.
    assert_eq!(
        system.memory.purged(),
        [MemoryArea::WorkingSets, MemoryArea::StandbyList]
    );
    let records = core.activity.list(10, None).expect("activity");
    assert_eq!(records.len(), 1);
    assert!(records[0].summary.contains("cannot be undone"));
    assert_eq!((records[0].batch, records[0].entry), (None, None));
}

#[test]
fn a_failure_is_a_typed_status_and_an_empty_selection_is_refused() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system
        .memory
        .fail_purge(MemoryArea::FileCache, PortError::Other("boom".into()));
    let core = core_with(&system, &dir);

    let result = clean_memory_of(&core, &[MemoryAreaView::FileCache]).expect("run");
    assert_eq!(result.areas[0].status, AreaStatus::Failed);
    assert!(result.areas[0].detail.is_some());

    assert!(matches!(
        clean_memory_of(&core, &[]),
        Err(IpcError::InvalidInput { .. })
    ));
}
