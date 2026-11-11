//! Administrator tweaks and the helper's rows on the Undo page, over a real core built on the
//! in-memory fakes and a throwaway store.
//!
//! Nothing here starts a helper, shows a prompt or touches this machine: the elevator is
//! `FakeElevator`, which answers from a script.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::elevation::fakes::{ElevatorCall, FakeElevator};
use volt_core::fakes::FakeSystem;
use volt_core::{
    ActivityId, ActivityOutcome, ActivityRecord, ElevationError, ErrorCode, Journal, Phrase,
    Source, Timestamp, UnavailableReason,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::administrator::{self, MIRROR_SETTINGS_KEY};
use volt_desktop_lib::api::{
    ApplyStatus, ChangeState, RevertStatus, UndoStatus, apply_tweak, clear_history, list_activity,
    list_tweaks, revert_tweak,
};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::wiring::{AppCore, build_core};

/// An administrator tweak of the shipped catalogue.
const TWEAK: &str = "turn-off-error-reporting";

fn core_with(system: &FakeSystem, dir: &TempDir, elevator: Option<&Arc<FakeElevator>>) -> AppCore {
    let core = build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    match elevator {
        Some(elevator) => core.with_elevator(elevator.clone()),
        None => core,
    }
}

fn granted(system: &FakeSystem) -> Arc<FakeElevator> {
    Arc::new(FakeElevator::granted(system.memory.clone()))
}

fn record(id: u64, outcome: ActivityOutcome, label: &str) -> ActivityRecord {
    let phrase = Phrase::new("summary.appliedPlain").text("subject", "the setting");
    ActivityRecord {
        id: ActivityId(id),
        timestamp: Timestamp::from_unix_millis(1_700_000_000_000 + i64::try_from(id).unwrap_or(0)),
        source: Source::Manual,
        outcome,
        summary: phrase.english(),
        batch: Some(volt_core::BatchId(77)),
        entry: Some(volt_core::EntryId(88)),
        label: Some(label.to_owned()),
        phrase: Some(phrase),
    }
}

#[test]
fn an_administrator_tweak_goes_through_the_helper_and_the_ordinary_path_is_not_used() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(&elevator));

    let applied = apply_tweak(&core, TWEAK).expect("answered");
    assert_eq!(applied.status, ApplyStatus::Applied);
    assert!(
        elevator
            .calls()
            .contains(&ElevatorCall::TweakApply(TWEAK.to_owned()))
    );
    // The app's own journal knows nothing about it: the helper holds the previous value.
    assert!(
        system
            .journal
            .entries_in_state(volt_core::EntryState::Applied)
            .expect("entries")
            .is_empty()
    );

    let reverted = revert_tweak(&core, TWEAK).expect("answered");
    assert_eq!(reverted.status, RevertStatus::Reverted);
    assert!(
        elevator
            .calls()
            .contains(&ElevatorCall::TweakRevert(TWEAK.to_owned()))
    );
}

#[test]
fn without_an_elevator_or_when_unavailable_the_ordinary_refusal_stands() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let none = core_with(&system, &dir, None);
    assert!(matches!(
        apply_tweak(&none, TWEAK),
        Err(IpcError::AccessDenied { .. })
    ));

    let elevator = Arc::new(FakeElevator::unavailable_because(
        system.memory.clone(),
        UnavailableReason::NotAllUsersInstall,
    ));
    let core = core_with(&system, &dir, Some(&elevator));
    assert!(matches!(
        apply_tweak(&core, TWEAK),
        Err(IpcError::AccessDenied { .. })
    ));
    assert_eq!(elevator.prompts(), 0);
    assert!(
        !elevator
            .calls()
            .iter()
            .any(|call| matches!(call, ElevatorCall::TweakApply(_)))
    );
}

#[test]
fn a_refusal_of_the_helper_is_a_result_that_says_nothing_was_changed() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(&elevator));
    for (error, apply, revert) in [
        (
            ElevationError::Declined,
            ApplyStatus::Declined,
            RevertStatus::Declined,
        ),
        (
            ElevationError::Refused(ErrorCode::NotConfirmed),
            ApplyStatus::NotConfirmed,
            RevertStatus::NotConfirmed,
        ),
        (
            ElevationError::Refused(ErrorCode::Blocked),
            ApplyStatus::Blocked,
            RevertStatus::Blocked,
        ),
    ] {
        elevator.set_tweak_error(Some(error));
        assert_eq!(apply_tweak(&core, TWEAK).expect("answered").status, apply);
        assert_eq!(revert_tweak(&core, TWEAK).expect("answered").status, revert);
    }
    elevator.set_tweak_error(Some(ElevationError::Refused(ErrorCode::RestorePointFailed)));
    assert_eq!(
        apply_tweak(&core, TWEAK).expect("answered").status,
        ApplyStatus::RestorePointFailed
    );
    elevator.set_tweak_error(Some(ElevationError::Refused(ErrorCode::NothingToRevert)));
    assert_eq!(
        revert_tweak(&core, TWEAK).expect("answered").status,
        RevertStatus::NothingToRevert
    );
    // A helper that cannot carry the change out is an error the UI words itself.
    elevator.set_tweak_error(Some(ElevationError::Refused(ErrorCode::TweakFailed)));
    assert!(matches!(
        apply_tweak(&core, TWEAK),
        Err(IpcError::Other { .. })
    ));
}

#[test]
fn the_helpers_rows_are_mirrored_by_pull_and_shown_with_the_administrator_mark() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    elevator.set_history(
        "store-1",
        vec![
            record(1, ActivityOutcome::Applied, TWEAK),
            record(2, ActivityOutcome::Failed, TWEAK),
        ],
    );
    let core = core_with(&system, &dir, Some(&elevator));

    // No session: looking at the history never starts the helper.
    let page = list_activity(&core, 50, None).expect("page");
    assert!(page.items.is_empty());
    assert_eq!(elevator.prompts(), 0);

    // A change opens the session and pulls.
    apply_tweak(&core, TWEAK).expect("answered");
    let page = list_activity(&core, 50, None).expect("page");
    assert_eq!(page.items.len(), 2);
    for item in &page.items {
        assert!(item.administrator);
        assert_eq!(item.administrator_tweak.as_deref(), Some(TWEAK));
        // The helper's own batch and entry ids are never shown as ids of the app's journal.
        assert!(item.batch.is_none() && item.entry.is_none());
        assert!(item.id.starts_with("admin-"));
    }
    let applied = page
        .items
        .iter()
        .find(|item| item.state == ChangeState::CanUndo)
        .expect("the apply can be undone through the helper");
    assert!(applied.summary_phrase.is_some());

    // The copy is saved: a new core over the same store still has the rows, read-only without a
    // helper (here: not installed for all users), with the reason.
    let unavailable = Arc::new(FakeElevator::unavailable_because(
        system.memory.clone(),
        UnavailableReason::NotAllUsersInstall,
    ));
    let again = core_with(&system, &dir, Some(&unavailable));
    let page = list_activity(&again, 50, None).expect("page");
    assert_eq!(page.items.len(), 2);
    let locked = page
        .items
        .iter()
        .find(|item| item.status == volt_desktop_lib::api::ActivityStatus::Applied)
        .expect("the apply row");
    assert_eq!(locked.state, ChangeState::CannotUndo);
    assert_eq!(
        locked
            .reason_phrase
            .as_ref()
            .map(|phrase| phrase.code.as_str()),
        Some("undo.adminNotAllUsers")
    );
    assert!(unavailable.calls().iter().all(|call| !matches!(
        call,
        ElevatorCall::TweakApply(_) | ElevatorCall::TweakRevert(_) | ElevatorCall::Hello
    )));
}

#[test]
fn an_administrator_row_is_undone_through_the_helper() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    let core = core_with(&system, &dir, Some(&elevator));
    let undone = administrator::undo_change(&core, TWEAK).expect("answered");
    assert_eq!(undone.status, UndoStatus::Restored);
    assert_eq!(undone.restored, 1);
    assert!(
        elevator
            .calls()
            .contains(&ElevatorCall::TweakRevert(TWEAK.to_owned()))
    );

    elevator.set_tweak_error(Some(ElevationError::Refused(ErrorCode::NothingToRevert)));
    assert_eq!(
        administrator::undo_change(&core, TWEAK)
            .expect("answered")
            .status,
        UndoStatus::NothingToUndo
    );
    elevator.set_tweak_error(Some(ElevationError::Refused(ErrorCode::NotConfirmed)));
    assert_eq!(
        administrator::undo_change(&core, TWEAK)
            .expect("answered")
            .status,
        UndoStatus::NotConfirmed
    );
}

#[test]
fn a_tweak_the_helper_applied_shows_as_applied_by_volt_and_clearing_keeps_what_can_be_undone() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = granted(&system);
    elevator.set_history(
        "store-1",
        vec![
            record(1, ActivityOutcome::Applied, TWEAK),
            record(2, ActivityOutcome::Reverted, TWEAK),
            record(3, ActivityOutcome::Failed, "turn-off-activity-history"),
            record(4, ActivityOutcome::Applied, "turn-off-fast-startup"),
        ],
    );
    let core = core_with(&system, &dir, Some(&elevator));
    apply_tweak(&core, TWEAK).expect("answered");

    let tweaks = list_tweaks(&core).expect("tweaks");
    let by = |id: &str| tweaks.iter().find(|tweak| tweak.id == id).expect("tweak");
    assert!(by("turn-off-fast-startup").applied_by_volt);
    assert!(!by(TWEAK).applied_by_volt, "its apply was reverted");

    let cleared = clear_history(&core).expect("cleared");
    assert!(cleared.removed >= 3, "{cleared:?}");
    let page = list_activity(&core, 50, None).expect("page");
    let left: Vec<_> = page.items.iter().map(|item| item.administrator).collect();
    assert_eq!(
        left,
        vec![true],
        "only the apply that can still be undone stays"
    );
    // The cursor stays: nothing that was cleared comes back at the next pull.
    apply_tweak(&core, TWEAK).expect("answered");
    assert_eq!(list_activity(&core, 50, None).expect("page").items.len(), 1);
}

#[test]
fn a_saved_copy_that_cannot_be_read_is_an_empty_one_and_never_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let core = core_with(&system, &dir, None);
    core.settings
        .set(MIRROR_SETTINGS_KEY, &serde_json::json!("not a mirror"))
        .expect("stored");
    assert!(
        list_activity(&core, 50, None)
            .expect("page")
            .items
            .is_empty()
    );
}
