//! The administrator commands over a real core built on the in-memory fakes and a throwaway store.
//!
//! Nothing here starts a helper, shows a prompt or touches this machine's memory: the elevator is
//! `FakeElevator` and the memory port is `FakeMemory`.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::elevation::fakes::{ElevatorCall, FakeElevator};
use volt_core::fakes::FakeSystem;
use volt_core::{Availability, MemoryArea, SystemProtection, UnavailableReason};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::boost::boost_of;
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::elevation::{
    ElevatedCleanResult, ElevationReasonView, ElevationStatusView, SystemProtectionView,
    clean_memory_elevated_of, elevation_status_of, end_session_of, system_protection_of_app,
};
use volt_desktop_lib::memory::{AreaStatus, MemoryAreaView};
use volt_desktop_lib::wiring::{AppCore, build_core};

fn core_with(system: &FakeSystem, dir: &TempDir, elevator: Option<Arc<FakeElevator>>) -> AppCore {
    let core = build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    match elevator {
        Some(elevator) => core.with_elevator(elevator),
        None => core,
    }
}

fn granted(system: &FakeSystem) -> Arc<FakeElevator> {
    Arc::new(FakeElevator::granted(system.memory.clone()))
}

#[test]
fn system_protection_is_a_hint_read_without_a_prompt() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));
    for (value, view) in [
        (SystemProtection::On, SystemProtectionView::On),
        (SystemProtection::Off, SystemProtectionView::Off),
        (SystemProtection::Unknown, SystemProtectionView::Unknown),
    ] {
        elevator.set_system_protection(value);
        assert_eq!(system_protection_of_app(&core).expect("hint"), view);
    }
    assert_eq!(elevator.prompts(), 0);
}

#[test]
fn the_status_is_available_or_names_the_reason_and_never_prompts() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));
    assert_eq!(
        elevation_status_of(&core).expect("status"),
        ElevationStatusView::Available
    );
    assert_eq!(elevator.prompts(), 0);

    for (reason, expected) in [
        (
            UnavailableReason::NotAllUsersInstall,
            ElevationReasonView::NotAllUsers,
        ),
        (
            UnavailableReason::HelperMissing,
            ElevationReasonView::HelperMissing,
        ),
        (
            UnavailableReason::InstallNotProtected { detail: "x".into() },
            ElevationReasonView::InstallNotProtected,
        ),
        (
            UnavailableReason::StandardUser,
            ElevationReasonView::StandardUser,
        ),
        (
            UnavailableReason::Unknown { detail: "y".into() },
            ElevationReasonView::Unknown,
        ),
    ] {
        let elevator = Arc::new(FakeElevator::unavailable_because(
            system.memory.clone(),
            reason,
        ));
        let core = core_with(&system, &dir, Some(elevator.clone()));
        match elevation_status_of(&core).expect("status") {
            ElevationStatusView::Unavailable { reason, detail } => {
                assert_eq!(reason, expected);
                assert!(!detail.is_empty());
            }
            other @ ElevationStatusView::Available => panic!("expected unavailable, got {other:?}"),
        }
        assert_eq!(elevator.prompts(), 0);
    }
}

#[test]
fn an_elevated_purge_reports_before_and_after_and_records_one_activity() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));

    let outcome = clean_memory_elevated_of(
        &core,
        &[MemoryAreaView::StandbyList, MemoryAreaView::ModifiedList],
    )
    .expect("run");
    let ElevatedCleanResult::Done { report } = outcome else {
        panic!("expected a done run");
    };
    assert!(report.recorded);
    let (before, after) = (report.before.expect("before"), report.after.expect("after"));
    assert!(after.free_bytes > before.free_bytes);
    assert!(
        report
            .areas
            .iter()
            .all(|area| area.status == AreaStatus::Done)
    );
    assert_eq!(
        system.memory.purged(),
        [MemoryArea::ModifiedList, MemoryArea::StandbyList]
    );

    let records = core.activity.list(10, None).expect("activity");
    assert_eq!(records.len(), 1, "one record per purge");
    assert!(records[0].summary.contains("as administrator"));
    assert!(records[0].summary.contains("cannot be undone"));
    assert!(records[0].summary.contains("free memory"));
    assert_eq!((records[0].batch, records[0].entry), (None, None));

    // The next purge of the session shows no prompt and is recorded on its own.
    clean_memory_elevated_of(&core, &[MemoryAreaView::StandbyListLowPriority]).expect("again");
    assert_eq!(elevator.prompts(), 1);
    assert_eq!(core.activity.list(10, None).expect("activity").len(), 2);
}

#[test]
fn a_cancelled_prompt_is_declined_and_changes_and_records_nothing() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::declined(system.memory.clone()));
    let core = core_with(&system, &dir, Some(elevator));

    assert_eq!(
        clean_memory_elevated_of(&core, &[MemoryAreaView::StandbyList]).expect("run"),
        ElevatedCleanResult::Declined
    );
    assert!(system.memory.purged().is_empty());
    assert!(core.activity.list(10, None).expect("activity").is_empty());
}

#[test]
fn an_unavailable_elevator_is_an_error_and_never_prompts() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::unavailable_because(
        system.memory.clone(),
        UnavailableReason::NotAllUsersInstall,
    ));
    let core = core_with(&system, &dir, Some(elevator.clone()));
    assert!(matches!(
        clean_memory_elevated_of(&core, &[MemoryAreaView::StandbyList]),
        Err(IpcError::Unsupported { .. })
    ));
    assert_eq!(elevator.prompts(), 0);
    assert!(system.memory.purged().is_empty());
    assert!(core.activity.list(10, None).expect("activity").is_empty());
}

#[test]
fn an_empty_selection_is_refused_before_the_elevator_is_asked() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));
    assert!(matches!(
        clean_memory_elevated_of(&core, &[]),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(elevator.calls().is_empty());
}

#[test]
fn a_core_without_an_elevator_answers_unsupported() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let core = core_with(&system, &dir, None);
    assert!(matches!(
        elevation_status_of(&core),
        Err(IpcError::Unsupported { .. })
    ));
    assert!(matches!(
        clean_memory_elevated_of(&core, &[MemoryAreaView::StandbyList]),
        Err(IpcError::Unsupported { .. })
    ));
}

#[test]
fn the_session_ends_with_the_app() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));
    clean_memory_elevated_of(&core, &[MemoryAreaView::StandbyList]).expect("run");
    assert!(elevator.is_active());
    end_session_of(&core);
    assert!(!elevator.is_active());
    assert_eq!(elevator.calls().last(), Some(&ElevatorCall::Bye));
    // No elevator: nothing to end, and no panic.
    end_session_of(&core_with(&system, &dir, None));
}

#[test]
fn boost_never_calls_the_elevator_and_reports_what_it_skipped() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    // The list purges need administrator rights in this session.
    system
        .memory
        .make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
    system
        .memory
        .make_unavailable(MemoryArea::ModifiedList, Availability::NeedsAdministrator);
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(elevator.clone()));

    let report = boost_of(&core).expect("boost");
    assert!(!report.areas.is_empty(), "Boost ran what it can run");
    assert!(elevator.calls().is_empty(), "{:?}", elevator.calls());
    assert_eq!(elevator.prompts(), 0);
    assert!(!elevator.is_active());
}

/// The tray runs Boost from Rust with no window and cannot be driven here, so its source is
/// checked too: neither it nor Boost may so much as name the elevator.
#[test]
fn neither_boost_nor_the_tray_mention_the_elevator() {
    for (name, source) in [
        ("boost.rs", include_str!("../src/boost.rs")),
        ("tray.rs", include_str!("../src/tray.rs")),
    ] {
        let lower = source.to_lowercase();
        for forbidden in ["elevator", "elevation", "clean_memory_elevated"] {
            assert!(!lower.contains(forbidden), "{name} mentions {forbidden}");
        }
    }
}
