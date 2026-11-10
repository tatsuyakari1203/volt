//! Executor, plan and journal behaviour against the in-memory fakes.

use super::*;
use crate::error::{PlanError, PortError};
use crate::fakes::{Event, FakeSystem};
use crate::journal::JournalEntry;
use crate::label::{Evidence, Risk};
use crate::operation::{Operation, PriorState};
use crate::plan::{OperationRequest, Plan};
use crate::ports::Services;
use crate::registry::{Registry, RegistryHive, RegistryValue, RegistryValueRef, RegistryView};
use crate::service::ServiceStartType;
use crate::time::Timestamp;

fn value_ref(name: &str) -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::LocalMachine,
        r"SOFTWARE\VoltTest\Tweaks",
        name,
    )
}

fn set_dword(name: &str, value: u32) -> Operation {
    Operation::SetRegistryValue {
        target: value_ref(name),
        value: RegistryValue::Dword(value),
    }
}

fn set_service(service: &str, start_type: ServiceStartType) -> Operation {
    Operation::SetServiceStartType {
        service: service.to_owned(),
        start_type,
    }
}

fn request(operation: Operation, risk: Risk) -> OperationRequest {
    OperationRequest::new(operation, risk, Evidence::Measured)
}

fn plan_of(system: &FakeSystem, requests: Vec<OperationRequest>) -> Plan {
    Plan::build(requests, &system.ports()).expect("plan builds against the fakes")
}

fn entry_state(system: &FakeSystem, id: EntryId) -> EntryState {
    system
        .journal
        .entry(id)
        .expect("journal reads")
        .expect("entry exists")
        .state
}

fn applied_entry(report: &ApplyReport, index: usize) -> EntryId {
    match report.outcomes[index].status {
        ItemStatus::Applied { entry } => entry,
        ref other => panic!("item {index} should be applied, was {other:?}"),
    }
}

// ---- crash reconciliation ---------------------------------------------------------------

/// Writes a pending entry the way the executor would, then "crashes" (does nothing else).
fn crash_after_journal(system: &FakeSystem, operation: Operation) -> EntryId {
    let prior = operation.detect(&system.ports()).expect("detect");
    let batch = system.journal.new_batch(None).expect("batch");
    system
        .journal
        .record_pending(NewEntry {
            batch,
            timestamp: Timestamp::from_unix_millis(1),
            source: Source::Manual,
            operation,
            prior,
        })
        .expect("record")
}

#[test]
fn reconcile_marks_never_applied_entries_failed() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    let id = crash_after_journal(&system, set_dword("A", 0));

    let report = system.executor().reconcile_pending().expect("reconcile");
    assert_eq!(
        report.outcomes,
        [ReconcileOutcome {
            entry: id,
            resolution: Resolution::NeverApplied
        }]
    );
    let entry = system.journal.entry(id).expect("read").expect("exists");
    assert_eq!(entry.state, EntryState::Failed);
    assert!(entry.error.is_some());
    assert_eq!(
        system.registry.get(&value_ref("A")),
        Some(RegistryValue::Dword(1))
    );
}

#[test]
fn reconcile_marks_applied_entries_applied_so_they_can_be_undone() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    let id = crash_after_journal(&system, set_dword("A", 0));
    // The apply happened, then the process died before the state update.
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(0));

    let executor = system.executor();
    let report = executor.reconcile_pending().expect("reconcile");
    assert_eq!(report.outcomes[0].resolution, Resolution::ConfirmedApplied);
    assert_eq!(entry_state(&system, id), EntryState::Applied);

    executor
        .revert_entry(id, Source::Manual, None)
        .expect("revert");
    assert_eq!(
        system.registry.get(&value_ref("A")),
        Some(RegistryValue::Dword(1))
    );
}

#[test]
fn reconcile_marks_unexpected_state_failed_and_unknown() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    let id = crash_after_journal(&system, set_dword("A", 0));
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(7));

    let report = system.executor().reconcile_pending().expect("reconcile");
    assert_eq!(report.outcomes[0].resolution, Resolution::StateUnknown);
    let entry = system.journal.entry(id).expect("read").expect("exists");
    assert_eq!(entry.state, EntryState::Failed);
    assert!(entry.error.expect("reason").contains("unknown"));
}

#[test]
fn reconcile_leaves_entries_pending_when_state_cannot_be_read() {
    let system = FakeSystem::new();
    system
        .services
        .add("DiagTrack", ServiceStartType::Automatic);
    let id = crash_after_journal(
        &system,
        set_service("DiagTrack", ServiceStartType::Disabled),
    );
    // The service vanished, so its state cannot be detected.
    let without_service = FakeSystem::new();
    let executor = Executor::new(
        without_service.ports(),
        system.journal.clone(),
        system.clock.clone(),
    );
    let report = executor.reconcile_pending().expect("reconcile");
    assert!(matches!(
        report.outcomes[0].resolution,
        Resolution::Undetermined { .. }
    ));
    assert_eq!(entry_state(&system, id), EntryState::Pending);
}

#[test]
fn reconcile_ignores_entries_that_are_not_pending() {
    let system = FakeSystem::new();
    let plan = plan_of(&system, vec![request(set_dword("A", 1), Risk::Safe)]);
    let executor = system.executor();
    executor.apply(&plan, Source::Manual).expect("apply");
    let report = executor.reconcile_pending().expect("reconcile");
    assert!(report.outcomes.is_empty());
}

// ---- plan and preview -------------------------------------------------------------------

#[test]
fn plan_previews_current_and_target_and_takes_the_maximum_risk() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Dword(1));
    system
        .services
        .add("DiagTrack", ServiceStartType::Automatic);

    let plan = plan_of(
        &system,
        vec![
            request(set_dword("Existing", 0), Risk::Safe),
            request(set_dword("Missing", 5), Risk::Caution),
            request(
                Operation::DeleteRegistryValue {
                    target: value_ref("Existing"),
                },
                Risk::Safe,
            ),
            request(
                set_service("DiagTrack", ServiceStartType::Disabled),
                Risk::Advanced,
            ),
        ],
    );

    let previews: Vec<String> = plan.items.iter().map(|i| i.preview.to_string()).collect();
    assert_eq!(
        previews,
        [
            r"HKLM\SOFTWARE\VoltTest\Tweaks\Existing: REG_DWORD 1 -> REG_DWORD 0",
            r"HKLM\SOFTWARE\VoltTest\Tweaks\Missing: (not set) -> REG_DWORD 5",
            r"HKLM\SOFTWARE\VoltTest\Tweaks\Existing: REG_DWORD 1 -> (not set)",
            "service DiagTrack start type: Automatic -> Disabled",
        ]
    );
    assert_eq!(plan.risk(), Risk::Advanced);
    assert_eq!(plan.items[1].evidence, Evidence::Measured);
}

#[test]
fn empty_plan_is_safe_and_building_a_plan_changes_nothing() {
    let system = FakeSystem::new();
    system.services.add("Svc", ServiceStartType::Manual);
    assert_eq!(plan_of(&system, vec![]).risk(), Risk::Safe);

    plan_of(
        &system,
        vec![
            request(set_dword("A", 1), Risk::Safe),
            request(set_service("Svc", ServiceStartType::Disabled), Risk::Safe),
        ],
    );
    assert!(system.log.events().is_empty(), "planning is read-only");
    assert!(
        system.journal.entries().is_empty(),
        "planning writes no journal entry"
    );
}

#[test]
fn plan_build_reports_which_operation_could_not_be_read() {
    let system = FakeSystem::new();
    let err = Plan::build(
        vec![
            request(set_dword("A", 1), Risk::Safe),
            request(
                set_service("NoSuchService", ServiceStartType::Disabled),
                Risk::Safe,
            ),
        ],
        &system.ports(),
    )
    .expect_err("unknown service cannot be previewed");
    assert!(matches!(
        err,
        PlanError::Detect {
            index: 1,
            error: PortError::NotFound(_)
        }
    ));
}

// ---- apply ------------------------------------------------------------------------------

#[test]
fn apply_changes_the_system_and_records_applied_entries_with_prior_state() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Dword(1));
    system
        .services
        .add("DiagTrack", ServiceStartType::Automatic);
    let plan = plan_of(
        &system,
        vec![
            request(set_dword("Existing", 0), Risk::Safe),
            request(
                set_service("DiagTrack", ServiceStartType::Disabled),
                Risk::Caution,
            ),
        ],
    );

    let report = system
        .executor()
        .apply(&plan, Source::Automation)
        .expect("batch is created");

    assert!(report.is_success());
    assert_eq!(report.applied().count(), 2);
    assert_eq!(
        system.registry.get(&value_ref("Existing")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(
        system.services.start_type("DiagTrack"),
        Ok(ServiceStartType::Disabled)
    );

    let entries = system.journal.entries();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.batch == report.batch));
    assert!(entries.iter().all(|e| e.state == EntryState::Applied));
    assert!(entries.iter().all(|e| e.source == Source::Automation));
    assert_eq!(
        entries[0].prior,
        PriorState::RegistryValue(Some(RegistryValue::Dword(1)))
    );
    assert_eq!(
        entries[1].prior,
        PriorState::ServiceStartType(ServiceStartType::Automatic)
    );
    // The injected clock makes timestamps deterministic.
    assert_eq!(entries[0].timestamp, Timestamp::from_unix_millis(1000));
    assert_eq!(entries[1].timestamp, Timestamp::from_unix_millis(1001));
}

#[test]
fn journal_pending_entry_is_written_before_the_system_is_mutated() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Dword(1));
    let plan = plan_of(&system, vec![request(set_dword("Existing", 0), Risk::Safe)]);

    let report = system
        .executor()
        .apply(&plan, Source::Manual)
        .expect("batch is created");
    let id = applied_entry(&report, 0);

    assert_eq!(
        system.log.events(),
        [
            Event::JournalPending(id),
            Event::RegistryWrite(value_ref("Existing")),
            Event::JournalState(id, EntryState::Applied),
        ]
    );
}

#[test]
fn apply_detects_again_so_a_stale_preview_records_the_real_prior_state() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Drift"), RegistryValue::Dword(1));
    let plan = plan_of(&system, vec![request(set_dword("Drift", 0), Risk::Safe)]);
    // The value changes between preview and apply.
    system
        .registry
        .set(&value_ref("Drift"), RegistryValue::Dword(7));

    system
        .executor()
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert_eq!(
        system.journal.entries()[0].prior,
        PriorState::RegistryValue(Some(RegistryValue::Dword(7)))
    );
}

#[test]
fn when_the_journal_refuses_the_pending_entry_nothing_is_mutated() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Dword(1));
    let plan = plan_of(&system, vec![request(set_dword("Existing", 0), Risk::Safe)]);
    system
        .journal
        .fail_next_record(PortError::Other("disk full".to_owned()));

    let report = system
        .executor()
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert!(matches!(
        &report.outcomes[0].status,
        ItemStatus::Failed {
            entry: None,
            error: ExecError::Journal(_)
        }
    ));
    assert_eq!(
        system.registry.get(&value_ref("Existing")),
        Some(RegistryValue::Dword(1))
    );
    assert!(
        system.log.events().is_empty(),
        "no mutation and no journal write happened"
    );
}

#[test]
fn failure_in_the_middle_stops_and_reports_applied_failed_and_not_attempted() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("First"), RegistryValue::Dword(1));
    system.services.add("Locked", ServiceStartType::Automatic);
    system.services.fail_on(
        "Locked",
        PortError::AccessDenied("service control".to_owned()),
    );
    let plan = plan_of(
        &system,
        vec![
            request(set_dword("First", 0), Risk::Safe),
            request(
                set_service("Locked", ServiceStartType::Disabled),
                Risk::Caution,
            ),
            request(set_dword("Third", 9), Risk::Safe),
        ],
    );

    let report = system
        .executor()
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert!(!report.is_success());
    assert_eq!(report.applied().map(|o| o.index).collect::<Vec<_>>(), [0]);
    assert_eq!(report.failed().map(|o| o.index).collect::<Vec<_>>(), [1]);
    assert_eq!(
        report.not_attempted().map(|o| o.index).collect::<Vec<_>>(),
        [2]
    );
    let ItemStatus::Failed {
        entry: Some(failed_entry),
        error,
    } = &report.outcomes[1].status
    else {
        panic!("item 1 should have failed with a journal entry");
    };
    assert!(matches!(
        error,
        ExecError::Apply {
            error: PortError::AccessDenied(_),
            journal: None
        }
    ));
    assert!(!error.system_changed());

    // Effects match the report.
    assert_eq!(
        system.registry.get(&value_ref("First")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(
        system.services.start_type("Locked"),
        Ok(ServiceStartType::Automatic)
    );
    assert_eq!(system.registry.get(&value_ref("Third")), None);
    // The journal tells the same story.
    let failed: JournalEntry = system
        .journal
        .entry(*failed_entry)
        .expect("reads")
        .expect("exists");
    assert_eq!(failed.state, EntryState::Failed);
    assert!(
        failed
            .error
            .as_deref()
            .is_some_and(|m| m.contains("access denied"))
    );
    assert_eq!(
        system.journal.entries().len(),
        2,
        "the third item never reached the journal"
    );
}

#[test]
fn a_failure_to_mark_failed_is_reported_and_the_entry_stays_pending() {
    let system = FakeSystem::new();
    system.services.add("Locked", ServiceStartType::Automatic);
    system
        .services
        .fail_on("Locked", PortError::AccessDenied("no".to_owned()));
    system
        .journal
        .fail_next_set_state(EntryState::Failed, PortError::Other("io".to_owned()));
    let plan = plan_of(
        &system,
        vec![request(
            set_service("Locked", ServiceStartType::Disabled),
            Risk::Safe,
        )],
    );

    let report = system
        .executor()
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert!(matches!(
        &report.outcomes[0].status,
        ItemStatus::Failed {
            error: ExecError::Apply {
                journal: Some(_),
                ..
            },
            ..
        }
    ));
    assert_eq!(system.journal.entries()[0].state, EntryState::Pending);
}

#[test]
fn a_change_that_could_not_be_marked_applied_is_flagged_as_changed_and_still_rolled_back() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Dword(1));
    system
        .journal
        .fail_next_set_state(EntryState::Applied, PortError::Other("io".to_owned()));
    let plan = plan_of(&system, vec![request(set_dword("Existing", 0), Risk::Safe)]);

    let report = system
        .executor()
        .apply_or_rollback(&plan, Source::Manual)
        .expect("batch is created");

    let ItemStatus::Failed { error, .. } = &report.outcomes[0].status else {
        panic!("the item is reported as failed");
    };
    assert!(matches!(error, ExecError::MarkApplied(_)));
    assert!(error.system_changed(), "the registry WAS written");
    // The pending entry carried the prior state, so rollback could restore it.
    assert_eq!(
        system.registry.get(&value_ref("Existing")),
        Some(RegistryValue::Dword(1))
    );
    assert!(
        report
            .rollback
            .as_ref()
            .is_some_and(RevertReport::is_complete)
    );
}

// ---- rollback and revert ----------------------------------------------------------------

#[test]
fn apply_or_rollback_restores_the_items_already_applied() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Existing"), RegistryValue::Sz("keep".to_owned()));
    system.services.add("Locked", ServiceStartType::Manual);
    system
        .services
        .fail_on("Locked", PortError::Other("boom".to_owned()));
    let plan = plan_of(
        &system,
        vec![
            request(set_dword("Fresh", 1), Risk::Safe),
            request(
                Operation::SetRegistryValue {
                    target: value_ref("Existing"),
                    value: RegistryValue::Sz("changed".to_owned()),
                },
                Risk::Safe,
            ),
            request(
                set_service("Locked", ServiceStartType::Disabled),
                Risk::Safe,
            ),
        ],
    );

    let report = system
        .executor()
        .apply_or_rollback(&plan, Source::Manual)
        .expect("batch is created");

    assert_eq!(
        report.applied().count(),
        2,
        "the report still says what was applied"
    );
    let rollback = report.rollback.expect("rollback ran");
    assert!(rollback.is_complete());
    // Newest first.
    assert_eq!(
        rollback
            .outcomes
            .iter()
            .map(|o| o.entry)
            .collect::<Vec<_>>(),
        [EntryId(2), EntryId(1)]
    );
    assert_eq!(system.registry.get(&value_ref("Fresh")), None);
    assert_eq!(
        system.registry.get(&value_ref("Existing")),
        Some(RegistryValue::Sz("keep".to_owned()))
    );
    assert_eq!(entry_state(&system, EntryId(1)), EntryState::Reverted);
    assert_eq!(entry_state(&system, EntryId(2)), EntryState::Reverted);
    assert_eq!(entry_state(&system, EntryId(3)), EntryState::Failed);
}

#[test]
fn reverting_a_value_that_did_not_exist_deletes_it() {
    let system = FakeSystem::new();
    let plan = plan_of(&system, vec![request(set_dword("New", 1), Risk::Safe)]);
    let executor = system.executor();
    let report = executor
        .apply(&plan, Source::Manual)
        .expect("batch is created");
    let id = applied_entry(&report, 0);
    // Neither the value nor its key existed, so the journal says the apply created the keys.
    assert_eq!(
        system.journal.entries()[0].prior,
        PriorState::RegistryValueInNewKey { missing_levels: 3 }
    );
    assert_eq!(
        system.registry.get(&value_ref("New")),
        Some(RegistryValue::Dword(1))
    );

    executor
        .revert_entry(id, Source::Manual, None)
        .expect("revert succeeds");
    // The keys the apply created are gone again, not left behind empty.
    for subkey in [
        r"SOFTWARE\VoltTest\Tweaks",
        r"SOFTWARE\VoltTest",
        "SOFTWARE",
    ] {
        assert_eq!(
            system
                .registry
                .key_exists(RegistryHive::LocalMachine, subkey, RegistryView::Native),
            Ok(false),
            "{subkey}"
        );
    }

    assert_eq!(
        system.registry.get(&value_ref("New")),
        None,
        "back to 'did not exist'"
    );
    assert!(
        system
            .log
            .events()
            .contains(&Event::RegistryDelete(value_ref("New")))
    );
    assert_eq!(entry_state(&system, id), EntryState::Reverted);
}

#[test]
fn reverting_a_delete_restores_the_exact_previous_value_and_type() {
    let system = FakeSystem::new();
    let original = RegistryValue::ExpandSz(r"%SystemRoot%\system32".to_owned());
    system.registry.set(&value_ref("Path"), original.clone());
    let plan = plan_of(
        &system,
        vec![request(
            Operation::DeleteRegistryValue {
                target: value_ref("Path"),
            },
            Risk::Caution,
        )],
    );
    let executor = system.executor();
    let report = executor
        .apply(&plan, Source::Manual)
        .expect("batch is created");
    assert_eq!(system.registry.get(&value_ref("Path")), None);

    executor
        .revert_entry(applied_entry(&report, 0), Source::Manual, None)
        .expect("revert succeeds");

    assert_eq!(system.registry.get(&value_ref("Path")), Some(original));
}

#[test]
fn deleting_an_already_absent_value_is_a_journaled_no_op_that_reverts_cleanly() {
    let system = FakeSystem::new();
    let plan = plan_of(
        &system,
        vec![request(
            Operation::DeleteRegistryValue {
                target: value_ref("Gone"),
            },
            Risk::Safe,
        )],
    );
    let executor = system.executor();

    let report = executor
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert!(report.is_success());
    executor
        .revert_entry(applied_entry(&report, 0), Source::Manual, None)
        .expect("revert succeeds");
    assert_eq!(system.registry.get(&value_ref("Gone")), None);
}

#[test]
fn revert_batch_undoes_everything_newest_first_and_cannot_run_twice() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    system.services.add("Svc", ServiceStartType::Automatic);
    let plan = plan_of(
        &system,
        vec![
            request(set_dword("A", 2), Risk::Safe),
            request(set_service("Svc", ServiceStartType::Manual), Risk::Safe),
            request(set_dword("B", 3), Risk::Safe),
        ],
    );
    let executor = system.executor();
    let report = executor
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    let reverted = executor
        .revert_batch(report.batch, Source::Manual, None)
        .expect("journal reads");

    assert!(reverted.is_complete());
    assert_eq!(
        reverted
            .outcomes
            .iter()
            .map(|o| o.entry)
            .collect::<Vec<_>>(),
        [EntryId(3), EntryId(2), EntryId(1)]
    );
    assert_eq!(
        system.registry.get(&value_ref("A")),
        Some(RegistryValue::Dword(1))
    );
    assert_eq!(system.registry.get(&value_ref("B")), None);
    assert_eq!(
        system.services.start_type("Svc"),
        Ok(ServiceStartType::Automatic)
    );

    assert_eq!(
        executor.revert_entry(EntryId(1), Source::Manual, None),
        Err(RevertError::NotRevertible(EntryState::Reverted))
    );
    assert!(
        executor
            .revert_batch(report.batch, Source::Manual, None)
            .expect("journal reads")
            .outcomes
            .is_empty()
    );
    assert_eq!(
        executor.revert_entry(EntryId(99), Source::Manual, None),
        Err(RevertError::EntryNotFound(EntryId(99)))
    );
}

#[test]
fn a_failed_revert_leaves_the_entry_applied_so_it_can_be_retried() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    let plan = plan_of(&system, vec![request(set_dword("A", 2), Risk::Safe)]);
    let executor = system.executor();
    let id = applied_entry(&executor.apply(&plan, Source::Manual).expect("batch"), 0);
    system.registry.fail_on(
        &value_ref("A"),
        PortError::AccessDenied("locked".to_owned()),
    );

    let result = executor.revert_entry(id, Source::Manual, None);

    assert!(matches!(
        result,
        Err(RevertError::Port(PortError::AccessDenied(_)))
    ));
    assert_eq!(entry_state(&system, id), EntryState::Applied);
}

#[test]
fn a_pending_entry_cannot_be_reverted_through_the_public_api() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    system
        .journal
        .fail_next_set_state(EntryState::Applied, PortError::Other("io".to_owned()));
    let plan = plan_of(&system, vec![request(set_dword("A", 2), Risk::Safe)]);
    let executor = system.executor();
    executor
        .apply(&plan, Source::Manual)
        .expect("batch is created");

    assert_eq!(
        executor.revert_entry(EntryId(1), Source::Manual, None),
        Err(RevertError::NotRevertible(EntryState::Pending))
    );
}

// ---- data types -------------------------------------------------------------------------

#[test]
fn plans_reports_and_journal_entries_round_trip_through_json() {
    let system = FakeSystem::new();
    system.registry.set(
        &value_ref("A"),
        RegistryValue::MultiSz(vec!["x".to_owned(), "y".to_owned()]),
    );
    system
        .services
        .add("Svc", ServiceStartType::AutomaticDelayed);
    let plan = plan_of(
        &system,
        vec![
            request(set_dword("A", 2), Risk::Safe),
            request(
                Operation::DeleteRegistryValue {
                    target: value_ref("B"),
                },
                Risk::Advanced,
            ),
            request(
                set_service("Svc", ServiceStartType::Disabled),
                Risk::Caution,
            ),
        ],
    );
    let report = system
        .executor()
        .apply(&plan, Source::Automation)
        .expect("batch is created");
    let entries = system.journal.entries();

    let plan_json = serde_json::to_string(&plan).expect("plan serializes");
    assert_eq!(
        serde_json::from_str::<Plan>(&plan_json).expect("plan parses"),
        plan
    );
    let report_json = serde_json::to_string(&report).expect("report serializes");
    assert_eq!(
        serde_json::from_str::<ApplyReport>(&report_json).expect("report parses"),
        report
    );
    let entries_json = serde_json::to_string(&entries).expect("entries serialize");
    assert_eq!(
        serde_json::from_str::<Vec<JournalEntry>>(&entries_json).expect("entries parse"),
        entries
    );
    // The wire format is tagged and readable.
    assert!(plan_json.contains(r#""kind":"set_registry_value""#));
}

#[test]
fn failed_reports_with_typed_errors_round_trip_through_json() {
    let report = ApplyReport {
        batch: BatchId(4),
        outcomes: vec![
            ItemOutcome {
                index: 0,
                status: ItemStatus::Applied { entry: EntryId(1) },
            },
            ItemOutcome {
                index: 1,
                status: ItemStatus::Failed {
                    entry: Some(EntryId(2)),
                    error: ExecError::Apply {
                        error: PortError::AccessDenied("x".to_owned()),
                        journal: None,
                    },
                },
            },
            ItemOutcome {
                index: 2,
                status: ItemStatus::NotAttempted,
            },
        ],
        rollback: Some(RevertReport {
            outcomes: vec![RevertOutcome {
                entry: EntryId(1),
                result: Err(RevertError::NotRevertible(EntryState::Reverted)),
            }],
        }),
    };
    let json = serde_json::to_string(&report).expect("serializes");
    assert_eq!(
        serde_json::from_str::<ApplyReport>(&json).expect("parses"),
        report
    );
}
