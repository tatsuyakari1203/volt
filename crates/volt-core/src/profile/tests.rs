//! Profile tests: the machine is a set of fakes and nothing waits on real time.

use std::sync::Arc;

use super::*;
use crate::fakes::{FakeEnvironment, FakeSettings, FakeSystem};
use crate::journal::{EntryState, Journal};
use crate::registry::{RegistryHive, RegistryValue, RegistryValueRef};
use crate::tweak::Catalogue;

const CATALOGUE: &str = r#"
[[tweak]]
id = "tweak-a"
title = "Tweak A"
description = "First."
category = "performance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Profiles'
name = "A"
value = { dword = 1 }

[[tweak]]
id = "tweak-b"
title = "Tweak B"
description = "Second."
category = "performance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Profiles'
name = "B"
value = { dword = 1 }

[[tweak]]
id = "tweak-c"
title = "Tweak C"
description = "Third, with some risk."
category = "performance"
risk = "caution"
evidence = "anecdotal"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Profiles'
name = "C"
value = { dword = 1 }

[[tweak]]
id = "tweak-admin"
title = "Tweak admin"
description = "Machine wide."
category = "performance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "Test."
requires_admin = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKLM"
subkey = 'Software\VoltTest\Profiles'
name = "Admin"
value = { dword = 1 }
"#;

const START: i64 = 1_000_000;

struct Rig {
    system: FakeSystem,
    profiles: Profiles,
    automation: Automation,
    settings: Arc<FakeSettings>,
    env: Arc<FakeEnvironment>,
}

fn rig() -> Rig {
    let system = FakeSystem::new();
    system.clock.advance(START);
    let settings = Arc::new(FakeSettings::new());
    let env = Arc::new(FakeEnvironment::new());
    let engine = TweakEngine::new(
        Arc::new(Catalogue::from_toml(CATALOGUE).expect("catalogue")),
        system.ports(),
        system.journal.clone(),
        system.clock.clone(),
    )
    .with_activity_log(system.activity.clone());
    let profiles = Profiles::new(
        settings.clone(),
        system.activity.clone(),
        system.clock.clone(),
        engine,
        system.processes.clone(),
    );
    let automation = Automation::new(
        settings.clone(),
        system.activity.clone(),
        system.clock.clone(),
        env.clone(),
    );
    Rig {
        system,
        profiles,
        automation,
        settings,
        env,
    }
}

fn value(rig: &Rig, name: &str) -> Option<RegistryValue> {
    rig.system.registry.get(&RegistryValueRef::new(
        RegistryHive::CurrentUser,
        r"Software\VoltTest\Profiles",
        name,
    ))
}

fn is_on(rig: &Rig, name: &str) -> bool {
    value(rig, name) == Some(RegistryValue::Dword(1))
}

fn make(id: &str, tweaks: &[&str]) -> Profile {
    Profile {
        id: id.to_owned(),
        name: id.to_uppercase(),
        tweaks: tweaks
            .iter()
            .map(|id| ProfileTweak {
                id: (*id).to_owned(),
                enabled: true,
            })
            .collect(),
        ..Profile::default()
    }
}

fn save(rig: &Rig, profile: Profile) {
    rig.profiles.save(profile).expect("profile saves");
}

fn switch(rig: &Rig, id: Option<&str>) -> SwitchReport {
    rig.profiles
        .switch(id, Source::Manual, &rig.automation)
        .expect("switch")
}

#[test]
fn the_suggestions_are_listed_until_the_user_saves_and_none_is_active() {
    let rig = rig();
    let names: Vec<String> = rig.profiles.list().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Gaming", "Work", "AI workload", "Battery"]);
    assert!(rig.profiles.saved().is_none());
    assert_eq!(rig.profiles.state(), ProfileState::default());

    let saved = rig
        .profiles
        .save(Profile {
            name: "  Mine ".to_owned(),
            ..Profile::default()
        })
        .expect("saves");
    assert_eq!(saved.id, "profile-1");
    assert_eq!(saved.name, "Mine");
    // Saving writes the whole list, so the suggestions are kept and editable.
    assert_eq!(rig.profiles.list().len(), 5);
    assert!(rig.profiles.delete("gaming").expect("deletes"));
    assert_eq!(rig.profiles.list().len(), 4);
    assert!(!rig.profiles.delete("gaming").expect("deletes"));
}

#[test]
fn a_profile_is_checked_before_it_is_saved() {
    let rig = rig();
    let bad = |profile: Profile| rig.profiles.save(profile).expect_err("refused");
    assert_eq!(
        bad(make("x", &[]).tap(|p| p.name = " ".into())),
        ProfileError::Name
    );
    assert_eq!(
        bad(make("x", &["no-such-tweak"])),
        ProfileError::UnknownTweak("no-such-tweak".to_owned())
    );
    assert_eq!(
        bad(make("x", &["tweak-admin"])),
        ProfileError::NeedsAdministrator("tweak-admin".to_owned())
    );
    assert_eq!(
        bad(make("x", &["tweak-a", "tweak-a"])),
        ProfileError::DuplicateTweak("tweak-a".to_owned())
    );
    for exe in ["game", "C:\\games\\game.exe", ".exe", "a/b.exe"] {
        let trigger = AppTrigger {
            exe: exe.to_owned(),
            return_on_exit: false,
        };
        assert_eq!(
            bad(make("x", &[]).tap(|p| p.trigger = Some(trigger))),
            ProfileError::Trigger,
            "{exe}"
        );
    }
    let memory = MemoryRule {
        cooldown_minutes: 1,
        ..MemoryRule::default()
    };
    assert_eq!(
        bad(make("x", &[]).tap(|p| p.automation.memory = Some(memory))),
        ProfileError::Automation
    );
    assert!(rig.profiles.saved().is_none());
}

trait Tap: Sized {
    fn tap(mut self, change: impl FnOnce(&mut Self)) -> Self {
        change(&mut self);
        self
    }
}
impl Tap for Profile {}

#[test]
fn the_first_switch_previews_and_applies_the_tweaks_through_the_journal() {
    let rig = rig();
    save(&rig, make("one", &["tweak-a", "tweak-c"]));
    let plan = rig
        .profiles
        .plan(Some("one"), &rig.automation)
        .expect("plan");
    assert_eq!(plan.from, None);
    assert_eq!(plan.items.len(), 2);
    assert!(
        plan.items
            .iter()
            .all(|item| matches!(&item.action, SwitchAction::TurnOn(steps) if steps.len() == 1))
    );
    let risky: Vec<&str> = plan.risky().map(|item| item.tweak.as_str()).collect();
    assert_eq!(risky, ["tweak-c"]);
    // The preview changed nothing.
    assert!(!is_on(&rig, "A"));
    assert_eq!(rig.profiles.state().active, None);

    let report = switch(&rig, Some("one"));
    assert!(report.complete);
    assert_eq!(report.changed(), 2);
    assert!(is_on(&rig, "A") && is_on(&rig, "C") && !is_on(&rig, "B"));
    let state = rig.profiles.state();
    assert_eq!(state.active.as_deref(), Some("one"));
    let record = state.last_switch.expect("recorded");
    assert_eq!(record.applied, ["tweak-a", "tweak-c"]);
    assert!(record.reverted.is_empty() && record.complete);
    // One journal batch per tweak, found again by the tweak id like any other apply.
    for id in ["tweak-a", "tweak-c"] {
        assert!(rig.profiles.engine.applied_by_volt(id).expect("journal"));
    }
    assert!(
        rig.system
            .activity
            .records()
            .iter()
            .any(|r| r.label.as_deref() == Some(SWITCH_LABEL)
                && r.outcome == ActivityOutcome::Applied)
    );
}

#[test]
fn switching_applies_and_reverts_only_the_difference() {
    let rig = rig();
    save(&rig, make("one", &["tweak-a", "tweak-b"]));
    save(&rig, make("two", &["tweak-b", "tweak-c"]));
    switch(&rig, Some("one"));
    let batches_before = rig.system.journal.entries().len();

    let plan = rig
        .profiles
        .plan(Some("two"), &rig.automation)
        .expect("plan");
    assert_eq!(plan.from.as_deref(), Some("one"));
    let shown: Vec<(&str, bool)> = plan
        .items
        .iter()
        .map(|item| {
            (
                item.tweak.as_str(),
                matches!(item.action, SwitchAction::TurnOff),
            )
        })
        .collect();
    // A is turned off first, then C on; B is in place and not touched.
    assert_eq!(shown, [("tweak-a", true), ("tweak-c", false)]);
    assert_eq!(plan.in_place, 1);

    let report = switch(&rig, Some("two"));
    assert!(report.complete);
    assert!(!is_on(&rig, "A") && is_on(&rig, "B") && is_on(&rig, "C"));
    assert_eq!(value(&rig, "A"), None, "the value before Volt came back");
    // B was not written again: the new entries are those of A (reverted) and C (applied).
    let touched = rig.system.journal.entries().len() - batches_before;
    assert_eq!(touched, 1, "only C adds an entry; A's entry changes state");
    assert_eq!(rig.profiles.state().active.as_deref(), Some("two"));

    // Switching to the active profile again changes nothing.
    let again = switch(&rig, Some("two"));
    assert!(again.complete && again.steps.is_empty());
}

#[test]
fn turning_a_profile_off_reverts_what_it_applied() {
    let rig = rig();
    save(&rig, make("one", &["tweak-a"]));
    switch(&rig, Some("one"));
    let report = switch(&rig, None);
    assert!(report.complete && report.to.is_none());
    assert!(!is_on(&rig, "A"));
    assert_eq!(rig.profiles.state().active, None);
}

#[test]
fn an_explicit_off_only_reverts_what_volt_applied() {
    let rig = rig();
    let off = Profile {
        tweaks: vec![
            ProfileTweak {
                id: "tweak-a".to_owned(),
                enabled: false,
            },
            ProfileTweak {
                id: "tweak-b".to_owned(),
                enabled: false,
            },
        ],
        ..make("quiet", &[])
    };
    save(&rig, off);
    // A was applied by the user outside Volt: there is no earlier value to put back.
    rig.system.registry.set(
        &RegistryValueRef::new(
            RegistryHive::CurrentUser,
            r"Software\VoltTest\Profiles",
            "A",
        ),
        RegistryValue::Dword(1),
    );
    let plan = rig
        .profiles
        .plan(Some("quiet"), &rig.automation)
        .expect("plan");
    assert!(plan.items.is_empty());
    assert_eq!(plan.left_out.len(), 1);
    assert_eq!(plan.left_out[0].tweak, "tweak-a");
    assert_eq!(plan.left_out[0].reason, LeftOut::NotAppliedByVolt);
    assert!(switch(&rig, Some("quiet")).complete);
    assert!(is_on(&rig, "A"), "left as it was");
}

#[test]
fn a_tweak_that_needs_administrator_rights_is_not_included_with_its_reason() {
    let rig = rig();
    // Written behind the checks, as an older file or catalogue could leave it.
    let stored = vec![make("old", &["tweak-a", "tweak-admin", "gone"])];
    rig.settings.set_typed(LIST_KEY, &stored).expect("store");
    let plan = rig
        .profiles
        .plan(Some("old"), &rig.automation)
        .expect("plan");
    assert_eq!(plan.items.len(), 1);
    let reasons: Vec<(&str, &LeftOut)> = plan
        .left_out
        .iter()
        .map(|left| (left.tweak.as_str(), &left.reason))
        .collect();
    assert_eq!(
        reasons,
        [
            ("tweak-admin", &LeftOut::NeedsAdministrator),
            ("gone", &LeftOut::Unknown)
        ]
    );
    assert!(switch(&rig, Some("old")).complete);
    assert!(
        rig.system
            .registry
            .get(&RegistryValueRef::new(
                RegistryHive::LocalMachine,
                r"Software\VoltTest\Profiles",
                "Admin"
            ))
            .is_none()
    );
}

#[test]
fn a_switch_that_fails_part_way_says_what_was_done_and_leaves_the_journal_consistent() {
    let rig = rig();
    save(&rig, make("one", &["tweak-a", "tweak-b", "tweak-c"]));
    rig.system.registry.fail_on(
        &RegistryValueRef::new(
            RegistryHive::CurrentUser,
            r"Software\VoltTest\Profiles",
            "B",
        ),
        PortError::AccessDenied("test".to_owned()),
    );
    let report = switch(&rig, Some("one"));
    assert!(!report.complete);
    let outcomes: Vec<&StepOutcome> = report.steps.iter().map(|s| &s.outcome).collect();
    assert_eq!(outcomes[0], &StepOutcome::Done);
    assert!(matches!(outcomes[1], StepOutcome::ApplyFailed(_)));
    assert_eq!(outcomes[2], &StepOutcome::NotAttempted);
    assert!(is_on(&rig, "A") && !is_on(&rig, "B") && !is_on(&rig, "C"));
    // Nothing is left half-written, and the active profile did not change.
    assert!(
        rig.system
            .journal
            .entries_in_state(EntryState::Pending)
            .expect("journal")
            .is_empty()
    );
    let state = rig.profiles.state();
    assert_eq!(state.active, None);
    let record = state.last_switch.expect("what was done is kept");
    assert_eq!(record.applied, ["tweak-a"]);
    assert!(!record.complete);

    // Undoing it puts back what it did.
    let undone = rig.profiles.undo_switch(&rig.automation).expect("undo");
    assert!(undone.complete);
    assert!(!is_on(&rig, "A"));
    assert!(rig.profiles.state().last_switch.is_none());

    // Once the cause is gone, switching again finishes the job.
    rig.system.registry.clear_failures();
    assert!(switch(&rig, Some("one")).complete);
    assert!(is_on(&rig, "A") && is_on(&rig, "B") && is_on(&rig, "C"));
}

#[test]
fn undoing_a_switch_puts_everything_back_in_one_go() {
    let rig = rig();
    save(&rig, make("one", &["tweak-a", "tweak-b"]));
    save(&rig, make("two", &["tweak-b", "tweak-c"]));
    switch(&rig, Some("one"));
    switch(&rig, Some("two"));
    assert!(!is_on(&rig, "A") && is_on(&rig, "C"));

    let undone = rig.profiles.undo_switch(&rig.automation).expect("undo");
    assert!(undone.complete);
    assert!(is_on(&rig, "A") && is_on(&rig, "B") && !is_on(&rig, "C"));
    let state = rig.profiles.state();
    assert_eq!(state.active.as_deref(), Some("one"));
    assert!(state.last_switch.is_none());
    assert_eq!(
        rig.profiles.undo_switch(&rig.automation),
        Err(ProfileError::NothingToUndo)
    );
}

#[test]
fn a_profile_sets_only_the_automation_parts_it_names_and_undo_restores_them() {
    let rig = rig();
    let mut mine = make("mine", &[]);
    mine.automation.memory = Some(MemoryRule {
        enabled: true,
        standby_above_mb: 8192,
        ..MemoryRule::default()
    });
    save(&rig, mine);
    let user = AutomationConfig {
        notify: false,
        cleaning: Schedule {
            enabled: true,
            rule_ids: vec!["user-temp".to_owned()],
            ..Schedule::default()
        },
        ..AutomationConfig::default()
    };
    rig.automation.save_config(&user).expect("saves");

    let plan = rig
        .profiles
        .plan(Some("mine"), &rig.automation)
        .expect("plan");
    let change = plan.automation.expect("a change");
    assert!(change.after.memory.enabled && !change.before.memory.enabled);
    assert_eq!(change.after.cleaning, user.cleaning);
    assert!(!change.after.notify);

    let report = switch(&rig, Some("mine"));
    assert_eq!(report.automation, AutomationOutcome::Set);
    assert_eq!(rig.automation.config().memory.standby_above_mb, 8192);
    assert!(rig.automation.config().cleaning.enabled);

    rig.profiles.undo_switch(&rig.automation).expect("undo");
    assert_eq!(rig.automation.config(), user);
}

#[test]
fn an_imported_profile_keeps_only_what_this_machine_knows() {
    let rig = rig();
    let mut wild = make("wild", &["tweak-a", "tweak-admin", "nope", "tweak-a"]);
    wild.name = "x".repeat(100);
    wild.trigger = Some(AppTrigger {
        exe: "..\\evil.exe".to_owned(),
        return_on_exit: true,
    });
    wild.automation.memory = Some(MemoryRule {
        free_below_mb: 1,
        ..MemoryRule::default()
    });
    let (clean, dropped) = rig.profiles.sanitized(wild).expect("usable");
    assert_eq!(clean.tweaks.len(), 1);
    assert_eq!(clean.name.chars().count(), MAX_NAME_CHARS);
    assert_eq!(clean.trigger, None);
    assert_eq!(dropped, 4);
    assert!(
        clean
            .automation
            .memory
            .is_some_and(|m| m.free_below_mb >= 128)
    );
    assert!(rig.profiles.validate(&clean).is_ok());
    assert!(rig.profiles.sanitized(make("Bad Id", &[])).is_none());
}

// ---- automatic switches ---------------------------------------------------------------------

fn triggered(rig: &Rig, id: &str, tweaks: &[&str], exe: &str, back: bool) {
    let mut profile = make(id, tweaks);
    profile.trigger = Some(AppTrigger {
        exe: exe.to_owned(),
        return_on_exit: back,
    });
    save(rig, profile);
}

fn poll(rig: &Rig, pause: Option<PauseReason>) -> Vec<AutoEvent> {
    rig.profiles.poll(&move || pause, &rig.automation)
}

#[test]
fn an_app_that_starts_activates_its_profile_and_its_exit_brings_the_earlier_one_back() {
    let rig = rig();
    save(&rig, make("work", &["tweak-b"]));
    triggered(&rig, "game", &["tweak-a", "tweak-c"], "Game.EXE", true);
    switch(&rig, Some("work"));

    // The first look is a baseline; nothing is running.
    assert!(poll(&rig, None).is_empty());
    assert!(poll(&rig, None).is_empty());
    rig.system.processes.add(10, "game.exe", 1 << 20);
    let events = poll(&rig, None);
    assert!(
        matches!(&events[..], [AutoEvent::Activated(r)] if r.complete && r.to.as_deref() == Some("game"))
    );
    assert!(is_on(&rig, "A") && is_on(&rig, "C") && !is_on(&rig, "B"));
    let state = rig.profiles.state();
    assert_eq!(state.active.as_deref(), Some("game"));
    assert_eq!(
        state.auto_return,
        Some(AutoReturn {
            profile: "game".to_owned(),
            previous: Some("work".to_owned()),
        })
    );
    // The changes are recorded as automatic.
    assert!(
        rig.system
            .journal
            .entries()
            .iter()
            .any(|e| e.source == Source::Automation)
    );
    // Still running: no second switch.
    assert!(poll(&rig, None).is_empty());

    rig.system.processes.terminate(10, 0).expect("exits");
    let events = poll(&rig, None);
    assert!(matches!(&events[..], [AutoEvent::Returned(r)] if r.complete));
    assert!(!is_on(&rig, "A") && !is_on(&rig, "C") && is_on(&rig, "B"));
    let state = rig.profiles.state();
    assert_eq!(state.active.as_deref(), Some("work"));
    assert_eq!(state.auto_return, None);
    assert!(poll(&rig, None).is_empty());
}

#[test]
fn an_app_that_was_running_when_volt_started_does_not_activate_anything() {
    let rig = rig();
    triggered(&rig, "game", &["tweak-a"], "game.exe", false);
    rig.system.processes.add(10, "game.exe", 1);
    assert!(poll(&rig, None).is_empty());
    assert!(poll(&rig, None).is_empty());
    assert!(!is_on(&rig, "A"));
    assert_eq!(rig.profiles.state().active, None);
}

#[test]
fn an_app_in_session_zero_is_not_a_user_app() {
    let rig = rig();
    triggered(&rig, "game", &["tweak-a"], "game.exe", false);
    assert!(poll(&rig, None).is_empty());
    rig.system.processes.add(10, "game.exe", 1);
    rig.system.processes.set_session(10, 0);
    assert!(poll(&rig, None).is_empty());
    assert!(!is_on(&rig, "A"));
}

#[test]
fn a_pause_skips_the_switch_for_that_start_and_it_is_not_queued() {
    let rig = rig();
    triggered(&rig, "game", &["tweak-a"], "game.exe", true);
    assert!(poll(&rig, None).is_empty());
    rig.system.processes.add(10, "game.exe", 1);
    let events = poll(&rig, Some(PauseReason::Fullscreen));
    assert_eq!(
        events,
        [AutoEvent::Skipped {
            profile: "game".to_owned(),
            reason: PauseReason::Fullscreen
        }]
    );
    assert!(!is_on(&rig, "A"));
    assert_eq!(
        rig.profiles.state().last_skip.map(|s| s.reason),
        Some(PauseReason::Fullscreen)
    );
    // The pause ends while the app still runs: it is the same start, so nothing happens.
    assert!(poll(&rig, None).is_empty());
    assert!(!is_on(&rig, "A"));
    // A new start is a new chance.
    rig.system.processes.terminate(10, 0).expect("exits");
    assert!(poll(&rig, None).is_empty());
    rig.system.processes.add(11, "game.exe", 1);
    assert_eq!(poll(&rig, None).len(), 1);
    assert!(is_on(&rig, "A"));
}

#[test]
fn the_real_pause_rules_of_automation_apply() {
    let rig = rig();
    assert_eq!(rig.automation.pause_now(None), None);
    rig.env.set_fullscreen(true);
    assert_eq!(
        rig.automation.pause_now(None),
        Some(PauseReason::Fullscreen)
    );
    rig.env.set_fullscreen(false);
    rig.env.set_battery_saver(true);
    assert_eq!(
        rig.automation.pause_now(None),
        Some(PauseReason::BatterySaver)
    );
}

#[test]
fn choosing_another_profile_by_hand_cancels_the_automatic_return() {
    let rig = rig();
    save(&rig, make("work", &["tweak-b"]));
    triggered(&rig, "game", &["tweak-a"], "game.exe", true);
    assert!(poll(&rig, None).is_empty());
    rig.system.processes.add(10, "game.exe", 1);
    assert_eq!(poll(&rig, None).len(), 1);
    assert!(rig.profiles.state().auto_return.is_some());

    switch(&rig, Some("work"));
    assert_eq!(rig.profiles.state().auto_return, None);
    rig.system.processes.terminate(10, 0).expect("exits");
    assert!(poll(&rig, None).is_empty());
    assert_eq!(rig.profiles.state().active.as_deref(), Some("work"));
}

#[test]
fn nothing_is_read_while_no_profile_names_an_app() {
    let rig = rig();
    save(&rig, make("work", &["tweak-b"]));
    assert!(!rig.profiles.watching());
    let before = rig.system.log.events().len();
    assert!(poll(&rig, None).is_empty());
    assert_eq!(rig.system.log.events().len(), before);
    triggered(&rig, "game", &[], "game.exe", false);
    assert!(rig.profiles.watching());
}
