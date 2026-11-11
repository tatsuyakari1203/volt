use super::*;
use crate::fakes::{FakeProcesses, FakeSettings, FakeSystem};
use crate::journal::{EntryState, Journal};
use crate::operation::PriorState;

const VOLT: u32 = 100;

struct Setup {
    system: FakeSystem,
    settings: Arc<FakeSettings>,
    executor: Executor,
    tuner: ProcessTuner,
}

fn setup() -> Setup {
    let system = FakeSystem::new();
    let settings = Arc::new(FakeSettings::new());
    let executor = system.executor_with_activity();
    let tuner = ProcessTuner::new(
        system.processes.clone(),
        executor.clone(),
        settings.clone(),
        system.clock.clone(),
        VOLT,
    )
    .with_activity_log(system.activity.clone());
    Setup {
        system,
        settings,
        executor,
        tuner,
    }
}

fn settings_of(s: &Setup, pid: u32) -> ProcessSettings {
    s.system.processes.settings_of(pid).expect("process")
}

fn priority(class: PriorityClass) -> TuneRequest {
    TuneRequest {
        priority: Some(class),
        ..TuneRequest::default()
    }
}

#[test]
fn priority_affinity_and_efficiency_are_set_on_every_process_of_the_app() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.add(11, "game.exe", 1);
    s.system.processes.add(12, "other.exe", 1);

    let request = TuneRequest {
        priority: Some(PriorityClass::High),
        affinity_mask: Some(0b0011),
        efficiency: None,
    };
    let report = s
        .tuner
        .tune("game.exe", &request, Source::Manual)
        .expect("tune");
    assert_eq!((report.changed, report.already_set), (2, 0));
    assert!(report.batch.is_some());
    for pid in [10, 11] {
        let now = settings_of(&s, pid);
        assert_eq!(now.priority, PriorityClass::High);
        assert_eq!(now.affinity_mask, 0b0011);
        assert_eq!(now.throttling, PowerThrottling::Default);
    }
    // Only that app was touched.
    assert_eq!(settings_of(&s, 12), FakeProcesses::DEFAULT_SETTINGS);

    let efficiency = TuneRequest {
        efficiency: Some(true),
        ..TuneRequest::default()
    };
    s.tuner
        .tune("game.exe", &efficiency, Source::Manual)
        .expect("tune");
    let now = settings_of(&s, 10);
    // Efficiency mode is EcoQoS together with the lowest priority class.
    assert_eq!(now.priority, PriorityClass::Idle);
    assert_eq!(now.throttling, PowerThrottling::On);
    assert_eq!(now.affinity_mask, 0b0011, "the affinity was not asked for");
}

#[test]
fn a_change_is_one_journal_entry_that_puts_back_what_each_process_held() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.add(11, "game.exe", 1);
    s.system.processes.set_settings(
        11,
        ProcessSettings {
            priority: PriorityClass::BelowNormal,
            ..FakeProcesses::DEFAULT_SETTINGS
        },
    );
    let request = TuneRequest {
        priority: Some(PriorityClass::High),
        affinity_mask: Some(0b1),
        efficiency: None,
    };
    s.tuner
        .tune("game.exe", &request, Source::Manual)
        .expect("tune");

    let applied = s
        .system
        .journal
        .entries_in_state(EntryState::Applied)
        .expect("entries");
    assert_eq!(applied.len(), 1);
    let entry = &applied[0];
    let PriorState::Processes(before) = &entry.prior else {
        panic!("prior was {:?}", entry.prior);
    };
    assert_eq!(before.len(), 2);
    assert_eq!(before[0], Some(FakeProcesses::DEFAULT_SETTINGS));
    assert_eq!(
        before[1].map(|b| b.priority),
        Some(PriorityClass::BelowNormal)
    );

    s.executor
        .revert_entry(entry.id, Source::Manual, None)
        .expect("revert");
    assert_eq!(settings_of(&s, 10), FakeProcesses::DEFAULT_SETTINGS);
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::BelowNormal);
    assert_eq!(settings_of(&s, 11).affinity_mask, 0xFF);
}

#[test]
fn only_the_fields_that_were_changed_are_restored() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .tune("game.exe", &priority(PriorityClass::High), Source::Manual)
        .expect("tune");
    let entry = s
        .system
        .journal
        .entries_in_state(EntryState::Applied)
        .expect("entries")[0]
        .id;
    // Something else changes the affinity afterwards: undoing the priority leaves it alone.
    s.system.processes.set_settings(
        10,
        ProcessSettings {
            priority: PriorityClass::High,
            affinity_mask: 0b10,
            ..FakeProcesses::DEFAULT_SETTINGS
        },
    );
    s.executor
        .revert_entry(entry, Source::Manual, None)
        .expect("revert");
    let now = settings_of(&s, 10);
    assert_eq!(now.priority, PriorityClass::Normal);
    assert_eq!(now.affinity_mask, 0b10);
}

#[test]
fn undoing_after_every_process_ended_is_a_no_op_that_says_so() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .tune("game.exe", &priority(PriorityClass::Idle), Source::Manual)
        .expect("tune");
    let entry = s
        .system
        .journal
        .entries_in_state(EntryState::Applied)
        .expect("entries")[0]
        .id;
    s.system.processes.terminate(10, 0).expect("end the fake");

    s.executor
        .revert_entry(entry, Source::Manual, None)
        .expect("a no-op, not an error");
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
    let last = s.system.activity.records().pop().expect("record");
    assert!(last.summary.contains("already ended"), "{}", last.summary);
    assert!(
        last.summary.contains("nothing to restore"),
        "{}",
        last.summary
    );
}

#[test]
fn undoing_restores_the_processes_that_live_and_skips_the_ended_ones() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.add(11, "game.exe", 1);
    s.tuner
        .tune("game.exe", &priority(PriorityClass::High), Source::Manual)
        .expect("tune");
    let entry = s
        .system
        .journal
        .entries_in_state(EntryState::Applied)
        .expect("entries")[0]
        .id;
    s.system.processes.terminate(10, 0).expect("end the fake");
    s.executor
        .revert_entry(entry, Source::Manual, None)
        .expect("revert");
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::Normal);
}

#[test]
fn a_request_that_cannot_be_right_is_refused_before_anything_is_read() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    let tune = |request: TuneRequest| s.tuner.tune("game.exe", &request, Source::Manual);
    assert_eq!(tune(TuneRequest::default()), Err(TuneError::Nothing));
    assert_eq!(
        tune(TuneRequest {
            affinity_mask: Some(0),
            ..TuneRequest::default()
        }),
        Err(TuneError::EmptyAffinity)
    );
    // Core 9 does not exist on the fake's eight cores.
    assert_eq!(
        tune(TuneRequest {
            affinity_mask: Some(1 << 9),
            ..TuneRequest::default()
        }),
        Err(TuneError::UnknownCore)
    );
    assert_eq!(settings_of(&s, 10), FakeProcesses::DEFAULT_SETTINGS);
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
}

#[test]
fn realtime_is_not_a_class_that_can_be_asked_for() {
    // The type has no such variant, and a stored or sent "realtime" does not parse.
    assert!(serde_json::from_str::<PriorityClass>("\"realtime\"").is_err());
    assert_eq!(PriorityClass::OFFERED.len(), 5);
}

#[test]
fn protected_apps_are_refused_in_the_core_whatever_was_asked() {
    let s = setup();
    s.system.processes.add(500, "winlogon.exe", 1);
    s.system.processes.add(600, "service.exe", 1);
    s.system.processes.set_session(600, 0);
    s.system.processes.add_process(VOLT, 1, "volt.exe", 1, 0);
    s.system
        .processes
        .add_process(VOLT + 1, VOLT, "webview.exe", 1, 0);
    let request = priority(PriorityClass::Idle);
    for (key, protection) in [
        ("winlogon.exe", Protection::Windows),
        ("SERVICE.EXE", Protection::Windows),
        ("webview.exe", Protection::Volt),
    ] {
        assert!(
            matches!(
                s.tuner.tune(key, &request, Source::Manual),
                Err(TuneError::Protected { protection: p, .. }) if p == protection
            ),
            "{key}"
        );
        assert!(
            matches!(s.tuner.inspect(key), Err(TuneError::Protected { .. })),
            "{key}"
        );
    }
    assert!(matches!(
        s.tuner.tune("nothing.exe", &request, Source::Manual),
        Err(TuneError::NotRunning(_))
    ));
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
}

#[test]
fn processes_the_user_may_not_change_are_counted_and_left_alone() {
    let s = setup();
    s.system.processes.add(10, "vendor.exe", 1);
    s.system.processes.add(11, "vendor.exe", 1);
    s.system
        .processes
        .fail_settings(11, PortError::AccessDenied("elevated".into()));
    let inspection = s.tuner.inspect("vendor.exe").expect("inspect");
    assert_eq!(
        (
            inspection.processes,
            inspection.changeable,
            inspection.needs_administrator
        ),
        (2, 1, 1)
    );

    let report = s
        .tuner
        .tune("vendor.exe", &priority(PriorityClass::Idle), Source::Manual)
        .expect("tune");
    assert_eq!((report.changed, report.needs_administrator), (1, 1));
    assert_eq!(settings_of(&s, 10).priority, PriorityClass::Idle);
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::Normal);
}

#[test]
fn an_app_with_nothing_changeable_changes_nothing_and_writes_no_entry() {
    let s = setup();
    s.system.processes.add(10, "vendor.exe", 1);
    s.system
        .processes
        .fail_settings(10, PortError::AccessDenied("elevated".into()));
    let report = s
        .tuner
        .tune("vendor.exe", &priority(PriorityClass::Idle), Source::Manual)
        .expect("tune");
    assert_eq!((report.changed, report.needs_administrator), (0, 1));
    assert_eq!(report.batch, None);
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
}

#[test]
fn processes_that_already_hold_the_settings_are_not_touched_or_recorded() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    let report = s
        .tuner
        .tune("game.exe", &priority(PriorityClass::Normal), Source::Manual)
        .expect("tune");
    assert_eq!(
        (report.changed, report.already_set, report.batch),
        (0, 1, None)
    );
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
}

#[test]
fn a_failure_while_applying_leaves_every_process_as_it_was() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.add(11, "game.exe", 1);
    s.system
        .processes
        .fail_apply_settings(11, PortError::AccessDenied("protected".into()));
    let result = s
        .tuner
        .tune("game.exe", &priority(PriorityClass::High), Source::Manual);
    assert!(matches!(result, Err(TuneError::Exec(_))), "{result:?}");
    assert_eq!(settings_of(&s, 10), FakeProcesses::DEFAULT_SETTINGS);
    assert_eq!(settings_of(&s, 11), FakeProcesses::DEFAULT_SETTINGS);
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
}

#[test]
fn inspecting_reads_only_and_says_what_all_the_processes_share() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.add(11, "game.exe", 1);
    let inspection = s.tuner.inspect("game.exe").expect("inspect");
    assert_eq!(inspection.priority, Some(PriorityClass::Normal));
    assert_eq!(inspection.affinity_mask, Some(0xFF));
    assert_eq!(inspection.system_affinity_mask, 0xFF);
    assert_eq!(inspection.efficiency, Some(false));

    s.system.processes.set_settings(
        11,
        ProcessSettings {
            priority: PriorityClass::High,
            throttling: PowerThrottling::On,
            ..FakeProcesses::DEFAULT_SETTINGS
        },
    );
    let mixed = s.tuner.inspect("game.exe").expect("inspect");
    assert_eq!((mixed.priority, mixed.efficiency), (None, None));
    assert_eq!(mixed.affinity_mask, Some(0xFF));
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
    assert!(
        s.system
            .log
            .events()
            .iter()
            .all(|e| !matches!(e, crate::fakes::Event::ProcessTuned(_)))
    );
}

// ---- rules ------------------------------------------------------------------------------------

#[test]
fn a_rule_is_saved_listed_and_removed_and_survives_a_new_tuner() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.set_path(10, "C:\\Games\\Game.exe");
    let request = TuneRequest {
        priority: Some(PriorityClass::AboveNormal),
        affinity_mask: Some(0b1111),
        efficiency: None,
    };
    let rule = s
        .tuner
        .save_rule("c:\\games\\game.exe", &request)
        .expect("save");
    assert_eq!(rule.key, "c:\\games\\game.exe");
    assert_eq!(rule.name, "game.exe");
    assert_eq!(rule.path.as_deref(), Some("c:\\games\\game.exe"));

    // Saving again replaces the rule of the same app.
    let replaced = s
        .tuner
        .save_rule("c:\\games\\game.exe", &priority(PriorityClass::Idle))
        .expect("save");
    assert_eq!(replaced.key, rule.key);
    let listed = s.tuner.rules().expect("rules");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].rule.request.priority, Some(PriorityClass::Idle));
    assert_eq!(listed[0].problem, None);

    // The store holds it: another tuner over the same settings sees it.
    let again = ProcessTuner::new(
        s.system.processes.clone(),
        s.executor.clone(),
        s.settings.clone(),
        s.system.clock.clone(),
        VOLT,
    );
    assert_eq!(again.rules().expect("rules").len(), 1);

    assert!(s.tuner.remove_rule("C:\\Games\\GAME.exe").expect("remove"));
    assert!(
        !s.tuner
            .remove_rule("c:\\games\\game.exe")
            .expect("remove twice")
    );
    assert!(s.tuner.rules().expect("rules").is_empty());
    assert!(
        again.rules().expect("cached").len() == 1,
        "a tuner reads the store once"
    );
}

#[test]
fn a_rule_needs_a_running_unprotected_app_and_a_request_that_can_be_right() {
    let s = setup();
    s.system.processes.add(500, "winlogon.exe", 1);
    assert!(matches!(
        s.tuner
            .save_rule("winlogon.exe", &priority(PriorityClass::Idle)),
        Err(TuneError::Protected { .. })
    ));
    assert!(matches!(
        s.tuner
            .save_rule("gone.exe", &priority(PriorityClass::Idle)),
        Err(TuneError::NotRunning(_))
    ));
    s.system.processes.add(10, "game.exe", 1);
    assert_eq!(
        s.tuner.save_rule("game.exe", &TuneRequest::default()),
        Err(TuneError::Nothing)
    );
    assert_eq!(
        s.tuner.save_rule(
            "game.exe",
            &TuneRequest {
                affinity_mask: Some(0),
                ..TuneRequest::default()
            }
        ),
        Err(TuneError::EmptyAffinity)
    );
    assert!(s.tuner.rules().expect("rules").is_empty());
}

#[test]
fn a_rule_is_applied_to_a_process_that_starts_and_only_once() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .save_rule("game.exe", &priority(PriorityClass::BelowNormal))
        .expect("save");

    // The process that runs when the rule is saved is caught on the next pass.
    assert_eq!(s.tuner.apply_rules().changed, 1);
    assert_eq!(settings_of(&s, 10).priority, PriorityClass::BelowNormal);

    // Reapplying a rule is not an undo entry (removing the rule is the undo); one activity line
    // says what the rule did, once.
    let entries = || {
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .len()
    };
    let lines = || {
        s.system
            .activity
            .records()
            .iter()
            .filter(|r| r.summary.contains("saved rule for game.exe changed"))
            .count()
    };
    assert_eq!(entries(), 0);
    assert_eq!(lines(), 1);
    assert_eq!(s.tuner.apply_rules(), RulesRun::default());

    // A new process of the app is changed, one of another app is not, and nothing is written.
    s.system.processes.add(20, "GAME.EXE", 1);
    s.system.processes.add(21, "other.exe", 1);
    assert_eq!(s.tuner.apply_rules().changed, 1);
    assert_eq!(settings_of(&s, 20).priority, PriorityClass::BelowNormal);
    assert_eq!(settings_of(&s, 21), FakeProcesses::DEFAULT_SETTINGS);
    for pid in 30..40 {
        s.system.processes.add(pid, "game.exe", 1);
        assert_eq!(s.tuner.apply_rules().changed, 1);
    }
    assert_eq!(entries(), 0);
    assert_eq!(lines(), 1, "one line per rule and session");

    // The rule list is what undoes it: saving again starts a new line.
    s.tuner.remove_rule("game.exe").expect("remove");
    assert_eq!(s.tuner.apply_rules(), RulesRun::default());
}

#[test]
fn a_rule_that_changes_some_processes_says_how_many_it_left_alone() {
    let s = setup();
    s.system.processes.add(10, "chrome.exe", 1);
    s.system.processes.add(11, "chrome.exe", 1);
    s.system.processes.add(12, "chrome.exe", 1);
    s.tuner
        .save_rule("chrome.exe", &priority(PriorityClass::BelowNormal))
        .expect("save");
    s.system
        .processes
        .fail_apply_settings(11, PortError::AccessDenied("elevated".into()));
    s.system
        .processes
        .fail_settings(12, PortError::AccessDenied("elevated".into()));

    let run = s.tuner.apply_rules();
    assert_eq!(
        run,
        RulesRun {
            changed: 1,
            failed: 0
        }
    );
    let listed = s.tuner.rules().expect("rules");
    assert_eq!(listed[0].problem, None);
    assert_eq!(listed[0].left_alone, 2);
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::Normal);
    let lines: Vec<String> = s
        .system
        .activity
        .records()
        .iter()
        .map(|r| r.summary.clone())
        .filter(|t| t.contains("saved rule"))
        .collect();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains("changed 1 process and left 2 processes alone"),
        "{lines:?}"
    );
}

#[test]
fn a_rule_is_refused_when_this_machine_lacks_a_core_it_names() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    let request = TuneRequest {
        affinity_mask: Some(1 << 9),
        ..TuneRequest::default()
    };
    assert_eq!(
        s.tuner.save_rule("game.exe", &request),
        Err(TuneError::UnknownCore)
    );
    assert!(s.tuner.rules().expect("rules").is_empty());
    let fits = TuneRequest {
        affinity_mask: Some(0b11),
        ..TuneRequest::default()
    };
    assert!(s.tuner.save_rule("game.exe", &fits).is_ok());
}

#[test]
fn turning_efficiency_off_gives_the_process_back_to_windows() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    let on = TuneRequest {
        efficiency: Some(true),
        ..TuneRequest::default()
    };
    s.tuner.tune("game.exe", &on, Source::Manual).expect("on");
    assert_eq!(settings_of(&s, 10).throttling, PowerThrottling::On);
    let off = TuneRequest {
        efficiency: Some(false),
        ..TuneRequest::default()
    };
    s.tuner.tune("game.exe", &off, Source::Manual).expect("off");
    assert_eq!(settings_of(&s, 10).throttling, PowerThrottling::Default);
}

#[test]
fn a_process_that_already_conforms_writes_no_record() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .save_rule("game.exe", &priority(PriorityClass::Normal))
        .expect("save");
    assert_eq!(s.tuner.apply_rules(), RulesRun::default());
    assert!(
        s.system
            .journal
            .entries_in_state(EntryState::Applied)
            .expect("entries")
            .is_empty()
    );
    assert!(s.system.activity.records().is_empty());
}

#[test]
fn a_rule_with_a_path_matches_only_that_file() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.system.processes.set_path(10, "C:\\Games\\Game.exe");
    s.system.processes.add(11, "game.exe", 1);
    s.system.processes.set_path(11, "D:\\Other\\Game.exe");
    s.system.processes.add(12, "game.exe", 1);
    s.tuner
        .save_rule("c:\\games\\game.exe", &priority(PriorityClass::Idle))
        .expect("save");
    assert_eq!(s.tuner.apply_rules().changed, 1);
    assert_eq!(settings_of(&s, 10).priority, PriorityClass::Idle);
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::Normal);
    assert_eq!(settings_of(&s, 12).priority, PriorityClass::Normal);
}

#[test]
fn rules_never_reach_protected_processes() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .save_rule("game.exe", &priority(PriorityClass::Idle))
        .expect("save");
    // The same name in session 0 (a service) and as a child of Volt.
    s.system.processes.add(30, "game.exe", 1);
    s.system.processes.set_session(30, 0);
    s.system.processes.add_process(VOLT, 1, "volt.exe", 1, 0);
    s.system.processes.add_process(31, VOLT, "game.exe", 1, 0);
    s.tuner.apply_rules();
    assert_eq!(settings_of(&s, 10).priority, PriorityClass::Idle);
    assert_eq!(settings_of(&s, 30).priority, PriorityClass::Normal);
    assert_eq!(settings_of(&s, 31).priority, PriorityClass::Normal);
}

#[test]
fn a_rule_that_cannot_be_applied_is_reported_once_not_on_every_tick() {
    let s = setup();
    s.system.processes.add(10, "vendor.exe", 1);
    s.tuner
        .save_rule("vendor.exe", &priority(PriorityClass::Idle))
        .expect("save");
    s.system
        .processes
        .fail_settings(10, PortError::AccessDenied("elevated".into()));

    assert_eq!(
        s.tuner.apply_rules(),
        RulesRun {
            changed: 0,
            failed: 1
        }
    );
    let failed = || {
        s.system
            .activity
            .records()
            .iter()
            .filter(|r| r.summary.contains("could not be applied"))
            .count()
    };
    assert_eq!(failed(), 1);
    let listed = s.tuner.rules().expect("rules");
    assert_eq!(listed[0].problem, Some("access denied"));

    // The same process is not tried again, and a second one that fails the same way adds no record.
    assert_eq!(s.tuner.apply_rules(), RulesRun::default());
    s.system.processes.add(11, "vendor.exe", 1);
    s.system
        .processes
        .fail_settings(11, PortError::AccessDenied("elevated".into()));
    assert_eq!(s.tuner.apply_rules().failed, 1);
    assert_eq!(failed(), 1, "one report until the rule works");

    // When it works the problem clears, and a later failure is reported again.
    s.system.processes.add(12, "vendor.exe", 1);
    assert_eq!(s.tuner.apply_rules().changed, 1);
    assert_eq!(s.tuner.rules().expect("rules")[0].problem, None);
    s.system.processes.add(13, "vendor.exe", 1);
    s.system
        .processes
        .fail_settings(13, PortError::AccessDenied("elevated".into()));
    s.tuner.apply_rules();
    assert_eq!(failed(), 2);
}

#[test]
fn with_no_rule_a_pass_reads_nothing() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    assert_eq!(s.tuner.apply_rules(), RulesRun::default());
    assert!(
        s.system
            .log
            .events()
            .iter()
            .all(|e| !matches!(e, crate::fakes::Event::ProcessTuned(_)))
    );
}

#[test]
fn the_rule_list_is_bounded() {
    let s = setup();
    for n in 0..MAX_RULES {
        let name = format!("app{n}.exe");
        s.system
            .processes
            .add(1000 + u32::try_from(n).expect("small"), &name, 1);
        s.tuner
            .save_rule(&name, &priority(PriorityClass::Idle))
            .expect("save");
    }
    s.system.processes.add(5000, "one-more.exe", 1);
    assert_eq!(
        s.tuner
            .save_rule("one-more.exe", &priority(PriorityClass::Idle)),
        Err(TuneError::TooManyRules)
    );
}

#[test]
fn the_descriptions_name_the_fields_and_never_a_path() {
    let change = TuneRequest {
        priority: Some(PriorityClass::AboveNormal),
        affinity_mask: Some(0b0111),
        efficiency: Some(false),
    }
    .change();
    let text = applied_phrase_of("game.exe", &change).english();
    assert_eq!(
        text,
        "Changed game.exe: priority above normal, 3 processor cores, Efficiency mode as Windows decides"
    );
    let on = TuneRequest {
        efficiency: Some(true),
        priority: Some(PriorityClass::High),
        affinity_mask: None,
    }
    .change();
    assert_eq!(
        on.priority,
        Some(PriorityClass::Idle),
        "Efficiency mode wins over a priority"
    );
    assert_eq!(on.throttling, Some(PowerThrottling::On));
}

#[test]
fn a_rule_without_a_path_does_not_reach_a_same_named_program_that_has_one() {
    let s = setup();
    s.system.processes.add(10, "game.exe", 1);
    s.tuner
        .save_rule("game.exe", &priority(PriorityClass::Idle))
        .expect("save");
    s.system.processes.add(11, "game.exe", 1);
    s.system
        .processes
        .set_path(11, r"C:\Windows\System32\Game.exe");
    s.tuner.apply_rules();
    assert_eq!(settings_of(&s, 10).priority, PriorityClass::Idle);
    assert_eq!(settings_of(&s, 11).priority, PriorityClass::Normal);
}
