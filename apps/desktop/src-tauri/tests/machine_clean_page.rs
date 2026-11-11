//! Machine-wide cleaning through the helper, at the IPC boundary, over a real core built on the
//! in-memory fakes and a throwaway store.
//!
//! Nothing here starts a helper, shows a prompt or deletes a file: the elevator is `FakeElevator`.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::machine::{ApplySkips, CleanApplyReply, CleanScanReply, ScanSkips};
use volt_core::elevation::fakes::{ElevatorCall, FakeElevator};
use volt_core::fakes::FakeSystem;
use volt_core::{ActivityOutcome, ElevationError, ErrorCode, MachineProblem, SettingsExt};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::machine_clean::{
    MachineCleanResult, MachineScanResult, clean_machine_rule_of, scan_machine_rule_of,
};
use volt_desktop_lib::wiring::{AppCore, build_core};

const TEMP: &str = "windows-temp";

fn core_with(system: &FakeSystem, dir: &TempDir, elevator: Arc<FakeElevator>) -> AppCore {
    build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds")
    .with_elevator(elevator)
}

fn found(files: u64, bytes: u64) -> CleanScanReply {
    CleanScanReply {
        scan: "00112233445566778899aabbccddeeff".to_owned(),
        rule: String::new(),
        files,
        folders: 2,
        bytes,
        min_age_days: 7,
        truncated: false,
        confirm: true,
        skipped: ScanSkips {
            too_new: 3,
            user_writable: 1,
            ..ScanSkips::default()
        },
        problem: None,
    }
}

fn deleted(files: u64, bytes: u64) -> CleanApplyReply {
    CleanApplyReply {
        rule: String::new(),
        files,
        folders: 2,
        bytes,
        skipped: ApplySkips {
            locked: 1,
            changed: 2,
            ..ApplySkips::default()
        },
        stopped: false,
    }
}

struct Rig {
    _dir: TempDir,
    elevator: Arc<FakeElevator>,
    core: AppCore,
}

fn rig() -> Rig {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::granted(system.memory.clone()));
    let core = core_with(&system, &dir, elevator.clone());
    Rig {
        _dir: dir,
        elevator,
        core,
    }
}

#[test]
fn a_scan_asks_the_helper_for_the_rule_by_id_and_shows_counts_with_no_name() {
    let rig = rig();
    rig.elevator.set_scan(Ok(found(10, 4096)));
    let MachineScanResult::Done { scan } = scan_machine_rule_of(&rig.core, TEMP).expect("scan")
    else {
        panic!("expected a scan");
    };
    assert_eq!(
        rig.elevator.calls(),
        [ElevatorCall::CleanScan(TEMP.into(), None)]
    );
    assert_eq!((scan.files, scan.folders), (10, 2));
    assert!((scan.bytes - 4096.0).abs() < f64::EPSILON);
    assert!(scan.confirm && scan.can_clean);
    assert_eq!(
        scan.skipped.as_deref(),
        Some("3 too new, 1 folders that other accounts can write to left alone")
    );
    assert_eq!(
        scan.skipped_phrase
            .as_ref()
            .map(|phrase| phrase.code.as_str()),
        Some("scan.skipped")
    );
    // The helper's scan id is not in what crosses to the window.
    let json = serde_json::to_string(&scan).expect("json");
    assert!(!json.contains("00112233"), "{json}");
    assert_eq!(rig.elevator.prompts(), 1);
}

#[test]
fn the_users_minimum_age_is_sent_and_is_only_ever_a_request_to_raise_it() {
    let rig = rig();
    rig.core
        .settings
        .set_typed("clean.temp_min_age_days", &30_u32)
        .expect("setting");
    scan_machine_rule_of(&rig.core, TEMP).expect("scan");
    assert_eq!(
        rig.elevator.calls(),
        [ElevatorCall::CleanScan(TEMP.into(), Some(30))]
    );
}

#[test]
fn only_machine_rules_go_to_the_helper() {
    let rig = rig();
    for id in ["user-temp", "chrome-cache", "no-such-rule"] {
        let error = scan_machine_rule_of(&rig.core, id).expect_err("refused");
        assert!(
            format!("{error:?}").contains("InvalidInput")
                || format!("{error:?}").contains("invalidInput"),
            "{error:?}"
        );
        assert!(clean_machine_rule_of(&rig.core, id).is_err());
    }
    assert!(
        rig.elevator.calls().is_empty(),
        "nothing reached the helper"
    );
    assert_eq!(rig.elevator.prompts(), 0);
}

#[test]
fn a_declined_prompt_is_an_outcome_and_leaves_nothing_to_clean() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::declined(system.memory.clone()));
    let core = core_with(&system, &dir, elevator.clone());
    assert_eq!(
        scan_machine_rule_of(&core, TEMP).expect("scan"),
        MachineScanResult::Declined
    );
    assert_eq!(
        clean_machine_rule_of(&core, TEMP).expect("clean"),
        MachineCleanResult::NoScan
    );
}

#[test]
fn a_problem_is_a_phrase_and_nothing_can_be_cleaned() {
    let rig = rig();
    rig.elevator.set_scan(Ok(CleanScanReply {
        scan: String::new(),
        problem: Some(MachineProblem::RootMissing),
        ..found(0, 0)
    }));
    let MachineScanResult::Done { scan } = scan_machine_rule_of(&rig.core, TEMP).expect("scan")
    else {
        panic!("expected a scan");
    };
    assert!(!scan.can_clean);
    assert_eq!(
        scan.problem_phrase
            .as_ref()
            .map(|phrase| phrase.code.as_str()),
        Some("machine.problem.rootMissing")
    );
    assert_eq!(
        clean_machine_rule_of(&rig.core, TEMP).expect("clean"),
        MachineCleanResult::NoScan
    );
}

#[test]
fn a_clean_uses_the_scan_once_deletes_permanently_and_records_a_run_that_cannot_be_undone() {
    let rig = rig();
    rig.elevator.set_scan(Ok(found(10, 4096)));
    rig.elevator.set_apply(Ok(deleted(9, 4000)));
    scan_machine_rule_of(&rig.core, TEMP).expect("scan");
    let result = clean_machine_rule_of(&rig.core, TEMP).expect("clean");
    let MachineCleanResult::Done {
        files,
        folders,
        bytes,
        skipped_total,
        skipped,
        message,
        history_incomplete,
        ..
    } = result
    else {
        panic!("expected a done result, got {result:?}");
    };
    assert_eq!((files, folders, skipped_total), (9, 2, 3));
    assert!((bytes - 4000.0).abs() < f64::EPSILON);
    assert_eq!(
        skipped.as_deref(),
        Some("1 in use or denied by Windows, 2 changed since the scan")
    );
    assert!(!history_incomplete);
    assert!(message.contains("Permanently deleted"), "{message}");
    assert!(message.contains("cannot be undone"), "{message}");
    assert!(
        message.contains("did not go to the Recycle Bin"),
        "{message}"
    );
    // The helper got exactly the scan id it had given, and only once.
    assert_eq!(
        rig.elevator.calls()[1],
        ElevatorCall::CleanApply("00112233445566778899aabbccddeeff".into())
    );
    assert_eq!(
        clean_machine_rule_of(&rig.core, TEMP).expect("clean again"),
        MachineCleanResult::NoScan,
        "a scan is good for one clean"
    );

    // The activity record says so and has no undo journal entry.
    let records = rig.core.activity.list(10, None).expect("records");
    let record = records
        .iter()
        .find(|record| record.label.as_deref() == Some(TEMP))
        .expect("a record of the run");
    assert_eq!(record.outcome, ActivityOutcome::Applied);
    assert!(record.summary.contains("cannot be undone"));
    assert!(record.batch.is_none() && record.entry.is_none());
    assert_eq!(
        record.phrase.as_ref().map(|phrase| phrase.code.as_str()),
        Some("summary.cleanMachine")
    );
}

#[test]
fn a_run_that_deleted_nothing_is_recorded_as_failed() {
    let rig = rig();
    rig.elevator.set_scan(Ok(found(1, 10)));
    rig.elevator.set_apply(Ok(CleanApplyReply {
        files: 0,
        folders: 0,
        bytes: 0,
        ..deleted(0, 0)
    }));
    scan_machine_rule_of(&rig.core, TEMP).expect("scan");
    clean_machine_rule_of(&rig.core, TEMP).expect("clean");
    let records = rig.core.activity.list(10, None).expect("records");
    assert_eq!(records[0].outcome, ActivityOutcome::Failed);
}

#[test]
fn refusals_of_the_helper_are_outcomes_and_nothing_is_recorded() {
    for (error, expected) in [
        (
            ElevationError::Refused(ErrorCode::NotConfirmed),
            MachineCleanResult::NotConfirmed,
        ),
        (
            ElevationError::Refused(ErrorCode::Blocked),
            MachineCleanResult::Blocked,
        ),
        (
            ElevationError::Refused(ErrorCode::UnknownScan),
            MachineCleanResult::NoScan,
        ),
        (ElevationError::TimedOut, MachineCleanResult::TimedOut),
        (ElevationError::Declined, MachineCleanResult::Declined),
    ] {
        let rig = rig();
        rig.elevator.set_scan(Ok(found(1, 10)));
        scan_machine_rule_of(&rig.core, TEMP).expect("scan");
        rig.elevator.set_apply(Err(error.clone()));
        assert_eq!(
            clean_machine_rule_of(&rig.core, TEMP).expect("an outcome"),
            expected,
            "{error:?}"
        );
        let records = rig.core.activity.list(10, None).expect("records");
        assert!(
            records
                .iter()
                .all(|record| record.label.as_deref() != Some(TEMP)),
            "{error:?} wrote a record"
        );
    }
}

#[test]
fn an_unavailable_helper_is_an_error_and_nothing_is_started() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let elevator = Arc::new(FakeElevator::unavailable(
        system.memory.clone(),
        "not installed for all users",
    ));
    let core = core_with(&system, &dir, elevator.clone());
    assert!(scan_machine_rule_of(&core, TEMP).is_err());
    assert_eq!(elevator.prompts(), 0);
}
