//! The shipped tweak catalogue (`catalogue/tweaks.toml`) against the in-memory fakes.
//!
//! The generic test here runs over EVERY tweak, so a tweak added to the catalogue is covered the
//! moment it exists: detect, apply, the state reads applied, revert, and the exact previous state
//! is back, including a value that was absent before, a key that was absent before (and its
//! parents) and a key that existed before with other values in it. Nothing touches the real system: the real
//! registry, services and commands are never reached.

use std::collections::HashMap;
use std::sync::Arc;

use volt_core::fakes::FakeSystem;
use volt_core::{
    ApplyOutcome, Catalogue, Category, EntryState, Evidence, Journal, Registry, RegistryHive,
    RegistryKeyRef, RegistryValue, RegistryValueRef, RegistryView, ServiceStartType, Services,
    Source, Tweak, TweakEngine, TweakError, TweakState, TweakStep,
};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;

fn catalogue() -> Arc<Catalogue> {
    Arc::new(Catalogue::from_toml(EMBEDDED_CATALOGUE).expect("the embedded catalogue is valid"))
}

/// A new engine over the machine. Calling it again is a "restart": only the system and the
/// journal carry over.
fn engine(system: &FakeSystem, catalogue: &Arc<Catalogue>, administrator: bool) -> TweakEngine {
    TweakEngine::new(
        catalogue.clone(),
        system.ports(),
        system.journal.clone(),
        system.clock.clone(),
    )
    .with_administrator(administrator)
}

// ---- arranging and observing a fake machine ------------------------------------------------

/// What the machine looks like before the tweak is applied.
#[derive(Clone, Copy, Debug)]
enum Before {
    /// Registry values the tweak sets are absent; values it deletes are present.
    Absent,
    /// Registry values the tweak sets exist with a different value of the same type.
    Different,
    /// Like `Different`, and the keys the tweak makes exist already exist, with another value in
    /// them. Only meaningful for a tweak that has such a key step.
    KeyPresent,
}

/// The value `Before::KeyPresent` leaves in a key the tweak did not put there.
const UNRELATED: &str = "VoltTestUnrelated";

fn target_of(
    hive: RegistryHive,
    subkey: &str,
    name: &str,
    view: volt_core::RegistryView,
) -> RegistryValueRef {
    RegistryValueRef::new(hive, subkey, name).with_view(view)
}

fn different_from(value: &RegistryValue) -> RegistryValue {
    match value {
        RegistryValue::Dword(n) => RegistryValue::Dword(n.wrapping_add(1)),
        RegistryValue::Qword(n) => RegistryValue::Qword(n.wrapping_add(1)),
        RegistryValue::Sz(text) => RegistryValue::Sz(format!("{text}-old")),
        RegistryValue::ExpandSz(text) => RegistryValue::ExpandSz(format!("{text}-old")),
        RegistryValue::MultiSz(lines) => {
            RegistryValue::MultiSz(lines.iter().map(|line| format!("{line}-old")).collect())
        }
        RegistryValue::Binary(bytes) => {
            RegistryValue::Binary(bytes.iter().map(|b| b.wrapping_add(1)).collect())
        }
    }
}

fn other_start_type(target: ServiceStartType) -> ServiceStartType {
    if target == ServiceStartType::Automatic {
        ServiceStartType::Manual
    } else {
        ServiceStartType::Automatic
    }
}

/// Puts the fake machine into the "before" state of `tweak`: every step is NOT at its target.
fn arrange(system: &FakeSystem, tweak: &Tweak, before: Before) {
    for step in &tweak.steps {
        match step {
            TweakStep::RegistrySet {
                hive,
                subkey,
                name,
                view,
                value,
                ..
            } => {
                if matches!(before, Before::Different | Before::KeyPresent) {
                    system.registry.set(
                        &target_of(*hive, subkey, name, *view),
                        different_from(value),
                    );
                }
            }
            TweakStep::RegistryDelete {
                hive,
                subkey,
                name,
                view,
                ..
            } => system.registry.set(
                &target_of(*hive, subkey, name, *view),
                RegistryValue::Dword(7),
            ),
            TweakStep::RegistryKey {
                hive,
                subkey,
                view,
                exists,
            } => {
                // A key that has to be absent is present, with a value, before.
                if !*exists || matches!(before, Before::KeyPresent) {
                    system.registry.set(
                        &target_of(*hive, subkey, UNRELATED, *view),
                        RegistryValue::Dword(9),
                    );
                }
            }
            TweakStep::Service {
                service,
                start_type,
                ..
            } => system.services.add(service, other_start_type(*start_type)),
            // A new step kind has no arrangement here: the test must learn about it, not skip it.
            other => panic!("{}: no test arrangement for step {other:?}", tweak.id),
        }
    }
}

/// Puts the fake machine into the state the tweak leaves behind, as if it were set outside Volt.
fn arrange_target(system: &FakeSystem, tweak: &Tweak) {
    for step in &tweak.steps {
        match step {
            TweakStep::RegistrySet {
                hive,
                subkey,
                name,
                view,
                value,
                ..
            } => system
                .registry
                .set(&target_of(*hive, subkey, name, *view), value.clone()),
            // Absent is the target of a delete step; a key is made by the key step.
            TweakStep::RegistryDelete { .. } => {}
            TweakStep::RegistryKey {
                hive,
                subkey,
                view,
                exists,
            } => {
                if *exists {
                    system.registry.add_key(*hive, subkey, *view);
                }
            }
            TweakStep::Service {
                service,
                start_type,
                ..
            } => system.services.add(service, *start_type),
            other => panic!("{}: no test arrangement for step {other:?}", tweak.id),
        }
    }
}

/// What a tweak's steps look like on the machine, step by step.
#[derive(Debug, PartialEq, Eq)]
enum Observed {
    /// A value, and whether each level of its key path exists (so a key a write created and a
    /// revert left behind shows).
    Registry(Option<RegistryValue>, Vec<bool>),
    /// A key: its values (`None` when it does not exist) and the same per-level existence.
    Key(Option<Vec<(String, RegistryValue)>>, Vec<bool>),
    Service(Option<ServiceStartType>),
}

/// For every level of `subkey` (`A`, `A\B`, `A\B\C`) whether that key exists.
fn levels(system: &FakeSystem, hive: RegistryHive, subkey: &str, view: RegistryView) -> Vec<bool> {
    let mut path = String::new();
    subkey
        .split('\\')
        .map(|part| {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(part);
            system
                .registry
                .key_exists(hive, &path, view)
                .expect("the fake answers")
        })
        .collect()
}

fn observe(system: &FakeSystem, tweak: &Tweak) -> Vec<Observed> {
    tweak
        .steps
        .iter()
        .map(|step| match step {
            TweakStep::RegistrySet {
                hive,
                subkey,
                name,
                view,
                ..
            }
            | TweakStep::RegistryDelete {
                hive,
                subkey,
                name,
                view,
                ..
            } => Observed::Registry(
                system.registry.get(&target_of(*hive, subkey, name, *view)),
                levels(system, *hive, subkey, *view),
            ),
            TweakStep::RegistryKey {
                hive, subkey, view, ..
            } => {
                let key = RegistryKeyRef::new(*hive, subkey.as_str(), *view);
                let exists = system
                    .registry
                    .key_exists(*hive, subkey, *view)
                    .expect("the fake answers");
                let values = exists.then(|| {
                    system
                        .registry
                        .list_values(&key)
                        .expect("the fake lists")
                        .into_iter()
                        .map(|named| (named.name, named.value.expect("readable")))
                        .collect()
                });
                Observed::Key(values, levels(system, *hive, subkey, *view))
            }
            TweakStep::Service { service, .. } => {
                Observed::Service(system.services.start_type(service).ok())
            }
            other => panic!("{}: no test observation for step {other:?}", tweak.id),
        })
        .collect()
}

// ---- the generic test ----------------------------------------------------------------------

#[test]
fn every_tweak_applies_reads_as_applied_and_reverts_to_the_exact_previous_state() {
    let catalogue = catalogue();
    for tweak in catalogue.tweaks() {
        let has_key_step = tweak
            .steps
            .iter()
            .any(|step| matches!(step, TweakStep::RegistryKey { exists: true, .. }));
        for before in [Before::Absent, Before::Different, Before::KeyPresent] {
            if matches!(before, Before::KeyPresent) && !has_key_step {
                continue;
            }
            let id = tweak.id.as_str();
            let context = format!("{id} ({before:?})");
            let system = FakeSystem::new();
            arrange(&system, tweak, before);
            let previous = observe(&system, tweak);

            // A key that already exists (a value in it puts it there) is a step in its target
            // state, before the apply and again after the revert.
            let state_before = if has_key_step && !matches!(before, Before::Absent) {
                TweakState::Partial
            } else {
                TweakState::NotApplied
            };
            let engine = self::engine(&system, &catalogue, true);
            assert_eq!(
                engine.state(id),
                Ok(state_before),
                "{context}: detect before"
            );
            let plan = engine.plan(id).expect("dry run");
            assert!(!plan.is_empty(), "{context}: the dry run lists the changes");
            assert!(
                system.log.events().is_empty() && system.journal.entries().is_empty(),
                "{context}: detect and the dry run change nothing and journal nothing"
            );

            let applied = engine.apply(id, Source::Manual).expect("apply");
            assert_eq!(applied.outcome, ApplyOutcome::Applied, "{context}");
            assert_eq!(
                engine.state(id),
                Ok(TweakState::Applied),
                "{context}: after apply"
            );
            assert_ne!(
                observe(&system, tweak),
                previous,
                "{context}: something changed"
            );
            assert_eq!(
                engine
                    .apply(id, Source::Manual)
                    .expect("apply again")
                    .outcome,
                ApplyOutcome::AlreadyApplied,
                "{context}"
            );
            // Volt applied it, so undo is the way back; a reset would only guess.
            let applied_state = observe(&system, tweak);
            assert_eq!(
                engine.reset(id, Source::Manual).unwrap_err(),
                TweakError::AppliedByVolt(id.to_owned()),
                "{context}"
            );
            assert_eq!(
                observe(&system, tweak),
                applied_state,
                "{context}: the refused reset changed nothing"
            );

            // A restart in between: a new engine over the same journal still reverts it.
            let engine = self::engine(&system, &catalogue, true);
            let reverted = engine.revert(id).expect("revert");
            assert!(reverted.is_complete(), "{context}: revert is complete");
            assert_eq!(
                engine.state(id),
                Ok(state_before),
                "{context}: after revert"
            );
            assert_eq!(
                observe(&system, tweak),
                previous,
                "{context}: the exact previous state is back"
            );
            assert!(
                system
                    .journal
                    .entries()
                    .iter()
                    .all(|entry| entry.state == EntryState::Reverted),
                "{context}: every journal entry is reverted"
            );
        }
    }
}

#[test]
fn every_tweak_set_outside_volt_is_turned_off_to_its_declared_windows_default() {
    let catalogue = catalogue();
    let mut reset_count = 0;
    for tweak in catalogue.tweaks() {
        let id = tweak.id.as_str();
        let system = FakeSystem::new();
        arrange_target(&system, tweak);
        let target_state = observe(&system, tweak);
        let engine = engine(&system, &catalogue, true);
        assert_eq!(engine.state(id), Ok(TweakState::Applied), "{id}: set");
        assert_eq!(engine.applied_by_volt(id), Ok(false), "{id}");
        let Some(plan) = engine.reset_plan(id).expect("plan") else {
            continue;
        };
        reset_count += 1;
        assert!(!plan.is_empty(), "{id}: the reset plan lists the changes");

        let report = engine.reset(id, Source::Manual).expect("reset");
        assert_eq!(report.outcome, ApplyOutcome::Applied, "{id}");
        assert_eq!(engine.state(id), Ok(TweakState::NotApplied), "{id}: off");
        for step in &tweak.steps {
            match step {
                TweakStep::RegistrySet {
                    hive,
                    subkey,
                    name,
                    view,
                    off,
                    ..
                } => assert_eq!(
                    system.registry.get(&target_of(*hive, subkey, name, *view)),
                    off.clone(),
                    "{id}: {name} is the declared default, or removed without one"
                ),
                TweakStep::RegistryKey {
                    hive, subkey, view, ..
                } => assert_eq!(
                    system.registry.key_exists(*hive, subkey, *view),
                    Ok(false),
                    "{id}: the key is removed"
                ),
                TweakStep::Service { service, off, .. } => assert_eq!(
                    system.services.start_type(service).ok(),
                    *off,
                    "{id}: {service}"
                ),
                other => panic!("{id}: unexpected step {other:?}"),
            }
        }
        // Volt did not apply it, so there is nothing to revert; the reset is its own batch.
        assert_eq!(
            engine.revert(id),
            Err(TweakError::NothingToRevert(id.to_owned())),
            "{id}"
        );
        // And the journal brings the tweak back exactly as it was set.
        let batches = system
            .journal
            .batches_with_label(&format!("{id}:reset"))
            .expect("batches");
        assert_eq!(batches.len(), 1, "{id}");
        let undone = engine
            .executor()
            .revert_batch(batches[0], Source::Manual, None)
            .expect("undo the reset");
        assert!(undone.is_complete(), "{id}");
        assert_eq!(engine.state(id), Ok(TweakState::Applied), "{id}: set again");
        assert_eq!(observe(&system, tweak), target_state, "{id}");
    }
    assert_eq!(
        reset_count,
        catalogue.len(),
        "every shipped tweak can be turned off"
    );
}

#[test]
fn the_classic_context_menu_is_a_key_that_must_exist_with_an_empty_default_value() {
    let catalogue = catalogue();
    let tweak = catalogue.get("classic-context-menu").expect("shipped");
    assert_eq!(tweak.category, Category::Appearance);
    assert!(!tweak.requires_admin, "it is a per-user change");
    assert_eq!(tweak.restart, volt_core::RestartRequirement::SignOut);
    assert!(
        tweak
            .description
            .to_lowercase()
            .contains("restart file explorer"),
        "it says that Explorer has to be restarted"
    );
    assert!(
        tweak.evidence_note.contains("not documented by Microsoft"),
        "the evidence note is honest about where the claim comes from"
    );
    let [
        TweakStep::RegistryKey {
            hive,
            subkey,
            exists: true,
            ..
        },
        TweakStep::RegistrySet {
            subkey: value_subkey,
            name,
            value,
            ..
        },
    ] = tweak.steps.as_slice()
    else {
        panic!("a key step and the default value in it: {:?}", tweak.steps);
    };
    assert_eq!(*hive, RegistryHive::CurrentUser);
    assert!(subkey.ends_with(r"{86ca1aa0-34aa-4e8b-a509-50c905bae2a2}\InprocServer32"));
    assert_eq!(subkey, value_subkey);
    assert_eq!(name, "");
    assert_eq!(*value, RegistryValue::Sz(String::new()));
}

#[test]
fn all_tweaks_applied_together_revert_to_the_original_machine() {
    let catalogue = catalogue();
    let system = FakeSystem::new();
    for (index, tweak) in catalogue.tweaks().iter().enumerate() {
        arrange(
            &system,
            tweak,
            if index % 2 == 0 {
                Before::Absent
            } else {
                Before::Different
            },
        );
    }
    let previous: Vec<_> = catalogue
        .tweaks()
        .iter()
        .map(|tweak| observe(&system, tweak))
        .collect();

    let engine = engine(&system, &catalogue, true);
    for tweak in catalogue.tweaks() {
        engine.apply(&tweak.id, Source::Manual).expect("apply");
    }
    for tweak in catalogue.tweaks() {
        assert_eq!(
            engine.state(&tweak.id),
            Ok(TweakState::Applied),
            "{}",
            tweak.id
        );
    }
    for tweak in catalogue.tweaks().iter().rev() {
        assert!(engine.revert(&tweak.id).expect("revert").is_complete());
    }
    let after: Vec<_> = catalogue
        .tweaks()
        .iter()
        .map(|tweak| observe(&system, tweak))
        .collect();
    assert_eq!(after, previous);
}

#[test]
fn a_tweak_that_needs_administrator_rights_is_refused_before_anything_is_touched() {
    let catalogue = catalogue();
    let admin_only: Vec<_> = catalogue
        .tweaks()
        .iter()
        .filter(|tweak| tweak.requires_admin)
        .collect();
    assert!(
        admin_only.len() >= 4,
        "the catalogue has machine-wide tweaks"
    );
    for tweak in admin_only {
        let system = FakeSystem::new();
        arrange(&system, tweak, Before::Different);
        let previous = observe(&system, tweak);
        let engine = engine(&system, &catalogue, false);

        assert_eq!(
            engine.apply(&tweak.id, Source::Manual).unwrap_err(),
            TweakError::NeedsAdministrator(tweak.id.clone()),
            "{}",
            tweak.id
        );
        assert_eq!(
            engine.revert(&tweak.id).unwrap_err(),
            TweakError::NeedsAdministrator(tweak.id.clone()),
            "{}",
            tweak.id
        );
        assert_eq!(observe(&system, tweak), previous, "{}", tweak.id);
        assert!(
            system.log.events().is_empty() && system.journal.entries().is_empty(),
            "{}: nothing was changed or journaled",
            tweak.id
        );
        // The screen can still show the state.
        assert_eq!(
            engine.state(&tweak.id),
            Ok(TweakState::NotApplied),
            "{}",
            tweak.id
        );
    }
}

// ---- the shape of the shipped catalogue ----------------------------------------------------

#[test]
fn the_catalogue_offers_about_thirty_tweaks_in_the_three_starter_categories() {
    let catalogue = catalogue();
    assert!(catalogue.len() >= 30, "{} tweaks", catalogue.len());
    for category in [
        Category::Performance,
        Category::Privacy,
        Category::Appearance,
    ] {
        let count = catalogue
            .tweaks()
            .iter()
            .filter(|tweak| tweak.category == category)
            .count();
        assert!(count >= 5, "{category:?} has {count} tweaks");
    }
    assert!(
        catalogue.tweaks().iter().all(|tweak| matches!(
            tweak.category,
            Category::Performance | Category::Privacy | Category::Appearance
        )),
        "the starter catalogue stays within Performance, Privacy and Appearance"
    );
}

#[test]
fn every_tweak_explains_itself_and_says_what_it_needs() {
    for tweak in catalogue().tweaks() {
        let id = &tweak.id;
        assert!(
            tweak.description.chars().count() >= 80,
            "{id}: the description must say what changes and what the user notices"
        );
        assert!(
            tweak.evidence_note.chars().count() >= 40,
            "{id}: the evidence note must say where the claim comes from"
        );
        // Volt measured none of these; a tweak may only say "measured" once it really was.
        assert_eq!(tweak.evidence, Evidence::Anecdotal, "{id}");
        let description = tweak.description.to_lowercase();
        match tweak.restart {
            volt_core::RestartRequirement::SignOut => {
                assert!(
                    description.contains("sign out"),
                    "{id}: say that a sign-out is needed"
                );
            }
            volt_core::RestartRequirement::Reboot => assert!(
                description.contains("restart your pc") || description.contains("next shutdown"),
                "{id}: say that a restart is needed"
            ),
            volt_core::RestartRequirement::None => {}
        }
    }
}

#[test]
fn a_tweak_needs_administrator_rights_exactly_when_it_changes_the_whole_machine() {
    for tweak in catalogue().tweaks() {
        let machine_wide = tweak.steps.iter().any(|step| match step {
            TweakStep::RegistrySet { hive, .. }
            | TweakStep::RegistryDelete { hive, .. }
            | TweakStep::RegistryKey { hive, .. } => *hive != RegistryHive::CurrentUser,
            _ => true,
        });
        assert_eq!(
            tweak.requires_admin, machine_wide,
            "{}: requires_admin must match what its steps touch",
            tweak.id
        );
    }
}

#[test]
fn no_two_tweaks_change_the_same_thing() {
    let mut owner: HashMap<String, String> = HashMap::new();
    for tweak in catalogue().tweaks() {
        for step in &tweak.steps {
            let key = match step {
                TweakStep::RegistrySet {
                    hive,
                    subkey,
                    name,
                    view,
                    ..
                }
                | TweakStep::RegistryDelete {
                    hive,
                    subkey,
                    name,
                    view,
                    ..
                } => format!("{hive:?}|{view:?}|{subkey}|{name}"),
                TweakStep::RegistryKey {
                    hive, subkey, view, ..
                } => format!("key|{hive:?}|{view:?}|{subkey}"),
                TweakStep::Service { service, .. } => format!("service|{service}"),
                other => panic!("{}: unexpected step {other:?}", tweak.id),
            }
            .to_lowercase();
            if let Some(first) = owner.insert(key.clone(), tweak.id.clone()) {
                assert_eq!(first, tweak.id, "`{key}` is changed by both tweaks");
            }
        }
    }
}

#[test]
fn nothing_weakens_security_and_nothing_runs_a_command() {
    // Defender, UAC, SmartScreen, Windows Update and the firewall are not for a tweak catalogue.
    const FORBIDDEN_KEYS: &[&str] = &[
        "defender",
        "currentversion\\policies\\system",
        "smartscreen",
        "windowsupdate",
        "firewall",
        "securitycenter",
        r"control\lsa",
    ];
    const FORBIDDEN_SERVICES: &[&str] = &[
        "windefend",
        "wuauserv",
        "usosvc",
        "mpssvc",
        "bits",
        "sense",
        "wscsvc",
        "wdnissvc",
    ];
    for tweak in catalogue().tweaks() {
        for step in &tweak.steps {
            match step {
                TweakStep::RegistrySet { subkey, .. }
                | TweakStep::RegistryDelete { subkey, .. }
                | TweakStep::RegistryKey { subkey, .. } => {
                    let subkey = subkey.to_lowercase();
                    for forbidden in FORBIDDEN_KEYS {
                        assert!(
                            !subkey.contains(forbidden),
                            "{}: touches `{subkey}`",
                            tweak.id
                        );
                    }
                }
                TweakStep::Service { service, .. } => assert!(
                    !FORBIDDEN_SERVICES.contains(&service.to_lowercase().as_str()),
                    "{}: touches the security service `{service}`",
                    tweak.id
                ),
                other => panic!(
                    "{}: the starter catalogue has no command steps, found {other:?}",
                    tweak.id
                ),
            }
        }
    }
}
