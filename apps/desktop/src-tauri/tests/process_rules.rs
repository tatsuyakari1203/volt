//! Process settings and saved rules over a real core built on the in-memory fakes and a throwaway
//! store: the commands behind the Settings dialog and the rules card, and the undo of a change
//! from the Undo page's backend.
//!
//! Nothing here touches this machine's processes: the port is `FakeProcesses`.

use tempfile::TempDir;
use volt_core::fakes::{FakeProcesses, FakeSystem};
use volt_core::{PortError, PriorityClass};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{ChangeState, UndoStatus, list_activity, undo_entry};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::tuning::{
    PriorityView, RuleProblemView, TuneRequestView, inspect_app_of, process_rules_of,
    remove_process_rule_of, tune_app_of,
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

fn request(priority: Option<PriorityView>, cores: Option<Vec<u32>>) -> TuneRequestView {
    TuneRequestView {
        priority,
        cores,
        efficiency: None,
    }
}

fn priority_of(system: &FakeSystem, pid: u32) -> PriorityClass {
    system.processes.settings_of(pid).expect("process").priority
}

#[test]
fn inspecting_says_what_the_processes_hold_and_how_many_can_be_changed() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "game.exe", 1);
    system.processes.add(201, "game.exe", 1);
    system
        .processes
        .fail_settings(201, PortError::AccessDenied("elevated".into()));
    let core = core_with(&system, &dir);

    let view = inspect_app_of(&core, "game.exe").expect("inspect");
    assert_eq!(view.name, "game.exe");
    assert_eq!(
        (view.processes, view.changeable, view.needs_administrator),
        (2, 1, 1)
    );
    assert_eq!(view.priority, Some(PriorityView::Normal));
    assert_eq!(view.cores, Some((0..8).collect::<Vec<u32>>()));
    assert_eq!(view.available_cores, (0..8).collect::<Vec<u32>>());
    assert_eq!(view.efficiency, Some(false));
}

#[test]
fn a_change_is_journaled_shows_on_the_undo_page_and_is_undone_from_there() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "game.exe", 1);
    system.processes.add(201, "game.exe", 1);
    let core = core_with(&system, &dir);

    let result = tune_app_of(
        &core,
        "game.exe",
        &request(Some(PriorityView::High), Some(vec![0, 1])),
        false,
    )
    .expect("tune");
    assert_eq!((result.changed, result.already_set), (2, 0));
    assert!(result.recorded && !result.rule_saved);
    assert_eq!(priority_of(&system, 200), PriorityClass::High);
    assert_eq!(
        system.processes.settings_of(201).expect("p").affinity_mask,
        0b11
    );

    let page = list_activity(&core, 10, None).expect("activity");
    let item = &page.items[0];
    assert_eq!(item.state, ChangeState::CanUndo);
    assert_eq!(item.label.as_deref(), Some("process.tune"));
    assert_eq!(item.title.as_deref(), Some("Process settings"));
    assert_eq!(
        item.summary,
        "Changed game.exe: priority high, 2 processor cores"
    );
    // What it was before, in words.
    assert_eq!(
        item.previous.as_deref(),
        Some("priority normal, 8 processor cores")
    );

    let entry = item.entry.clone().expect("entry");
    let undone = undo_entry(&core, &entry, &[]).expect("undo");
    assert_eq!((undone.status, undone.restored), (UndoStatus::Restored, 1));
    assert_eq!(
        system.processes.settings_of(200),
        Some(FakeProcesses::DEFAULT_SETTINGS)
    );
    assert_eq!(
        system.processes.settings_of(201),
        Some(FakeProcesses::DEFAULT_SETTINGS)
    );
}

#[test]
fn undoing_after_the_app_ended_is_a_no_op_that_the_history_says_so() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "game.exe", 1);
    let core = core_with(&system, &dir);
    tune_app_of(
        &core,
        "game.exe",
        &request(Some(PriorityView::Idle), None),
        false,
    )
    .expect("tune");
    let entry = list_activity(&core, 10, None).expect("activity").items[0]
        .entry
        .clone()
        .expect("entry");
    volt_core::Processes::terminate(&*system.processes, 200, 0).expect("end the fake");

    let undone = undo_entry(&core, &entry, &[]).expect("undo");
    assert_eq!(undone.status, UndoStatus::Restored);
    let page = list_activity(&core, 10, None).expect("activity");
    assert!(
        page.items[0].summary.contains("already ended"),
        "{}",
        page.items[0].summary
    );
}

#[test]
fn saving_a_rule_lists_it_and_applies_it_to_a_process_that_starts_later() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "game.exe", 1);
    system.processes.set_path(200, "C:\\Games\\Game.exe");
    let core = core_with(&system, &dir);

    let result = tune_app_of(
        &core,
        "c:\\games\\game.exe",
        &request(Some(PriorityView::BelowNormal), Some(vec![2, 3])),
        true,
    )
    .expect("tune");
    assert!(result.rule_saved);

    let rules = process_rules_of(&core).expect("rules");
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].id, "c:\\games\\game.exe");
    assert_eq!(rules[0].name, "game.exe");
    assert_eq!(rules[0].request.cores, Some(vec![2, 3]));
    assert_eq!(rules[0].problem, None);

    // The same process is not touched again; a new one is.
    let tuner = core.process_tuner().expect("tuner");
    assert_eq!(tuner.apply_rules().changed, 0);
    system.processes.add(300, "game.exe", 1);
    system.processes.set_path(300, "C:\\Games\\Game.exe");
    assert_eq!(tuner.apply_rules().changed, 1);
    assert_eq!(priority_of(&system, 300), PriorityClass::BelowNormal);

    // The rule is in the store: a new core over the same database lists it.
    drop(core);
    let again = core_with(&system, &dir);
    assert_eq!(process_rules_of(&again).expect("rules").len(), 1);
}

#[test]
fn a_rule_that_cannot_be_applied_shows_its_problem_and_is_removable() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "vendor.exe", 1);
    let core = core_with(&system, &dir);
    tune_app_of(
        &core,
        "vendor.exe",
        &request(Some(PriorityView::Idle), None),
        true,
    )
    .expect("tune");
    // The app is restarted and its new process runs elevated, so the rule cannot reach it.
    volt_core::Processes::terminate(&*system.processes, 200, 0).expect("end the fake");
    system.processes.add(201, "vendor.exe", 1);
    system
        .processes
        .fail_settings(201, PortError::AccessDenied("elevated".into()));
    core.process_tuner().expect("tuner").apply_rules();

    let rules = process_rules_of(&core).expect("rules");
    assert_eq!(rules[0].problem, Some(RuleProblemView::NeedsAdministrator));
    assert!(
        remove_process_rule_of(&core, "vendor.exe")
            .expect("remove")
            .is_empty()
    );
    assert!(process_rules_of(&core).expect("rules").is_empty());
}

#[test]
fn a_rule_is_not_saved_for_an_app_nothing_of_which_can_be_changed() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(200, "vendor.exe", 1);
    system
        .processes
        .fail_settings(200, PortError::AccessDenied("elevated".into()));
    let core = core_with(&system, &dir);
    let result = tune_app_of(
        &core,
        "vendor.exe",
        &request(Some(PriorityView::Idle), None),
        true,
    )
    .expect("tune");
    assert_eq!((result.changed, result.needs_administrator), (0, 1));
    assert!(!result.rule_saved);
    assert!(process_rules_of(&core).expect("rules").is_empty());
}

#[test]
fn protected_apps_an_empty_core_list_and_a_missing_core_are_refused_with_nothing_changed() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    system.processes.add(500, "winlogon.exe", 1);
    system.processes.add(200, "game.exe", 1);
    let core = core_with(&system, &dir);

    for id in ["winlogon.exe", "WINLOGON.EXE"] {
        assert!(matches!(
            tune_app_of(&core, id, &request(Some(PriorityView::Idle), None), true),
            Err(IpcError::InvalidInput { .. })
        ));
        assert!(matches!(
            inspect_app_of(&core, id),
            Err(IpcError::InvalidInput { .. })
        ));
    }
    // An empty list of cores, a core the machine does not have, a core beyond 63 and no change
    // at all.
    for bad in [
        request(None, Some(vec![])),
        request(None, Some(vec![12])),
        request(None, Some(vec![64])),
        request(None, None),
    ] {
        assert!(
            matches!(
                tune_app_of(&core, "game.exe", &bad, true),
                Err(IpcError::InvalidInput { .. })
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        tune_app_of(
            &core,
            "nothing.exe",
            &request(Some(PriorityView::Idle), None),
            false
        ),
        Err(IpcError::NotFound { .. })
    ));
    assert_eq!(
        system.processes.settings_of(200),
        Some(FakeProcesses::DEFAULT_SETTINGS)
    );
    assert!(process_rules_of(&core).expect("rules").is_empty());
}
