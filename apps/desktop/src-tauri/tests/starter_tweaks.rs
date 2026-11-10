//! The shipped tweak catalogue (`catalogue/tweaks.toml`) against the in-memory fakes.
//!
//! The generic test here runs over EVERY tweak, so a tweak added to the catalogue is covered the
//! moment it exists: detect, apply, the state reads applied, revert, and the exact previous state
//! is back, including a value that was absent before. Nothing touches the real system: the real
//! registry, services and commands are never reached.

use std::collections::HashMap;
use std::sync::Arc;

use volt_core::fakes::FakeSystem;
use volt_core::{
    ApplyOutcome, Catalogue, Category, EntryState, Evidence, RegistryHive, RegistryValue,
    RegistryValueRef, ServiceStartType, Services, Source, Tweak, TweakEngine, TweakError,
    TweakState, TweakStep,
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
}

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
            } => {
                if matches!(before, Before::Different) {
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
            } => system.registry.set(
                &target_of(*hive, subkey, name, *view),
                RegistryValue::Dword(7),
            ),
            TweakStep::Service {
                service,
                start_type,
            } => system.services.add(service, other_start_type(*start_type)),
            // A new step kind has no arrangement here: the test must learn about it, not skip it.
            other => panic!("{}: no test arrangement for step {other:?}", tweak.id),
        }
    }
}

/// What a tweak's steps look like on the machine, step by step.
#[derive(Debug, PartialEq, Eq)]
enum Observed {
    Registry(Option<RegistryValue>),
    Service(Option<ServiceStartType>),
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
            } => Observed::Registry(system.registry.get(&target_of(*hive, subkey, name, *view))),
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
        for before in [Before::Absent, Before::Different] {
            let id = tweak.id.as_str();
            let context = format!("{id} ({before:?})");
            let system = FakeSystem::new();
            arrange(&system, tweak, before);
            let previous = observe(&system, tweak);

            let engine = self::engine(&system, &catalogue, true);
            assert_eq!(
                engine.state(id),
                Ok(TweakState::NotApplied),
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

            // A restart in between: a new engine over the same journal still reverts it.
            let engine = self::engine(&system, &catalogue, true);
            let reverted = engine.revert(id).expect("revert");
            assert!(reverted.is_complete(), "{context}: revert is complete");
            assert_eq!(
                engine.state(id),
                Ok(TweakState::NotApplied),
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
            TweakStep::RegistrySet { hive, .. } | TweakStep::RegistryDelete { hive, .. } => {
                *hive != RegistryHive::CurrentUser
            }
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
                } => format!("{hive:?}|{view:?}|{subkey}|{name}"),
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
                | TweakStep::RegistryDelete { subkey, .. } => {
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
