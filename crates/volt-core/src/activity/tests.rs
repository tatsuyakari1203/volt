//! Activity log behaviour against the in-memory fakes.

use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use super::*;
use crate::command::{Command, CommandOutput, Commands};
use crate::executor::{Executor, ItemStatus, Resolution};
use crate::fakes::{FakeActivityLog, FakeSystem};
use crate::journal::{EntryState, Journal, NewEntry};
use crate::label::{Evidence, Risk};
use crate::plan::{OperationRequest, Plan};
use crate::ports::Ports;
use crate::registry::{RegistryHive, RegistryValueRef};
use crate::service::ServiceStartType;

/// A string that must never show up in any summary, export or log line.
const SECRET: &str = "hunter2-SECRET-9f3a";

// ---- a capturing logger ----------------------------------------------------------------

struct CaptureLogger;

fn captured() -> &'static Mutex<Vec<String>> {
    static LINES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
    LINES.get_or_init(|| Mutex::new(Vec::new()))
}

impl log::Log for CaptureLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        captured()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}", record.level(), record.args()));
    }

    fn flush(&self) {}
}

fn install_logger() {
    static LOGGER: CaptureLogger = CaptureLogger;
    // Another test may have installed it already; that is fine.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
}

fn captured_lines() -> Vec<String> {
    captured()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

// ---- helpers ---------------------------------------------------------------------------------

fn value_ref(name: &str) -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::CurrentUser,
        r"Software\VoltTest\Activity",
        name,
    )
}

fn set_dword(name: &str, value: u32) -> Operation {
    Operation::SetRegistryValue {
        target: value_ref(name),
        value: RegistryValue::Dword(value),
    }
}

fn plan_of(system: &FakeSystem, operations: Vec<Operation>) -> Plan {
    plan_with(&system.ports(), operations)
}

fn plan_with(ports: &Ports, operations: Vec<Operation>) -> Plan {
    let requests = operations
        .into_iter()
        .map(|operation| OperationRequest::new(operation, Risk::Safe, Evidence::Measured))
        .collect();
    Plan::build(requests, ports).expect("plan builds against the fakes")
}

fn record(id: u64, millis: i64, outcome: ActivityOutcome, summary: &str) -> ActivityRecord {
    ActivityRecord {
        id: ActivityId(id),
        timestamp: Timestamp::from_unix_millis(millis),
        source: Source::Manual,
        outcome,
        summary: summary.to_owned(),
        batch: None,
        entry: None,
        label: None,
    }
}

fn new_activity(summary: &str) -> NewActivity {
    NewActivity {
        timestamp: Timestamp::from_unix_millis(5),
        source: Source::Manual,
        outcome: ActivityOutcome::Applied,
        summary: summary.to_owned(),
        batch: None,
        entry: None,
        label: None,
    }
}

// ---- export as text ----------------------------------------------------------------------

#[test]
fn export_renders_stable_lines_with_utc_time_source_outcome_and_summary() {
    let mut linked = record(
        2,
        1_700_000_000_123,
        ActivityOutcome::Failed,
        "Failed to apply service Spooler start type: the change failed (access denied)",
    );
    linked.source = Source::Automation;
    linked.batch = Some(BatchId(3));
    linked.entry = Some(EntryId(7));
    linked.label = Some("tweak.example".to_owned());
    let records = vec![
        linked,
        record(1, 0, ActivityOutcome::RolledBack, "two\nlines\tand a tab"),
    ];

    let expected = "\
2023-11-14T22:13:20.123Z automation failed      Failed to apply service Spooler start type: the change failed (access denied) [label tweak.example, batch 3, entry 7]
1970-01-01T00:00:00.000Z manual     rolled back two lines and a tab
";
    assert_eq!(render_text(&records), expected);
    // Stable: rendering again gives the same text.
    assert_eq!(render_text(&records), expected);
    assert_eq!(render_text(&[]), "");
}

// ---- reading -----------------------------------------------------------------------------

#[test]
fn list_is_newest_first_with_a_limit_and_a_cursor() {
    let log = FakeActivityLog::new();
    for n in 1..=5 {
        log.record(new_activity(&format!("event {n}")))
            .expect("record");
    }
    let summaries = |records: Vec<ActivityRecord>| -> Vec<String> {
        records.into_iter().map(|r| r.summary).collect()
    };

    let first = log.list(2, None).expect("first page");
    assert_eq!(summaries(first.clone()), ["event 5", "event 4"]);
    let cursor = first.last().map(|r| r.id);
    let second = log.list(2, cursor).expect("second page");
    assert_eq!(summaries(second.clone()), ["event 3", "event 2"]);
    let third = log
        .list(2, second.last().map(|r| r.id))
        .expect("third page");
    assert_eq!(summaries(third), ["event 1"]);
    assert!(log.list(0, None).expect("zero").is_empty());
}

// ---- the executor writes records -------------------------------------------------------------

#[test]
fn applying_a_plan_records_every_operation_with_links_and_the_label() {
    let system = FakeSystem::new();
    system.services.add("Spooler", ServiceStartType::Automatic);
    let plan = plan_of(
        &system,
        vec![
            set_dword("Flag", 1),
            Operation::SetServiceStartType {
                service: "Spooler".to_owned(),
                start_type: ServiceStartType::Disabled,
            },
        ],
    )
    .with_label("tweak.example");

    let report = system
        .executor_with_activity()
        .apply(&plan, Source::Automation)
        .expect("apply");
    assert!(report.is_success());

    let records = system.activity.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert_eq!(
        records[0].summary,
        r"Applied registry value HKCU\Software\VoltTest\Activity\Flag = REG_DWORD 1"
    );
    assert_eq!(
        records[1].summary,
        "Applied service Spooler start type = Disabled"
    );
    for (record, entry) in records.iter().zip(system.journal.entries()) {
        assert_eq!(record.source, Source::Automation);
        assert_eq!(record.batch, Some(report.batch));
        assert_eq!(record.entry, Some(entry.id));
        assert_eq!(record.label.as_deref(), Some("tweak.example"));
    }
    assert!(
        records[0].timestamp < records[1].timestamp,
        "time comes from the clock"
    );
}

#[test]
fn a_failed_step_is_recorded_as_failed_and_the_undone_steps_as_rolled_back() {
    let system = FakeSystem::new();
    system.services.add("Spooler", ServiceStartType::Automatic);
    system
        .services
        .fail_on("Spooler", PortError::AccessDenied("no".to_owned()));
    let plan = plan_of(
        &system,
        vec![
            set_dword("One", 1),
            Operation::SetServiceStartType {
                service: "Spooler".to_owned(),
                start_type: ServiceStartType::Disabled,
            },
            set_dword("Never", 3),
        ],
    )
    .with_label("tweak.example");

    let report = system
        .executor_with_activity()
        .apply_or_rollback(&plan, Source::Manual)
        .expect("apply");
    assert!(!report.is_success());

    let outcomes: Vec<_> = system
        .activity
        .records()
        .iter()
        .map(|r| (r.outcome, r.label.clone()))
        .collect();
    let label = Some("tweak.example".to_owned());
    assert_eq!(
        outcomes,
        [
            (ActivityOutcome::Applied, label.clone()),
            (ActivityOutcome::Failed, label.clone()),
            (ActivityOutcome::RolledBack, label),
        ],
        "nothing is recorded for the step that was never attempted"
    );
    let records = system.activity.records();
    assert_eq!(
        records[1].summary,
        "Failed to apply service Spooler start type: the change failed (access denied)"
    );
    assert_eq!(
        records[2].summary,
        r"Rolled back registry value HKCU\Software\VoltTest\Activity\One, restored (not set)"
    );
}

#[test]
fn reverting_records_the_restore_and_a_failed_revert_is_recorded_as_failed() {
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("Flag"), RegistryValue::Dword(7));
    let executor = system.executor_with_activity();
    let plan = plan_of(&system, vec![set_dword("Flag", 1)]);
    let report = executor.apply(&plan, Source::Manual).expect("apply");
    let entry = report.applied().next().and_then(|o| match o.status {
        ItemStatus::Applied { entry } => Some(entry),
        _ => None,
    });
    let entry = entry.expect("applied entry");

    system.registry.fail_on(
        &value_ref("Flag"),
        PortError::AccessDenied("locked".to_owned()),
    );
    assert!(executor.revert_entry(entry).is_err());
    system.registry.clear_failures();
    executor.revert_entry(entry).expect("revert");

    let records = system.activity.records();
    let outcomes: Vec<_> = records.iter().map(|r| r.outcome).collect();
    assert_eq!(
        outcomes,
        [
            ActivityOutcome::Applied,
            ActivityOutcome::Failed,
            ActivityOutcome::Reverted
        ]
    );
    assert_eq!(
        records[1].summary,
        r"Failed to revert registry value HKCU\Software\VoltTest\Activity\Flag: the previous state could not be restored (access denied)"
    );
    assert_eq!(
        records[2].summary,
        r"Reverted registry value HKCU\Software\VoltTest\Activity\Flag, restored REG_DWORD 7"
    );
    assert_eq!(records[2].entry, Some(entry));
}

#[test]
fn a_single_entry_or_batch_revert_carries_the_label_the_batch_was_applied_with() {
    let system = FakeSystem::new();
    let executor = system.executor_with_activity();
    let plan = plan_of(&system, vec![set_dword("One", 1), set_dword("Two", 2)])
        .with_label("tweak.example");
    let report = executor.apply(&plan, Source::Manual).expect("apply");
    let first = report.applied().next().map(|o| match o.status {
        ItemStatus::Applied { entry } => entry,
        _ => unreachable!("filtered to applied"),
    });

    // One entry, then the rest of the batch: neither call is told the label.
    executor
        .revert_entry(first.expect("entry"))
        .expect("revert");
    let batch = executor.revert_batch(report.batch).expect("revert batch");
    assert!(batch.is_complete());
    assert_eq!(
        batch.outcomes.len(),
        1,
        "the first entry was already reverted"
    );

    let reverts: Vec<_> = system
        .activity
        .records()
        .into_iter()
        .filter(|r| r.outcome == ActivityOutcome::Reverted)
        .map(|r| r.label)
        .collect();
    assert_eq!(reverts.len(), 2);
    assert!(
        reverts
            .iter()
            .all(|label| label.as_deref() == Some("tweak.example")),
        "{reverts:?}"
    );
}

#[test]
fn a_batch_without_a_label_and_an_unknown_batch_have_no_label() {
    let system = FakeSystem::new();
    let plain = system.journal.new_batch(None).expect("batch");
    let named = system.journal.new_batch(Some("tweak.x")).expect("batch");
    assert_eq!(system.journal.batch_label(plain), Ok(None));
    assert_eq!(
        system.journal.batch_label(named),
        Ok(Some("tweak.x".to_owned()))
    );
    assert_eq!(system.journal.batch_label(BatchId(99)), Ok(None));
}

#[test]
fn restore_words_follow_the_redaction_rule() {
    use crate::registry::RegistryValue as V;
    let words = |prior| restore_words(&prior);
    assert_eq!(
        words(PriorState::RegistryValue(Some(V::Dword(5)))),
        "set back to REG_DWORD 5"
    );
    assert_eq!(
        words(PriorState::RegistryValue(None)),
        "removed (it did not exist before)"
    );
    let text = words(PriorState::RegistryValue(Some(V::Sz(SECRET.to_owned()))));
    assert!(!text.contains(SECRET), "{text}");
    assert!(text.contains("REG_SZ"), "{text}");
    assert_eq!(
        words(PriorState::ServiceStartType(ServiceStartType::Manual)),
        format!("set back to {}", ServiceStartType::Manual)
    );
}

#[test]
fn reconcile_pending_records_what_it_decided() {
    let system = FakeSystem::new();
    let operation = set_dword("Flag", 1);
    let prior = PriorState::RegistryValue(None);
    let batch = system.journal.new_batch(None).expect("batch");
    let pending = |system: &FakeSystem| {
        system
            .journal
            .record_pending(NewEntry {
                batch,
                timestamp: Timestamp::from_unix_millis(1),
                source: Source::Automation,
                operation: operation.clone(),
                prior: prior.clone(),
            })
            .expect("pending")
    };

    // The crash came before the write: the system still holds the prior state.
    let never = pending(&system);
    let report = system
        .executor_with_activity()
        .reconcile_pending()
        .expect("reconcile");
    assert_eq!(report.outcomes[0].resolution, Resolution::NeverApplied);
    // The crash came after the write.
    system
        .registry
        .set(&value_ref("Flag"), RegistryValue::Dword(1));
    let done = pending(&system);
    system
        .executor_with_activity()
        .reconcile_pending()
        .expect("reconcile");

    let records = system.activity.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].outcome, ActivityOutcome::Failed);
    assert_eq!(records[0].entry, Some(never));
    assert_eq!(records[0].source, Source::Automation);
    assert_eq!(
        records[0].summary,
        r"Interrupted apply of registry value HKCU\Software\VoltTest\Activity\Flag: the change never happened"
    );
    assert_eq!(records[1].outcome, ActivityOutcome::Applied);
    assert_eq!(records[1].entry, Some(done));
    assert!(
        records[1]
            .summary
            .starts_with("Confirmed after an interruption: Applied ")
    );
}

// ---- an activity log that fails ---------------------------------------------------------------

#[test]
fn a_failing_activity_log_never_blocks_the_change_or_its_journal_entry() {
    install_logger();
    let system = FakeSystem::new();
    system
        .activity
        .fail_records_with(Some(PortError::Other("activity disk is full".to_owned())));
    let executor = system.executor_with_activity();
    assert_eq!(executor.activity_write_failures(), 0);

    let plan = plan_of(&system, vec![set_dword("Flag", 1), set_dword("Other", 2)]);
    let report = executor.apply(&plan, Source::Manual).expect("apply");

    assert!(report.is_success(), "the change still goes through");
    assert_eq!(
        system.registry.get(&value_ref("Flag")),
        Some(RegistryValue::Dword(1))
    );
    assert!(
        system
            .journal
            .entries()
            .iter()
            .all(|e| e.state == EntryState::Applied),
        "the journal is complete"
    );
    assert!(system.activity.records().is_empty());
    // It is surfaced: counted for the shell, and written to the diagnostic log.
    assert_eq!(executor.activity_write_failures(), 2);
    assert!(captured_lines().iter().any(|line| {
        line.contains("the activity log could not take a record: activity disk is full")
    }));
    // Clones share the counter.
    assert_eq!(executor.clone().activity_write_failures(), 2);
}

#[test]
fn an_executor_without_an_activity_log_behaves_as_before() {
    let system = FakeSystem::new();
    let executor = Executor::new(system.ports(), system.journal.clone(), system.clock.clone());
    let plan = plan_of(&system, vec![set_dword("Flag", 1)]);
    assert!(
        executor
            .apply(&plan, Source::Manual)
            .expect("apply")
            .is_success()
    );
    assert_eq!(executor.activity_write_failures(), 0);
    assert!(system.activity.records().is_empty());
}

// ---- redaction ---------------------------------------------------------------------------------

/// A commands port whose programs print the secret and exit with a failure.
struct LoudCommands;

impl Commands for LoudCommands {
    fn run(&self, command: &Command) -> Result<CommandOutput, PortError> {
        if command.args.first().is_some_and(|arg| arg == "detect") {
            return Ok(CommandOutput {
                exit_code: 1,
                output: String::new(),
            });
        }
        Ok(CommandOutput {
            exit_code: 5,
            output: format!("token={SECRET}"),
        })
    }
}

#[test]
fn a_secret_in_a_value_an_argument_or_an_output_appears_nowhere() {
    install_logger();
    let system = FakeSystem::new();
    let ports = Ports {
        commands: Arc::new(LoudCommands),
        ..system.ports()
    };
    let executor = Executor::new(ports.clone(), system.journal.clone(), system.clock.clone())
        .with_activity_log(system.activity.clone());

    // 1. Registry data of string, expandable string, multi-string and binary kinds.
    let strings = vec![
        RegistryValue::Sz(SECRET.to_owned()),
        RegistryValue::ExpandSz(format!("%PATH%;{SECRET}")),
        RegistryValue::MultiSz(vec![SECRET.to_owned(), "x".to_owned()]),
        RegistryValue::Binary(SECRET.as_bytes().to_vec()),
    ];
    let mut operations: Vec<Operation> = strings
        .into_iter()
        .enumerate()
        .map(|(n, value)| Operation::SetRegistryValue {
            target: value_ref(&format!("Secret{n}")),
            value,
        })
        .collect();
    // A previous value holding the secret, so the revert summary has to hide it too.
    system
        .registry
        .set(&value_ref("Secret0"), RegistryValue::Sz(SECRET.to_owned()));
    // 2. A command with the secret as an argument whose output contains it as well.
    operations.push(Operation::RunCommand {
        detect: Command::new("tool.exe", ["detect", SECRET]),
        apply: Command::new("tool.exe", ["apply", SECRET]),
        revert: Command::new("tool.exe", ["revert", SECRET]),
    });
    let plan = plan_with(&ports, operations[..4].to_vec()).with_label("tweak.secret");
    let report = executor
        .apply_or_rollback(&plan, Source::Manual)
        .expect("apply");
    assert!(report.is_success());
    executor.revert_batch(report.batch).expect("revert");

    // The command step fails with the secret in its output; the failure is
    // recorded, rolled back, and the port error text is never used.
    let failing = plan_with(&ports, vec![operations[4].clone()]);
    let failed = executor
        .apply_or_rollback(&failing, Source::Manual)
        .expect("apply");
    assert!(!failed.is_success());
    // A port error whose own message carries the secret.
    system.registry.fail_on(
        &value_ref("Secret0"),
        PortError::Other(format!("write failed for data {SECRET}")),
    );
    let leaky = plan_with(&ports, vec![operations[0].clone()]);
    let leaked = executor
        .apply_or_rollback(&leaky, Source::Manual)
        .expect("apply");
    assert!(!leaked.is_success());

    let records = system.activity.records();
    assert!(records.len() >= 8, "every event was recorded: {records:#?}");
    assert!(
        records
            .iter()
            .any(|r| r.summary.contains("REG_SZ (19 chars)")
                || r.summary.contains("REG_BINARY (19 bytes)")),
        "string and binary data show kind and length only"
    );
    assert!(
        records
            .iter()
            .any(|r| r.summary.contains("command tool.exe")),
        "a command shows its program"
    );
    for r in &records {
        assert!(!r.summary.contains(SECRET), "summary leaks: {}", r.summary);
        assert!(r.label.as_deref().is_none_or(|l| !l.contains(SECRET)));
    }
    let exported = render_text(&records);
    assert!(!exported.contains(SECRET), "export leaks:\n{exported}");
    let log = captured_lines().join("\n");
    assert!(
        log.contains("command tool.exe"),
        "the diagnostic log saw the events"
    );
    assert!(!log.contains(SECRET), "diagnostic log leaks:\n{log}");
}
