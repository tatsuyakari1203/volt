//! The build check for administrator tweaks.
//!
//! The elevated helper runs a tweak with administrator rights, so what such a tweak may contain is
//! narrower than what an ordinary tweak may contain (see the decision on the helper):
//!
//! - no command step: an elevated process starts no program, whatever the catalogue says;
//! - no registry step outside `HKLM`: the per-user hive (`HKCU`, `HKU`) and the merged view
//!   (`HKCR`, which reads and writes the user's classes) are the user's, and an elevated process
//!   must never be steered into them. `HKCC` is left out too: nothing needs it.
//!
//! [`Catalogue::administrator_issues`] reports every violation, so a catalogue that breaks the rule
//! fails the tests of every place that loads the embedded catalogue, and the helper refuses to
//! serve its tweaks at all.

use super::catalogue::{Catalogue, CatalogueIssue, IssueKind};
use super::definition::TweakStep;
use crate::registry::RegistryHive;

impl Catalogue {
    /// The problems of the tweaks that require administrator rights: a command step, or a
    /// registry step outside `HKLM`. Empty when the catalogue is fit for the elevated helper.
    /// Tweaks that do not require administrator rights are not looked at.
    #[must_use]
    pub fn administrator_issues(&self) -> Vec<CatalogueIssue> {
        let mut issues = Vec::new();
        for (index, tweak) in self.tweaks().iter().enumerate() {
            if !tweak.requires_admin {
                continue;
            }
            for (step, definition) in tweak.steps.iter().enumerate() {
                let problem = match definition {
                    TweakStep::Command { .. } => Some(
                        "a tweak that requires administrator rights must not have a command step: \
                         the elevated helper starts no program",
                    ),
                    TweakStep::RegistrySet { hive, .. }
                    | TweakStep::RegistryDelete { hive, .. }
                    | TweakStep::RegistryKey { hive, .. }
                        if *hive != RegistryHive::LocalMachine =>
                    {
                        Some(
                            "a tweak that requires administrator rights may only change HKLM: a \
                             per-user or merged registry hive is the user's, not the helper's",
                        )
                    }
                    _ => None,
                };
                if let Some(message) = problem {
                    issues.push(CatalogueIssue {
                        position: Some(index + 1),
                        tweak: Some(tweak.id.clone()),
                        field: format!("steps[{step}]"),
                        kind: IssueKind::AdministratorStep(message.to_owned()),
                    });
                }
            }
        }
        issues
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tweak(admin: bool, step: &str) -> String {
        format!(
            r#"
[[tweak]]
id = "sample"
title = "Sample"
description = "A sample."
category = "privacy"
risk = "safe"
evidence = "measured"
evidence_note = "Test."
requires_admin = {admin}

[[tweak.steps]]
{step}
"#
        )
    }

    const HKLM_STEP: &str = r#"kind = "registry_set"
hive = "HKLM"
subkey = 'SOFTWARE\VoltTest'
name = "A"
value = { dword = 1 }"#;
    const SERVICE_STEP: &str = r#"kind = "service"
service = "Spooler"
start_type = "disabled""#;

    fn registry_step(kind: &str, hive: &str) -> String {
        let tail = match kind {
            "registry_set" => "name = \"A\"\nvalue = { dword = 1 }",
            "registry_delete" => "name = \"A\"",
            _ => "exists = true",
        };
        format!("kind = \"{kind}\"\nhive = \"{hive}\"\nsubkey = 'Software\\Example'\n{tail}")
    }

    fn issues(text: &str) -> Vec<CatalogueIssue> {
        Catalogue::from_toml(text)
            .expect("the catalogue itself is valid")
            .administrator_issues()
    }

    #[test]
    fn machine_registry_and_service_steps_are_fine_for_an_administrator_tweak() {
        assert!(issues(&tweak(true, HKLM_STEP)).is_empty());
        assert!(issues(&tweak(true, SERVICE_STEP)).is_empty());
    }

    #[test]
    fn a_command_step_in_an_administrator_tweak_is_a_problem() {
        let command = r#"kind = "command"
detect = { program = "powercfg", args = ["/query"] }
apply = { program = "powercfg", args = ["/hibernate", "off"] }
revert = { program = "powercfg", args = ["/hibernate", "on"] }"#;
        let found = issues(&tweak(true, command));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].tweak.as_deref(), Some("sample"));
        assert_eq!(found[0].field, "steps[0]");
        assert!(matches!(found[0].kind, IssueKind::AdministratorStep(_)));
        assert!(found[0].to_string().contains("command step"));
    }

    #[test]
    fn a_per_user_or_merged_registry_step_in_an_administrator_tweak_is_a_problem() {
        for hive in ["HKCU", "HKU", "HKCR", "HKCC"] {
            for kind in ["registry_set", "registry_delete", "registry_key"] {
                let step = registry_step(kind, hive);
                let found = issues(&tweak(true, &step));
                assert_eq!(found.len(), 1, "{hive}: {step}");
                assert!(found[0].to_string().contains("HKLM"), "{}", found[0]);
            }
        }
    }

    #[test]
    fn the_same_steps_are_fine_in_a_tweak_that_does_not_need_administrator_rights() {
        let step = registry_step("registry_set", "HKCU");
        assert!(issues(&tweak(false, &step)).is_empty());
    }

    #[test]
    fn every_violation_is_reported_with_its_step() {
        let text = format!(
            "{}\n[[tweak.steps]]\n{}",
            tweak(true, HKLM_STEP),
            registry_step("registry_key", "HKCU")
        );
        let found = issues(&text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].field, "steps[1]");
    }
}
