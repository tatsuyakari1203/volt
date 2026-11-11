//! Catalogue loading and the tweak engine against the in-memory fakes. Nothing
//! here touches the real system.

use std::fmt::Write as _;
use std::sync::Arc;

use super::*;
use crate::command::Command;
use crate::error::{ExecError, PortError};
use crate::executor::ItemStatus;
use crate::fakes::{Event, FakeSystem};
use crate::journal::{EntryState, Journal, Source};
use crate::registry::{
    Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef, RegistryView,
};
use crate::service::ServiceStartType;
use crate::{Evidence, Risk, Services};

// ---- fixtures -----------------------------------------------------------------------------

/// One tweak with every step kind.
const SAMPLE: &str = r#"
[[tweak]]
id = "sample-tweak"
title = "Sample tweak"
description = "Touches one of everything."
category = "performance"
risk = "caution"
evidence = "measured"
evidence_note = "Benchmarked on a test machine."
restart = "reboot"

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "Flag"
value = { dword = 0 }

[[tweak.steps]]
kind = "registry_delete"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "Legacy"

[[tweak.steps]]
kind = "service"
service = "SampleSvc"
start_type = "disabled"

[[tweak.steps]]
kind = "command"
detect = { program = "sample-tool", args = ["--is-enabled"] }
apply = { program = "sample-tool", args = ["--enable"] }
revert = { program = "sample-tool", args = ["--disable"] }
"#;

fn registry_ref(name: &str) -> RegistryValueRef {
    RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\VoltTest\Tweaks", name)
}

fn detect() -> Command {
    Command::new("sample-tool", ["--is-enabled"])
}

fn apply_cmd() -> Command {
    Command::new("sample-tool", ["--enable"])
}

fn revert_cmd() -> Command {
    Command::new("sample-tool", ["--disable"])
}

/// A fake machine in its "before" state: Flag = 1, Legacy = 7, the service
/// automatic and the sample effect off.
fn machine() -> FakeSystem {
    let system = FakeSystem::new();
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(1));
    system
        .registry
        .set(&registry_ref("Legacy"), RegistryValue::Dword(7));
    system
        .services
        .add("SampleSvc", ServiceStartType::Automatic);
    system
        .commands
        .add_toggle(detect(), apply_cmd(), revert_cmd(), false);
    system
}

fn catalogue(toml: &str) -> Arc<Catalogue> {
    Arc::new(Catalogue::from_toml(toml).expect("catalogue loads"))
}

/// A new engine over the machine. Calling it again is a "restart": nothing is
/// shared with the earlier engine except the system and the journal.
fn engine(system: &FakeSystem, catalogue: &Arc<Catalogue>) -> TweakEngine {
    TweakEngine::new(
        catalogue.clone(),
        system.ports(),
        system.journal.clone(),
        system.clock.clone(),
    )
}

fn assert_before(system: &FakeSystem) {
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(1))
    );
    assert_eq!(
        system.registry.get(&registry_ref("Legacy")),
        Some(RegistryValue::Dword(7))
    );
    assert_eq!(
        system.services.start_type("SampleSvc"),
        Ok(ServiceStartType::Automatic)
    );
    assert_eq!(system.commands.is_on(&detect()), Some(false));
}

fn assert_after(system: &FakeSystem) {
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(system.registry.get(&registry_ref("Legacy")), None);
    assert_eq!(
        system.services.start_type("SampleSvc"),
        Ok(ServiceStartType::Disabled)
    );
    assert_eq!(system.commands.is_on(&detect()), Some(true));
}

// ---- loading ------------------------------------------------------------------------------

#[test]
fn a_valid_definition_loads_with_every_field() {
    let catalogue = catalogue(SAMPLE);
    assert_eq!(catalogue.len(), 1);
    let tweak = catalogue.get("sample-tweak").expect("tweak");
    assert_eq!(tweak.title, "Sample tweak");
    assert_eq!(tweak.description, "Touches one of everything.");
    assert_eq!(tweak.category, Category::Performance);
    assert_eq!(tweak.risk, Risk::Caution);
    assert_eq!(tweak.evidence, Evidence::Measured);
    assert_eq!(tweak.evidence_note, "Benchmarked on a test machine.");
    assert_eq!(tweak.restart, RestartRequirement::Reboot);
    assert_eq!(tweak.steps.len(), 4);
    assert_eq!(
        tweak.steps[0],
        TweakStep::RegistrySet {
            hive: RegistryHive::CurrentUser,
            subkey: r"Software\VoltTest\Tweaks".to_owned(),
            name: "Flag".to_owned(),
            view: crate::RegistryView::Native,
            value: RegistryValue::Dword(0),
            off: None,
        }
    );
    assert!(matches!(tweak.steps[3], TweakStep::Command { .. }));
    assert!(catalogue.get("missing").is_none());
}

#[test]
fn an_empty_file_is_an_empty_catalogue() {
    assert!(Catalogue::from_toml("").expect("loads").is_empty());
}

#[test]
fn restart_defaults_to_none_and_names_are_accepted_in_both_spellings() {
    let toml = r#"
[[tweak]]
id = "spelling"
title = "T"
description = "D"
category = "windows_update"
risk = "Advanced"
evidence = "anecdotal"
evidence_note = "N"
[[tweak.steps]]
kind = "registry_set"
hive = "LocalMachine"
subkey = 'SOFTWARE\X'
name = ""
view = "bit64"
value = { Sz = "text" }
"#;
    let catalogue = catalogue(toml);
    let tweak = catalogue.get("spelling").expect("tweak");
    assert_eq!(tweak.restart, RestartRequirement::None);
    assert_eq!(tweak.category, Category::WindowsUpdate);
    assert_eq!(tweak.risk, Risk::Advanced);
}

#[test]
fn the_serialized_form_of_a_tweak_loads_back_unchanged() {
    // Guards against the TOML reader and the serialized model drifting apart.
    let original = catalogue(SAMPLE);
    let tweak = toml::Value::try_from(original.get("sample-tweak").expect("tweak"))
        .expect("serializes to TOML data");
    let mut root = toml::Table::new();
    root.insert("tweak".to_owned(), toml::Value::Array(vec![tweak]));
    let reloaded = Catalogue::from_root(&root).expect("reloads");
    assert_eq!(reloaded, *original);
}

// ---- validation ---------------------------------------------------------------------------

fn first_issue(toml: &str) -> CatalogueIssue {
    let error = Catalogue::from_toml(toml).expect_err("must be rejected");
    assert_eq!(error.issues.len(), 1, "expected one issue: {error}");
    error.issues[0].clone()
}

/// A tweak whose body is `extra` appended to valid header fields.
fn tweak_with(header_overrides: &[(&str, &str)], steps: &str) -> String {
    let mut fields = vec![
        ("id", "\"t\""),
        ("title", "\"T\""),
        ("description", "\"D\""),
        ("category", "\"privacy\""),
        ("risk", "\"safe\""),
        ("evidence", "\"measured\""),
        ("evidence_note", "\"N\""),
    ];
    for &(key, value) in header_overrides {
        fields.retain(|&(k, _)| k != key);
        if !value.is_empty() {
            fields.push((key, value));
        }
    }
    let mut text = String::from("[[tweak]]\n");
    for (key, value) in fields {
        let _ = writeln!(text, "{key} = {value}");
    }
    text.push_str(steps);
    text.push('\n');
    text
}

const GOOD_STEP: &str = "[[tweak.steps]]\nkind = \"registry_delete\"\nhive = \"HKCU\"\nsubkey = 'Software\\X'\nname = \"v\"\n";

fn command_step_with_timeout(value: &str) -> String {
    tweak_with(
        &[],
        &format!(
            "[[tweak.steps]]
kind = \"command\"
             detect = {{ program = \"p\", args = [] }}
             apply = {{ program = \"p\", args = [\"on\"], timeout_secs = {value} }}
             revert = {{ program = \"p\", args = [\"off\"] }}
"
        ),
    )
}

#[test]
fn a_command_may_state_its_own_timeout_up_to_the_fixed_bound() {
    for ok in [1, 120, crate::MAX_TIMEOUT_SECS] {
        let catalogue =
            Catalogue::from_toml(&command_step_with_timeout(&ok.to_string())).expect("loads");
        let step = &catalogue.get("t").expect("tweak").steps[0];
        let TweakStep::Command { detect, apply, .. } = step else {
            panic!("a command step");
        };
        assert_eq!(apply.timeout_secs, Some(ok));
        assert_eq!(detect.timeout_secs, None);
    }
    for bad in ["0", "-5", "601", "99999999999", "\"soon\""] {
        let issue = first_issue(&command_step_with_timeout(bad));
        assert_eq!(issue.field, "steps[0].apply.timeout_secs", "{bad}");
        assert!(matches!(issue.kind, IssueKind::Invalid(_)), "{bad}");
    }
}

#[test]
fn a_command_step_without_revert_is_rejected_naming_tweak_and_field() {
    let toml = tweak_with(
        &[],
        "[[tweak.steps]]\nkind = \"command\"\n\
         detect = { program = \"p\", args = [] }\n\
         apply = { program = \"p\", args = [\"on\"] }\n",
    );
    let issue = first_issue(&toml);
    assert_eq!(issue.tweak.as_deref(), Some("t"));
    assert_eq!(issue.position, Some(1));
    assert_eq!(issue.field, "steps[0].revert");
    assert_eq!(issue.kind, IssueKind::MissingRevert);
    let text = issue.to_string();
    assert!(
        text.contains("\"t\"") && text.contains("steps[0].revert") && text.contains("undoable")
    );
}

#[test]
fn an_empty_step_list_is_rejected() {
    let issue = first_issue(&tweak_with(&[], "steps = []"));
    assert_eq!(
        (issue.field.as_str(), &issue.kind),
        ("steps", &IssueKind::Empty)
    );
    assert_eq!(issue.tweak.as_deref(), Some("t"));
    // A tweak with no steps key at all is just as invalid.
    let issue = first_issue(&tweak_with(&[], ""));
    assert_eq!(issue.field, "steps");
    assert_eq!(issue.kind, IssueKind::MissingField);
}

#[test]
fn empty_and_malformed_registry_subkeys_are_rejected() {
    for (subkey, expect_empty) in [
        ("''", true),
        ("'   '", true),
        (r"'\Software\X'", false),
        (r"'Software\X\'", false),
        (r"'Software\\X'", false),
        (r"'Software\ \X'", false),
    ] {
        let step = format!(
            "[[tweak.steps]]\nkind = \"registry_delete\"\nhive = \"HKCU\"\nsubkey = {subkey}\nname = \"v\""
        );
        let issue = first_issue(&tweak_with(&[], &step));
        assert_eq!(issue.field, "steps[0].subkey", "subkey {subkey}");
        assert_eq!(issue.tweak.as_deref(), Some("t"));
        if expect_empty {
            assert_eq!(issue.kind, IssueKind::Empty, "subkey {subkey}");
        } else {
            assert!(
                matches!(issue.kind, IssueKind::Malformed(_)),
                "subkey {subkey}: {:?}",
                issue.kind
            );
        }
    }
}

#[test]
fn unknown_enum_values_are_rejected_with_the_field_that_holds_them() {
    for (key, value, field) in [
        ("risk", "\"extreme\"", "risk"),
        ("evidence", "\"rumour\"", "evidence"),
        ("category", "\"misc\"", "category"),
        ("restart", "\"later\"", "restart"),
    ] {
        let issue = first_issue(&tweak_with(&[(key, value)], GOOD_STEP));
        assert_eq!(issue.field, field);
        assert!(
            matches!(issue.kind, IssueKind::UnknownValue(_)),
            "{key}: {:?}",
            issue.kind
        );
    }
    // Inside a step too: hive, start type, step kind, registry value type.
    for (step, field) in [
        (
            "[[tweak.steps]]\nkind = \"registry_delete\"\nhive = \"HKXX\"\nsubkey = 'A'\nname = \"v\"",
            "steps[0].hive",
        ),
        (
            "[[tweak.steps]]\nkind = \"service\"\nservice = \"S\"\nstart_type = \"sometimes\"",
            "steps[0].start_type",
        ),
        ("[[tweak.steps]]\nkind = \"reboot\"", "steps[0].kind"),
        (
            "[[tweak.steps]]\nkind = \"registry_set\"\nhive = \"HKCU\"\nsubkey = 'A'\nname = \"v\"\nvalue = { float = 1.5 }",
            "steps[0].value",
        ),
    ] {
        let issue = first_issue(&tweak_with(&[], step));
        assert_eq!(issue.field, field);
        assert!(
            matches!(issue.kind, IssueKind::UnknownValue(_)),
            "{field}: {:?}",
            issue.kind
        );
    }
}

#[test]
fn wrong_types_missing_and_unknown_fields_are_reported_by_name() {
    let issue = first_issue(&tweak_with(&[("title", "5")], GOOD_STEP));
    assert_eq!(issue.field, "title");
    assert!(matches!(issue.kind, IssueKind::Invalid(_)));

    let issue = first_issue(&tweak_with(&[("title", "\"  \"")], GOOD_STEP));
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("title", IssueKind::Empty)
    );

    let issue = first_issue(&tweak_with(&[("evidence_note", "")], GOOD_STEP));
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("evidence_note", IssueKind::MissingField)
    );

    let issue = first_issue(&tweak_with(&[("colour", "\"red\"")], GOOD_STEP));
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("colour", IssueKind::UnknownField)
    );

    let step = "[[tweak.steps]]\nkind = \"registry_delete\"\nhive = \"HKCU\"\nsubkey = 'A'\nname = \"v\"\nextra = 1";
    let issue = first_issue(&tweak_with(&[], step));
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("steps[0].extra", IssueKind::UnknownField)
    );

    // A dword outside u32 is a value error, not a panic or a silent wrap.
    let step = "[[tweak.steps]]\nkind = \"registry_set\"\nhive = \"HKCU\"\nsubkey = 'A'\nname = \"v\"\nvalue = { dword = -1 }";
    let issue = first_issue(&tweak_with(&[], step));
    assert_eq!(issue.field, "steps[0].value");
    assert!(matches!(issue.kind, IssueKind::Invalid(_)));
}

#[test]
fn malformed_ids_commands_and_services_are_rejected() {
    for id in [
        "\"\"",
        "\"Has-Caps\"",
        "\"-lead\"",
        "\"trail-\"",
        "\"two--hyphens\"",
        "\"has space\"",
    ] {
        let issue = first_issue(&tweak_with(&[("id", id)], GOOD_STEP));
        assert_eq!(issue.field, "id", "id {id}");
    }

    // A command is a table, never a shell string.
    let step = "[[tweak.steps]]\nkind = \"command\"\ndetect = \"p --check\"\n\
                apply = { program = \"p\" }\nrevert = { program = \"p\" }";
    let issue = first_issue(&tweak_with(&[], step));
    assert_eq!(issue.field, "steps[0].detect");
    assert!(matches!(issue.kind, IssueKind::Invalid(_)));

    let step = "[[tweak.steps]]\nkind = \"command\"\ndetect = { program = \"p\" }\n\
                apply = { program = \"\" }\nrevert = { program = \"p\" }";
    let issue = first_issue(&tweak_with(&[], step));
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("steps[0].apply.program", IssueKind::Empty)
    );

    let step = "[[tweak.steps]]\nkind = \"service\"\nservice = \"a\\\\b\"\nstart_type = \"manual\"";
    let issue = first_issue(&tweak_with(&[], step));
    assert_eq!(issue.field, "steps[0].service");
    assert!(matches!(issue.kind, IssueKind::Malformed(_)));
}

#[test]
fn two_steps_changing_the_same_thing_are_rejected() {
    let steps = format!(
        "{GOOD_STEP}\n[[tweak.steps]]\nkind = \"registry_set\"\nhive = \"HKCU\"\n\
         subkey = 'SOFTWARE\\x'\nname = \"V\"\nvalue = {{ dword = 1 }}"
    );
    let issue = first_issue(&tweak_with(&[], &steps));
    assert_eq!(issue.field, "steps[1]");
    assert_eq!(issue.kind, IssueKind::DuplicateTarget { first_step: 0 });
}

#[test]
fn duplicate_ids_are_rejected_and_point_at_the_first_use() {
    let toml = format!(
        "{}\n{}",
        tweak_with(&[], GOOD_STEP),
        tweak_with(&[], GOOD_STEP)
    );
    let issue = first_issue(&toml);
    assert_eq!(issue.position, Some(2));
    assert_eq!(issue.tweak.as_deref(), Some("t"));
    assert_eq!(issue.field, "id");
    assert_eq!(issue.kind, IssueKind::DuplicateId { first_position: 1 });
}

#[test]
fn one_bad_tweak_fails_the_whole_catalogue_and_every_problem_is_reported() {
    let good = tweak_with(&[("id", "\"good\"")], GOOD_STEP);
    let bad_risk = tweak_with(
        &[("id", "\"bad-risk\""), ("risk", "\"extreme\"")],
        GOOD_STEP,
    );
    let many = tweak_with(
        &[
            ("id", "\"many\""),
            ("title", "\"\""),
            ("category", "\"nope\""),
        ],
        "steps = []",
    );
    let duplicate = tweak_with(&[("id", "\"good\"")], GOOD_STEP);
    let error = Catalogue::from_toml(&format!("{good}\n{bad_risk}\n{many}\n{duplicate}"))
        .expect_err("rejected");

    let found: Vec<(Option<&str>, &str)> = error
        .issues
        .iter()
        .map(|i| (i.tweak.as_deref(), i.field.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (Some("bad-risk"), "risk"),
            (Some("many"), "title"),
            (Some("many"), "category"),
            (Some("many"), "steps"),
            (Some("good"), "id"),
        ]
    );
    // The message names every tweak and field too.
    let text = error.to_string();
    for needle in [
        "5 problem(s)",
        "\"bad-risk\"",
        "`category`",
        "\"many\"",
        "duplicate id",
    ] {
        assert!(text.contains(needle), "{needle} missing from:\n{text}");
    }
}

#[test]
fn syntax_errors_and_a_misspelt_table_name_are_reported() {
    let issue = first_issue("[[tweak]\nid = 1");
    assert!(matches!(issue.kind, IssueKind::Syntax { .. }));
    assert!(issue.tweak.is_none() && issue.position.is_none());

    // `[[tweaks]]` instead of `[[tweak]]` must not load as an empty catalogue.
    let issue = first_issue("[[tweaks]]\nid = \"x\"");
    assert_eq!(
        (issue.field.as_str(), issue.kind),
        ("tweaks", IssueKind::UnknownField)
    );

    let issue = first_issue("tweak = 3");
    assert_eq!(issue.field, "tweak");
    assert!(matches!(issue.kind, IssueKind::Invalid(_)));
}

// ---- state detection ----------------------------------------------------------------------

#[test]
fn state_is_read_from_the_system_so_changes_made_outside_volt_show_up() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));

    // Someone makes the tweak's changes by hand, one at a time.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(0));
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Partial));
    system
        .registry
        .delete_value(&registry_ref("Legacy"))
        .expect("delete");
    system.services.add("SampleSvc", ServiceStartType::Disabled);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Partial));
    system.commands.set_on(&detect(), true);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Applied));

    // And back again.
    system.commands.set_on(&detect(), false);
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(1));
    system
        .registry
        .set(&registry_ref("Legacy"), RegistryValue::Dword(7));
    system
        .services
        .add("SampleSvc", ServiceStartType::Automatic);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));

    // Looking never writes the journal or changes the system: the only side
    // effect on record is the manual delete above.
    assert!(system.journal.entries().is_empty());
    assert_eq!(
        system.log.events(),
        [Event::RegistryDelete(registry_ref("Legacy"))]
    );
}

#[test]
fn a_state_that_cannot_be_read_is_a_typed_error_not_a_guess() {
    let catalogue = catalogue(SAMPLE);
    let system = machine();
    let engine = engine(&system, &catalogue);
    assert_eq!(
        engine.state("nope"),
        Err(TweakError::UnknownTweak("nope".to_owned()))
    );

    // The service does not exist: step 2 cannot be read.
    let no_service = FakeSystem::new();
    no_service
        .commands
        .add_toggle(detect(), apply_cmd(), revert_cmd(), false);
    assert_eq!(
        self::engine(&no_service, &catalogue).state("sample-tweak"),
        Err(TweakError::Detect {
            tweak: "sample-tweak".to_owned(),
            step: 2,
            error: PortError::NotFound("service SampleSvc".to_owned()),
        })
    );

    // A detect command that exits with anything but 0 or 1 failed to check.
    system.commands.set_exit_code(&detect(), 2);
    assert!(matches!(
        engine.state("sample-tweak"),
        Err(TweakError::Detect {
            step: 3,
            error: PortError::Other(_),
            ..
        })
    ));

    // A detect command that cannot be started is an error as well.
    system.commands.fail_on(
        &detect(),
        PortError::NotFound("program sample-tool".to_owned()),
    );
    assert!(matches!(
        engine.state("sample-tweak"),
        Err(TweakError::Detect {
            step: 3,
            error: PortError::NotFound(_),
            ..
        })
    ));
    // The same errors stop a dry run and an apply before anything changes.
    assert!(matches!(
        engine.plan("sample-tweak"),
        Err(TweakError::Detect { .. })
    ));
    assert!(matches!(
        engine.apply("sample-tweak", Source::Manual),
        Err(TweakError::Detect { .. })
    ));
    assert!(system.journal.entries().is_empty());
}

// ---- dry run and apply --------------------------------------------------------------------

#[test]
fn the_plan_is_a_dry_run_that_shows_current_and_target_and_changes_nothing() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    let plan = engine.plan("sample-tweak").expect("plan");
    assert_eq!(plan.items.len(), 4);
    assert_eq!(plan.label.as_deref(), Some("sample-tweak"));
    assert_eq!(plan.risk(), Risk::Caution);
    assert!(plan.items.iter().all(|i| i.evidence == Evidence::Measured));
    assert_eq!(
        plan.items[0].preview.to_string(),
        r"HKCU\Software\VoltTest\Tweaks\Flag: REG_DWORD 1 -> REG_DWORD 0"
    );
    assert_eq!(
        plan.items[1].preview.to_string(),
        r"HKCU\Software\VoltTest\Tweaks\Legacy: REG_DWORD 7 -> (not set)"
    );
    assert_eq!(
        plan.items[2].preview.to_string(),
        "service SampleSvc start type: Automatic -> Disabled"
    );
    assert_eq!(
        plan.items[3].preview.to_string(),
        "command sample-tool --enable: not applied -> applied"
    );
    // Every part of the preview also comes as a phrase that reads the same in English.
    for item in &plan.items {
        let preview = &item.preview;
        assert_eq!(preview.subject_phrase.english(), preview.subject);
        assert_eq!(preview.current_phrase.english(), preview.current);
        assert_eq!(preview.target_phrase.english(), preview.target);
    }
    assert_before(&system);
    assert!(
        system.log.events().is_empty(),
        "a dry run has no side effect"
    );
    assert!(system.journal.entries().is_empty());
}

#[test]
fn apply_journals_each_step_before_changing_it_and_labels_the_batch() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    let result = engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(result.outcome, ApplyOutcome::Applied);
    let report = result.report.expect("report");
    assert!(report.is_success());
    assert_after(&system);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Applied));

    // One labelled batch with four applied entries holding the prior states.
    assert_eq!(
        system.journal.batches_with_label("sample-tweak"),
        Ok(vec![report.batch])
    );
    let entries = system.journal.batch_entries(report.batch).expect("entries");
    assert_eq!(entries.len(), 4);
    assert!(entries.iter().all(|e| e.state == EntryState::Applied));
    assert_eq!(
        entries[1].prior,
        crate::PriorState::RegistryValue(Some(RegistryValue::Dword(7)))
    );
    assert_eq!(entries[3].prior, crate::PriorState::CommandState(false));

    // Write-ahead: each mutation is preceded by its pending journal entry.
    let events = system.log.events();
    let position =
        |wanted: &dyn Fn(&Event) -> bool| events.iter().position(wanted).expect("event happened");
    for (entry, mutation) in [
        (0, position(&|e| matches!(e, Event::RegistryWrite(_)))),
        (1, position(&|e| matches!(e, Event::RegistryDelete(_)))),
        (
            2,
            position(&|e| matches!(e, Event::ServiceSetStartType(..))),
        ),
        (3, position(&|e| matches!(e, Event::CommandRun(_)))),
    ] {
        let pending = position(&|e| *e == Event::JournalPending(entries[entry].id));
        assert!(
            pending < mutation,
            "step {entry} was changed before it was journaled"
        );
    }
}

#[test]
fn applying_an_applied_tweak_does_nothing_and_a_partial_one_changes_only_what_is_missing() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);

    // Partial: the flag was already set outside Volt.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(0));
    let plan = engine.plan("sample-tweak").expect("plan");
    assert_eq!(plan.items.len(), 3, "the flag is left out");
    let result = engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(result.outcome, ApplyOutcome::Applied);
    assert_eq!(result.report.expect("report").outcomes.len(), 3);
    assert_after(&system);

    // Applied: nothing to do, no batch, no entry.
    let batches = system
        .journal
        .batches_with_label("sample-tweak")
        .expect("batches");
    let again = engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(again.outcome, ApplyOutcome::AlreadyApplied);
    assert!(again.report.is_none());
    assert_eq!(
        system
            .journal
            .batches_with_label("sample-tweak")
            .expect("batches"),
        batches
    );
    assert!(engine.plan("sample-tweak").expect("plan").is_empty());
}

#[test]
fn a_journal_that_refuses_an_entry_blocks_the_change() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    system
        .journal
        .fail_next_record(PortError::Other("disk full".to_owned()));
    let result = engine
        .apply("sample-tweak", Source::Manual)
        .expect("apply returns a report");
    // The first step could not be journaled, so nothing was changed and the
    // report says exactly that.
    assert_eq!(result.outcome, ApplyOutcome::RolledBack);
    let report = result.report.expect("report");
    assert!(matches!(
        report.outcomes[0].status,
        ItemStatus::Failed {
            entry: None,
            error: ExecError::Journal(_)
        }
    ));
    assert_eq!(report.not_attempted().count(), 3);
    assert_before(&system);
}

// ---- failure part-way ---------------------------------------------------------------------

#[test]
fn a_failure_part_way_is_rolled_back_and_the_report_says_so() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    // Steps 0 and 1 apply; the service step (2) fails.
    system.services.fail_on(
        "SampleSvc",
        PortError::AccessDenied("not elevated".to_owned()),
    );

    let result = engine
        .apply("sample-tweak", Source::Manual)
        .expect("apply returns a report");
    assert_eq!(result.outcome, ApplyOutcome::RolledBack);
    let report = result.report.expect("report");
    assert!(!report.is_success());
    assert_eq!(report.applied().count(), 2);
    assert_eq!(report.failed().count(), 1);
    assert_eq!(report.not_attempted().count(), 1);
    assert!(matches!(
        &report.outcomes[2].status,
        ItemStatus::Failed {
            error: ExecError::Apply { .. },
            ..
        }
    ));
    let rollback = report.rollback.expect("rollback attached");
    assert!(rollback.is_complete());
    assert_eq!(rollback.outcomes.len(), 2);

    // Back where it started, verified by reading the system, and journaled so.
    assert_before(&system);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));
    let entries = system.journal.batch_entries(report.batch).expect("entries");
    assert_eq!(
        entries.iter().map(|e| e.state).collect::<Vec<_>>(),
        [
            EntryState::Reverted,
            EntryState::Reverted,
            EntryState::Failed
        ]
    );
    // Nothing is left to undo afterwards.
    assert_eq!(
        engine.revert("sample-tweak"),
        Err(TweakError::NothingToRevert("sample-tweak".to_owned()))
    );
}

#[test]
fn a_command_that_exits_non_zero_fails_the_step_and_is_rolled_back() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    system.commands.set_exit_code(&apply_cmd(), 5);

    let result = engine
        .apply("sample-tweak", Source::Manual)
        .expect("apply returns a report");
    assert_eq!(result.outcome, ApplyOutcome::RolledBack);
    let report = result.report.expect("report");
    let ItemStatus::Failed {
        error: ExecError::Apply { error, .. },
        ..
    } = &report.outcomes[3].status
    else {
        panic!(
            "the command step should have failed: {:?}",
            report.outcomes[3]
        );
    };
    assert!(error.to_string().contains("sample-tool --enable"));
    assert!(error.to_string().contains("code 5"));
    assert_before(&system);
}

#[test]
fn a_rollback_that_cannot_be_completed_is_reported_as_needing_attention() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    system.services.fail_on(
        "SampleSvc",
        PortError::AccessDenied("not elevated".to_owned()),
    );
    // The journal cannot record the first revert, so the rollback is incomplete
    // even though the system itself gets restored.
    system.journal.fail_next_set_state(
        EntryState::Reverted,
        PortError::Other("journal write failed".to_owned()),
    );

    let result = engine
        .apply("sample-tweak", Source::Manual)
        .expect("apply returns a report");
    assert_eq!(result.outcome, ApplyOutcome::NeedsAttention);
    let rollback = result.report.expect("report").rollback.expect("rollback");
    assert!(!rollback.is_complete());
    assert_eq!(
        rollback
            .outcomes
            .iter()
            .filter(|o| o.result.is_err())
            .count(),
        1
    );
}

#[test]
fn a_failed_step_that_left_its_effect_behind_is_not_called_rolled_back() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    // The command switches its effect on and then exits non-zero: commands are
    // not atomic. The rollback undoes the other steps, but reading the system
    // again shows it is not back where it started.
    system.commands.leak(&apply_cmd(), 9);

    let result = engine
        .apply("sample-tweak", Source::Manual)
        .expect("apply returns a report");
    assert_eq!(result.outcome, ApplyOutcome::NeedsAttention);
    let report = result.report.expect("report");
    assert!(report.rollback.expect("rollback").is_complete());
    assert_eq!(system.commands.is_on(&detect()), Some(true));
    assert_eq!(
        engine.state("sample-tweak"),
        Ok(TweakState::Partial),
        "the leftover effect is visible to the next state check"
    );
}

// ---- revert ---------------------------------------------------------------------------------

#[test]
fn revert_works_in_a_new_engine_after_a_restart() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    {
        let first = engine(&system, &catalogue);
        assert_eq!(
            first
                .apply("sample-tweak", Source::Manual)
                .expect("apply")
                .outcome,
            ApplyOutcome::Applied
        );
    } // the first engine is gone; only the journal and the machine remain
    assert_after(&system);

    let second = engine(&system, &catalogue);
    assert_eq!(second.state("sample-tweak"), Ok(TweakState::Applied));
    let report = second.revert("sample-tweak").expect("revert");
    assert!(report.is_complete());
    assert_eq!(report.batches.len(), 1);
    assert_eq!(report.batches[0].report.outcomes.len(), 4);

    // Everything is as before, including the value that did not exist.
    assert_before(&system);
    assert_eq!(second.state("sample-tweak"), Ok(TweakState::NotApplied));
    let entries = system
        .journal
        .batch_entries(report.batches[0].batch)
        .expect("entries");
    assert!(entries.iter().all(|e| e.state == EntryState::Reverted));
    // Newest first: the command was undone before the registry value.
    let reverted_order: Vec<_> = report.batches[0]
        .report
        .outcomes
        .iter()
        .map(|o| o.entry)
        .collect();
    let mut expected: Vec<_> = entries.iter().map(|e| e.id).collect();
    expected.reverse();
    assert_eq!(reverted_order, expected);

    // A second revert has nothing left to do.
    assert_eq!(
        second.revert("sample-tweak"),
        Err(TweakError::NothingToRevert("sample-tweak".to_owned()))
    );
}

#[test]
fn a_tweak_volt_never_applied_cannot_be_reverted() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    // Changed outside Volt: no journal, so no recorded prior state to restore.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(0));
    assert_eq!(
        engine.revert("sample-tweak"),
        Err(TweakError::NothingToRevert("sample-tweak".to_owned()))
    );
    assert_eq!(
        engine.revert("nope"),
        Err(TweakError::UnknownTweak("nope".to_owned()))
    );
}

#[test]
fn a_tweak_applied_twice_over_time_is_reverted_batch_by_batch_newest_first() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    engine
        .apply("sample-tweak", Source::Manual)
        .expect("first apply");

    // Windows Update (or the user) undoes the tweak; Volt applies it again.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(1));
    system
        .registry
        .set(&registry_ref("Legacy"), RegistryValue::Dword(8));
    system.services.add("SampleSvc", ServiceStartType::Manual);
    system.commands.set_on(&detect(), false);
    engine
        .apply("sample-tweak", Source::Manual)
        .expect("second apply");
    assert_after(&system);

    let report = engine.revert("sample-tweak").expect("revert");
    assert!(report.is_complete());
    assert_eq!(report.batches.len(), 2);
    assert!(
        report.batches[0].batch > report.batches[1].batch,
        "newest first"
    );
    // The oldest batch restores the original machine last.
    assert_before(&system);
}

#[test]
fn a_revert_that_fails_is_reported_precisely_and_can_be_retried() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    engine.apply("sample-tweak", Source::Manual).expect("apply");
    system.services.fail_on(
        "SampleSvc",
        PortError::AccessDenied("not elevated".to_owned()),
    );

    let report = engine
        .revert("sample-tweak")
        .expect("revert returns a report");
    assert!(!report.is_complete());
    let outcomes = &report.batches[0].report.outcomes;
    assert_eq!(outcomes.len(), 4, "the other steps are still undone");
    assert_eq!(outcomes.iter().filter(|o| o.result.is_err()).count(), 1);
    // The failed entry stays applied, so a later revert can finish the job.
    let still_applied = system
        .journal
        .batch_entries(report.batches[0].batch)
        .expect("entries")
        .iter()
        .filter(|e| e.state == EntryState::Applied)
        .count();
    assert_eq!(still_applied, 1);
}

#[test]
fn reverting_leaves_alone_a_command_effect_that_was_there_before_the_apply() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    // The command's effect was already on, so Volt does not run it and must
    // not run its revert command (which would remove something not its own).
    system.commands.set_on(&detect(), true);
    let applied = engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(applied.report.expect("report").outcomes.len(), 3);
    assert!(!system.commands.ran().contains(&apply_cmd()));

    assert!(engine.revert("sample-tweak").expect("revert").is_complete());
    assert!(!system.commands.ran().contains(&revert_cmd()));
    assert_eq!(system.commands.is_on(&detect()), Some(true));
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(1))
    );
}

#[test]
fn commands_are_shown_with_quoted_arguments_but_kept_as_a_list() {
    let command = Command::new("tool", ["--flag", "two words", ""]);
    assert_eq!(command.to_string(), "tool --flag \"two words\" \"\"");
    assert_eq!(command.args.len(), 3);
}

// ---- end to end ---------------------------------------------------------------------------

#[test]
fn sample_tweak_end_to_end_against_the_fakes() {
    // Load.
    let catalogue = catalogue(SAMPLE);
    let system = machine();
    let engine = engine(&system, &catalogue);

    // Look: not applied, and looking changes nothing.
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));
    assert!(system.log.events().is_empty());

    // Preview, apply.
    assert_eq!(engine.plan("sample-tweak").expect("plan").items.len(), 4);
    let applied = engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(applied.outcome, ApplyOutcome::Applied);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Applied));
    assert_after(&system);

    // Quit and come back: a new engine still sees it applied and can undo it.
    drop(engine);
    let engine = self::engine(&system, &catalogue);
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::Applied));
    assert!(engine.revert("sample-tweak").expect("revert").is_complete());
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));
    assert_before(&system);

    // The whole story is in the journal: four entries, all reverted.
    let entries = system.journal.entries();
    assert_eq!(entries.len(), 4);
    assert!(entries.iter().all(|e| e.state == EntryState::Reverted));
}

#[test]
fn the_engine_writes_activity_labelled_with_the_tweak_id_for_apply_and_revert() {
    let catalogue = catalogue(SAMPLE);
    let system = machine();
    let engine = engine(&system, &catalogue).with_activity_log(system.activity.clone());

    engine.apply("sample-tweak", Source::Manual).expect("apply");
    // A new engine over the same journal, as after a restart.
    let engine = self::engine(&system, &catalogue).with_activity_log(system.activity.clone());
    engine.revert("sample-tweak").expect("revert");

    let records = system.activity.records();
    assert_eq!(records.len(), 8, "four steps applied, four reverted");
    assert!(
        records
            .iter()
            .all(|r| r.label.as_deref() == Some("sample-tweak"))
    );
    let count = |outcome| records.iter().filter(|r| r.outcome == outcome).count();
    assert_eq!(count(crate::ActivityOutcome::Applied), 4);
    assert_eq!(count(crate::ActivityOutcome::Reverted), 4);
    assert_eq!(engine.activity_write_failures(), 0);
}

// ---- administrator rights -----------------------------------------------------------------

fn admin_sample() -> String {
    SAMPLE.replace(
        "restart = \"reboot\"",
        "restart = \"reboot\"\nrequires_admin = true",
    )
}

#[test]
fn requires_admin_is_optional_and_defaults_to_false() {
    assert!(
        !catalogue(SAMPLE)
            .get("sample-tweak")
            .expect("tweak")
            .requires_admin
    );
    assert!(
        catalogue(&admin_sample())
            .get("sample-tweak")
            .expect("tweak")
            .requires_admin
    );
}

#[test]
fn requires_admin_must_be_a_boolean() {
    let text = SAMPLE.replace(
        "restart = \"reboot\"",
        "restart = \"reboot\"\nrequires_admin = \"yes\"",
    );
    let error = Catalogue::from_toml(&text).expect_err("rejected");
    assert!(
        error
            .issues
            .iter()
            .any(|issue| issue.field == "requires_admin"),
        "{error}"
    );
}

#[test]
fn a_tweak_that_needs_administrator_rights_is_refused_before_anything_is_touched() {
    let catalogue = catalogue(&admin_sample());
    let system = machine();
    let engine = engine(&system, &catalogue);

    assert_eq!(
        engine.apply("sample-tweak", Source::Manual).unwrap_err(),
        TweakError::NeedsAdministrator("sample-tweak".to_owned())
    );
    assert_eq!(
        engine.revert("sample-tweak").unwrap_err(),
        TweakError::NeedsAdministrator("sample-tweak".to_owned())
    );
    assert_before(&system);
    assert!(system.journal.entries().is_empty(), "nothing was journaled");
    assert!(system.log.events().is_empty(), "no side effect of any kind");
    // Reading is still allowed, so the screen can show the state.
    assert_eq!(engine.state("sample-tweak"), Ok(TweakState::NotApplied));
    assert!(engine.plan("sample-tweak").is_ok());
}

#[test]
fn an_engine_told_it_has_administrator_rights_applies_and_reverts_such_a_tweak() {
    let catalogue = catalogue(&admin_sample());
    let system = machine();
    let engine = engine(&system, &catalogue).with_administrator(true);

    engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_after(&system);
    assert!(engine.revert("sample-tweak").expect("revert").is_complete());
    assert_before(&system);
}

// ---- describe, provenance and reset -------------------------------------------------------

const REGISTRY_ONLY: &str = r#"
[[tweak]]
id = "reg-tweak"
title = "Registry tweak"
description = "Sets two values."
category = "appearance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "A preference."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "Flag"
value = { dword = 0 }

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "Other"
value = { dword = 5 }
"#;

#[test]
fn describe_lists_every_step_with_its_state_even_when_the_tweak_is_on() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    let before = engine.describe("sample-tweak").expect("describe");
    assert_eq!(before.len(), 4);
    assert!(before.iter().all(|step| !step.at_target));
    engine.apply("sample-tweak", Source::Manual).expect("apply");
    let after = engine.describe("sample-tweak").expect("describe");
    assert_eq!(after.len(), 4);
    assert!(after.iter().all(|step| step.at_target));
    assert!(engine.plan("sample-tweak").expect("plan").is_empty());
}

#[test]
fn a_tweak_is_applied_by_volt_only_when_the_journal_says_so() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.applied_by_volt("sample-tweak"), Ok(false));
    engine.apply("sample-tweak", Source::Manual).expect("apply");
    assert_eq!(engine.applied_by_volt("sample-tweak"), Ok(true));
    engine.revert("sample-tweak").expect("revert");
    assert_eq!(engine.applied_by_volt("sample-tweak"), Ok(false));
}

#[test]
fn reset_removes_the_values_of_a_tweak_set_outside_volt_and_can_be_undone() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(0));
    system
        .registry
        .set(&registry_ref("Other"), RegistryValue::Dword(5));

    let plan = engine.reset_plan("reg-tweak").expect("plan").expect("some");
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].preview.target, "(not set)");

    let report = engine.reset("reg-tweak", Source::Manual).expect("reset");
    assert_eq!(report.outcome, ApplyOutcome::Applied);
    assert_eq!(system.registry.get(&registry_ref("Flag")), None);
    assert_eq!(system.registry.get(&registry_ref("Other")), None);
    assert_eq!(engine.state("reg-tweak"), Ok(TweakState::NotApplied));

    // Filed under its own label, so reverting the tweak never switches it back on.
    assert_eq!(
        engine.revert("reg-tweak"),
        Err(TweakError::NothingToRevert("reg-tweak".to_owned()))
    );
    assert_eq!(
        engine
            .reset("reg-tweak", Source::Manual)
            .expect("again")
            .outcome,
        ApplyOutcome::AlreadyApplied
    );
    // The journal can take it back.
    let batches = system
        .journal
        .batches_with_label("reg-tweak:reset")
        .expect("batches");
    assert_eq!(batches.len(), 1);
    engine
        .executor()
        .revert_batch(batches[0], Source::Manual, Some("reg-tweak:reset"))
        .expect("undo");
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(0))
    );
}

#[test]
fn a_tweak_with_a_service_or_command_cannot_be_reset() {
    let system = machine();
    let catalogue = catalogue(SAMPLE);
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.reset_plan("sample-tweak"), Ok(None));
    assert_eq!(
        engine.reset("sample-tweak", Source::Manual),
        Err(TweakError::NothingToReset("sample-tweak".to_owned()))
    );
    assert_before(&system);
}

// ---- registry key steps -------------------------------------------------------------------

/// A key that has to exist, and a value in it.
const KEY_TWEAK: &str = r#"
[[tweak]]
id = "key-tweak"
title = "Key tweak"
description = "A key and a value in it."
category = "appearance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "A preference."

[[tweak.steps]]
kind = "registry_key"
hive = "HKCU"
subkey = 'Software\VoltTest\Keys\Made'
exists = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Keys\Made'
name = "Flag"
value = { dword = 1 }
"#;

/// A key that has to be absent.
const GONE_TWEAK: &str = r#"
[[tweak]]
id = "gone-tweak"
title = "Gone tweak"
description = "A key that is deleted."
category = "appearance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "A preference."

[[tweak.steps]]
kind = "registry_key"
hive = "HKCU"
subkey = 'Software\VoltTest\Keys\Gone'
exists = false
"#;

/// A key below `HKCU\Software\VoltTest\Keys`; an empty `path` is that key itself.
fn key_at(path: &str) -> RegistryKeyRef {
    let subkey = if path.is_empty() {
        r"Software\VoltTest\Keys".to_owned()
    } else {
        format!(r"Software\VoltTest\Keys\{path}")
    };
    RegistryKeyRef::new(RegistryHive::CurrentUser, subkey, RegistryView::Native)
}

fn exists_at(system: &FakeSystem, subkey: &str) -> bool {
    system
        .registry
        .key_exists(RegistryHive::CurrentUser, subkey, RegistryView::Native)
        .expect("the fake answers")
}

fn key_exists(system: &FakeSystem, path: &str) -> bool {
    exists_at(system, &key_at(path).subkey)
}

#[test]
fn a_key_step_creates_the_key_and_a_revert_removes_what_the_apply_created() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::NotApplied));
    let plan = engine.plan("key-tweak").expect("plan");
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].preview.current, "key missing");
    assert_eq!(plan.items[0].preview.target, "key exists");

    let report = engine.apply("key-tweak", Source::Manual).expect("apply");
    assert_eq!(report.outcome, ApplyOutcome::Applied);
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::Applied));
    assert!(key_exists(&system, "Made"));
    assert!(
        system
            .log
            .events()
            .contains(&Event::RegistryKeyCreate(key_at("Made")))
    );

    assert!(engine.revert("key-tweak").expect("revert").is_complete());
    // The key, and the parents the apply had to create, are gone: nothing is left behind.
    assert!(!key_exists(&system, "Made"));
    assert!(!key_exists(&system, ""));
    assert!(!exists_at(&system, r"Software\VoltTest"));
    assert!(!exists_at(&system, "Software"));
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::NotApplied));
}

#[test]
fn a_revert_keeps_a_key_that_existed_before_with_its_other_values() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    let other = key_at("Made").value("Other");
    system.registry.set(&other, RegistryValue::Dword(5));

    // The key is there already, so only the value is applied.
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::Partial));
    engine.apply("key-tweak", Source::Manual).expect("apply");
    assert!(
        !system
            .log
            .events()
            .iter()
            .any(|event| matches!(event, Event::RegistryKeyCreate(_)))
    );
    assert!(engine.revert("key-tweak").expect("revert").is_complete());

    assert!(key_exists(&system, "Made"));
    assert_eq!(system.registry.get(&other), Some(RegistryValue::Dword(5)));
    assert_eq!(system.registry.get(&key_at("Made").value("Flag")), None);
}

#[test]
fn a_revert_does_not_remove_a_key_that_holds_something_it_did_not_write() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    engine.apply("key-tweak", Source::Manual).expect("apply");
    // Another program puts a subkey into the key, and a value next to it in the parent.
    system
        .registry
        .set(&key_at(r"Made\Sub").value("x"), RegistryValue::Dword(1));
    system
        .registry
        .set(&key_at("").value("Parent"), RegistryValue::Dword(2));

    assert!(engine.revert("key-tweak").expect("revert").is_complete());
    assert_eq!(system.registry.get(&key_at("Made").value("Flag")), None);
    assert!(key_exists(&system, "Made"), "a key with a subkey stays");
    assert!(key_exists(&system, r"Made\Sub"));
    assert_eq!(
        system.registry.get(&key_at("").value("Parent")),
        Some(RegistryValue::Dword(2))
    );
}

#[test]
fn a_value_step_that_had_to_create_its_key_removes_only_what_it_created() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    // `Software\VoltTest` exists already, `Tweaks` below it does not.
    system.registry.add_key(
        RegistryHive::CurrentUser,
        r"Software\VoltTest",
        RegistryView::Native,
    );

    engine.apply("reg-tweak", Source::Manual).expect("apply");
    assert!(engine.revert("reg-tweak").expect("revert").is_complete());

    assert!(!exists_at(&system, r"Software\VoltTest\Tweaks"));
    assert!(exists_at(&system, r"Software\VoltTest"));
    assert!(exists_at(&system, "Software"));
}

#[test]
fn a_failed_step_after_a_key_step_rolls_the_created_keys_back() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    system.registry.fail_on(
        &key_at("Made").value("Flag"),
        PortError::AccessDenied("test".to_owned()),
    );

    let report = engine.apply("key-tweak", Source::Manual).expect("report");
    assert_eq!(report.outcome, ApplyOutcome::RolledBack);
    assert!(
        !key_exists(&system, "Made"),
        "the key the first step made is gone again"
    );
    assert!(!exists_at(&system, "Software"));
}

#[test]
fn a_key_delete_step_records_the_key_and_a_revert_puts_it_back_exactly() {
    let system = FakeSystem::new();
    let catalogue = catalogue(GONE_TWEAK);
    let engine = engine(&system, &catalogue);
    let gone = |name: &str| key_at("Gone").value(name);
    system.registry.set(&gone("A"), RegistryValue::Dword(1));
    system
        .registry
        .set(&gone("B"), RegistryValue::Sz("two".to_owned()));
    system
        .registry
        .set(&gone(""), RegistryValue::Binary(vec![1, 2, 3]));
    let before = system.registry.list_values(&key_at("Gone")).expect("lists");
    assert_eq!(before.len(), 3);

    assert_eq!(engine.state("gone-tweak"), Ok(TweakState::NotApplied));
    assert_eq!(
        engine.plan("gone-tweak").expect("plan").items[0]
            .preview
            .current,
        "key exists, 3 values"
    );
    engine.apply("gone-tweak", Source::Manual).expect("apply");
    assert_eq!(engine.state("gone-tweak"), Ok(TweakState::Applied));
    assert!(!key_exists(&system, "Gone"));
    assert!(
        system
            .log
            .events()
            .contains(&Event::RegistryKeyDelete(key_at("Gone")))
    );

    assert!(engine.revert("gone-tweak").expect("revert").is_complete());
    assert!(key_exists(&system, "Gone"));
    assert_eq!(
        system.registry.list_values(&key_at("Gone")).expect("lists"),
        before,
        "every value is back with its type"
    );
}

#[test]
fn a_key_with_subkeys_or_more_data_than_can_be_recorded_is_never_deleted() {
    let catalogue = catalogue(GONE_TWEAK);

    // A subkey.
    let system = FakeSystem::new();
    let engine = engine(&system, &catalogue);
    system
        .registry
        .set(&key_at(r"Gone\Child").value("x"), RegistryValue::Dword(1));
    let report = engine.apply("gone-tweak", Source::Manual).expect("report");
    assert_eq!(report.outcome, ApplyOutcome::RolledBack);
    let executor_report = report.report.expect("executor report");
    assert!(matches!(
        executor_report.outcomes[0].status,
        ItemStatus::Failed {
            error: ExecError::Apply {
                error: PortError::InvalidInput(_),
                ..
            },
            ..
        }
    ));
    assert!(key_exists(&system, r"Gone\Child"));

    // Too many values to record.
    let system = FakeSystem::new();
    let engine = self::engine(&system, &catalogue);
    for n in 0..=crate::registry::MAX_RECORDED_VALUES {
        system.registry.set(
            &key_at("Gone").value(format!("v{n}")),
            RegistryValue::Dword(1),
        );
    }
    let report = engine.apply("gone-tweak", Source::Manual).expect("report");
    assert_eq!(report.outcome, ApplyOutcome::RolledBack);
    assert_eq!(
        system
            .registry
            .list_values(&key_at("Gone"))
            .expect("lists")
            .len(),
        crate::registry::MAX_RECORDED_VALUES + 1,
        "nothing was deleted"
    );
}

// ---- off values and reset -----------------------------------------------------------------

/// Every step kind, with a Windows default where one is known.
const OFF_TWEAK: &str = r#"
[[tweak]]
id = "off-tweak"
title = "Off tweak"
description = "Every step says what it goes back to."
category = "appearance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "A preference."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "WithDefault"
value = { dword = 0 }
off = { dword = 3 }

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "WithoutDefault"
value = { dword = 0 }

[[tweak.steps]]
kind = "registry_delete"
hive = "HKCU"
subkey = 'Software\VoltTest\Tweaks'
name = "Legacy"
off = { sz = "legacy" }

[[tweak.steps]]
kind = "service"
service = "SampleSvc"
start_type = "disabled"
off = "automatic_delayed"

[[tweak.steps]]
kind = "command"
detect = { program = "sample-tool", args = ["--is-enabled"] }
apply = { program = "sample-tool", args = ["--enable"] }
revert = { program = "sample-tool", args = ["--disable"] }
off = { program = "sample-tool", args = ["--turn-off"] }
"#;

/// The machine as `OFF_TWEAK` leaves it, set outside Volt.
fn set_outside_volt() -> FakeSystem {
    let system = FakeSystem::new();
    system
        .registry
        .set(&registry_ref("WithDefault"), RegistryValue::Dword(0));
    system
        .registry
        .set(&registry_ref("WithoutDefault"), RegistryValue::Dword(0));
    system.services.add("SampleSvc", ServiceStartType::Disabled);
    system.commands.add_toggle(
        detect(),
        apply_cmd(),
        Command::new("sample-tool", ["--turn-off"]),
        true,
    );
    system
}

#[test]
fn a_reset_writes_the_declared_default_removes_a_value_without_one_and_can_be_undone() {
    let system = set_outside_volt();
    let catalogue = catalogue(OFF_TWEAK);
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.state("off-tweak"), Ok(TweakState::Applied));

    let plan = engine.reset_plan("off-tweak").expect("plan").expect("some");
    assert_eq!(plan.items.len(), 5);
    // The last step goes first.
    assert_eq!(plan.items[0].preview.target, "not applied");

    let report = engine.reset("off-tweak", Source::Manual).expect("reset");
    assert_eq!(report.outcome, ApplyOutcome::Applied);
    assert_eq!(engine.state("off-tweak"), Ok(TweakState::NotApplied));
    assert_eq!(
        system.registry.get(&registry_ref("WithDefault")),
        Some(RegistryValue::Dword(3)),
        "the declared default is written"
    );
    assert_eq!(
        system.registry.get(&registry_ref("WithoutDefault")),
        None,
        "without one the value is removed"
    );
    assert_eq!(
        system.registry.get(&registry_ref("Legacy")),
        Some(RegistryValue::Sz("legacy".to_owned())),
        "a deleted value comes back as its default"
    );
    assert_eq!(
        system.services.start_type("SampleSvc"),
        Ok(ServiceStartType::AutomaticDelayed)
    );
    assert_eq!(system.commands.is_on(&detect()), Some(false));

    // A reset is not something revert switches back on, but the journal can undo it.
    assert_eq!(
        engine.revert("off-tweak"),
        Err(TweakError::NothingToRevert("off-tweak".to_owned()))
    );
    let batches = system
        .journal
        .batches_with_label("off-tweak:reset")
        .expect("batches");
    assert_eq!(batches.len(), 1);
    let undone = engine
        .executor()
        .revert_batch(batches[0], Source::Manual, None)
        .expect("undo");
    assert!(undone.is_complete());
    assert_eq!(engine.state("off-tweak"), Ok(TweakState::Applied));
    assert_eq!(system.registry.get(&registry_ref("Legacy")), None);
    assert_eq!(
        system.registry.get(&registry_ref("WithDefault")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(
        system.services.start_type("SampleSvc"),
        Ok(ServiceStartType::Disabled)
    );
    assert_eq!(system.commands.is_on(&detect()), Some(true));
}

#[test]
fn a_reset_of_a_key_step_deletes_the_key_after_the_value_in_it() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    system
        .registry
        .set(&key_at("Made").value("Flag"), RegistryValue::Dword(1));
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::Applied));

    let report = engine.reset("key-tweak", Source::Manual).expect("reset");
    assert_eq!(report.outcome, ApplyOutcome::Applied);
    assert!(!key_exists(&system, "Made"));
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::NotApplied));

    // The journal brings the key back with its value.
    let batches = system
        .journal
        .batches_with_label("key-tweak:reset")
        .expect("batches");
    engine
        .executor()
        .revert_batch(batches[0], Source::Manual, None)
        .expect("undo");
    assert_eq!(
        system.registry.get(&key_at("Made").value("Flag")),
        Some(RegistryValue::Dword(1))
    );
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::Applied));
}

#[test]
fn a_tweak_that_volt_applied_is_not_reset_because_undo_is_the_way_back() {
    let system = FakeSystem::new();
    let catalogue = catalogue(KEY_TWEAK);
    let engine = engine(&system, &catalogue);
    engine.apply("key-tweak", Source::Manual).expect("apply");
    assert_eq!(engine.applied_by_volt("key-tweak"), Ok(true));
    assert_eq!(
        engine.reset("key-tweak", Source::Manual),
        Err(TweakError::AppliedByVolt("key-tweak".to_owned()))
    );
    assert!(key_exists(&system, "Made"), "nothing was changed");
    // Undo is.
    assert!(engine.revert("key-tweak").expect("revert").is_complete());
    assert_eq!(engine.state("key-tweak"), Ok(TweakState::NotApplied));
}

#[test]
fn a_tweak_with_a_step_that_has_no_declared_default_cannot_be_reset() {
    // A key that has to be absent: Volt does not know what to put there.
    let catalogue = catalogue(GONE_TWEAK);
    let system = FakeSystem::new();
    let engine = engine(&system, &catalogue);
    assert_eq!(engine.reset_plan("gone-tweak"), Ok(None));
    assert_eq!(
        engine.reset("gone-tweak", Source::Manual),
        Err(TweakError::NothingToReset("gone-tweak".to_owned()))
    );
}

#[test]
fn off_values_and_key_steps_are_validated_by_name() {
    let step = |body: &str| tweak_with(&[], &format!("[[tweak.steps]]\n{body}\n"));
    // `off` equal to the value could never be turned off.
    let issue = first_issue(&step(
        "kind = \"registry_set\"\nhive = \"HKCU\"\nsubkey = 'Software\\X'\nname = \"V\"\nvalue = { dword = 0 }\noff = { dword = 0 }",
    ));
    assert_eq!(issue.field, "steps[0].off");
    let issue = first_issue(&step(
        "kind = \"service\"\nservice = \"S\"\nstart_type = \"disabled\"\noff = \"disabled\"",
    ));
    assert_eq!(issue.field, "steps[0].off");
    // A key step needs `exists`, knows no `off` and takes a well-formed path.
    let issue = first_issue(&step(
        "kind = \"registry_key\"\nhive = \"HKCU\"\nsubkey = 'Software\\X'",
    ));
    assert_eq!(issue.field, "steps[0].exists");
    assert_eq!(issue.kind, IssueKind::MissingField);
    let issue = first_issue(&step(
        "kind = \"registry_key\"\nhive = \"HKCU\"\nsubkey = 'Software\\X'\nexists = true\noff = true",
    ));
    assert_eq!(issue.field, "steps[0].off");
    assert_eq!(issue.kind, IssueKind::UnknownField);
    let issue = first_issue(&step(
        "kind = \"registry_key\"\nhive = \"HKCU\"\nsubkey = 'Software\\\\X'\nexists = true",
    ));
    assert_eq!(issue.field, "steps[0].subkey");
    // The same key twice is one step too many.
    let issue = first_issue(&step(
        "kind = \"registry_key\"\nhive = \"HKCU\"\nsubkey = 'Software\\X'\nexists = true\n\n[[tweak.steps]]\nkind = \"registry_key\"\nhive = \"HKCU\"\nsubkey = 'software\\x'\nexists = false",
    ));
    assert_eq!(issue.kind, IssueKind::DuplicateTarget { first_step: 0 });
}

#[test]
fn the_serialized_form_of_key_steps_and_off_values_loads_back_unchanged() {
    for (text, id) in [(OFF_TWEAK, "off-tweak"), (KEY_TWEAK, "key-tweak")] {
        let original = catalogue(text);
        let tweak = toml::Value::try_from(original.get(id).expect("tweak"))
            .expect("serializes to TOML data");
        let mut root = toml::Table::new();
        root.insert("tweak".to_owned(), toml::Value::Array(vec![tweak]));
        let reloaded = Catalogue::from_root(&root).expect("reloads");
        assert_eq!(reloaded, *original, "{id}");
    }
}

// ---- drift, accept, export and import -----------------------------------------------------

const ADMIN_TWEAK: &str = r#"
[[tweak]]
id = "admin-tweak"
title = "Admin tweak"
description = "Needs administrator rights."
category = "security"
risk = "advanced"
evidence = "anecdotal"
evidence_note = "A test."
requires_admin = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKLM"
subkey = 'Software\VoltTest\Tweaks'
name = "Machine"
value = { dword = 1 }
"#;

/// The registry tweak and the admin tweak in one catalogue.
fn two_tweaks() -> String {
    format!("{REGISTRY_ONLY}{ADMIN_TWEAK}")
}

fn drift_of(engine: &TweakEngine) -> Vec<(String, TweakState)> {
    engine
        .drifted()
        .into_iter()
        .map(|drift| (drift.tweak, drift.state))
        .collect()
}

#[test]
fn a_tweak_changed_after_volt_applied_it_is_drift_and_one_never_applied_is_not() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    // On before Volt ran, then changed: Volt never applied it, so it is not drift.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(0));
    system
        .registry
        .set(&registry_ref("Other"), RegistryValue::Dword(5));
    system
        .registry
        .set(&registry_ref("Other"), RegistryValue::Dword(6));
    assert!(engine.drifted().is_empty());

    let system = FakeSystem::new();
    let engine = self::engine(&system, &catalogue);
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    assert!(engine.drifted().is_empty());

    // Windows resets one value: the tweak is partly in place.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(1));
    assert_eq!(
        drift_of(&engine),
        [("reg-tweak".to_owned(), TweakState::Partial)]
    );
    system
        .registry
        .set(&registry_ref("Other"), RegistryValue::Dword(1));
    assert_eq!(
        drift_of(&engine),
        [("reg-tweak".to_owned(), TweakState::NotApplied)]
    );
}

#[test]
fn reapplying_a_drifted_tweak_goes_through_the_journal_and_clears_the_drift() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(9));
    assert_eq!(engine.drifted().len(), 1);

    let report = engine.apply("reg-tweak", Source::Manual).expect("reapply");
    assert_eq!(report.outcome, ApplyOutcome::Applied);
    assert!(engine.drifted().is_empty());
    // Undo walks back through both batches to the state before Volt first touched the machine.
    assert!(engine.revert("reg-tweak").expect("revert").is_complete());
    assert_eq!(system.registry.get(&registry_ref("Flag")), None);
    assert_eq!(system.registry.get(&registry_ref("Other")), None);
}

#[test]
fn accepting_the_new_state_changes_nothing_but_the_journal() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(9));
    let events_before = system.log.events().len();

    assert_eq!(engine.accept("reg-tweak"), Ok(2));
    // The system was only read; the value the user kept is still there.
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(9))
    );
    assert!(
        system.log.events()[events_before..]
            .iter()
            .all(|event| matches!(event, Event::JournalState(_, EntryState::Accepted)))
    );
    assert_eq!(engine.applied_by_volt("reg-tweak"), Ok(false));
    assert!(engine.drifted().is_empty());
    // The undo record stays but is no longer revertible.
    let batch = system.journal.batches_with_label("reg-tweak").unwrap()[0];
    for entry in system.journal.batch_entries(batch).unwrap() {
        assert_eq!(entry.state, EntryState::Accepted);
    }
    assert_eq!(
        engine.revert("reg-tweak"),
        Err(TweakError::NothingToRevert("reg-tweak".to_owned()))
    );
    // Accepting again has nothing left to accept.
    assert_eq!(
        engine.accept("reg-tweak"),
        Err(TweakError::NotDrifted("reg-tweak".to_owned()))
    );
}

#[test]
fn accept_refuses_a_tweak_that_is_in_place_or_that_volt_did_not_apply() {
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    assert_eq!(
        engine.accept("reg-tweak"),
        Err(TweakError::NotDrifted("reg-tweak".to_owned()))
    );
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    assert_eq!(
        engine.accept("reg-tweak"),
        Err(TweakError::NotDrifted("reg-tweak".to_owned()))
    );
    assert_eq!(
        engine.accept("nope"),
        Err(TweakError::UnknownTweak("nope".to_owned()))
    );
}

#[test]
fn export_lists_only_what_volt_applied_and_is_still_in_place() {
    let system = FakeSystem::new();
    let catalogue = catalogue(&two_tweaks());
    let engine = engine(&system, &catalogue).with_administrator(true);
    // On because the user set it, not Volt: never exported.
    system.registry.set(
        &RegistryValueRef::new(
            RegistryHive::LocalMachine,
            r"Software\VoltTest\Tweaks",
            "Machine",
        ),
        RegistryValue::Dword(1),
    );
    assert!(engine.export_tweaks().is_empty());
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    let ids: Vec<_> = engine.export_tweaks().into_iter().map(|t| t.id).collect();
    assert_eq!(ids, ["reg-tweak"]);
    // Changed since: not in effect, so not exported.
    system
        .registry
        .set(&registry_ref("Flag"), RegistryValue::Dword(9));
    assert!(engine.export_tweaks().is_empty());
}

#[test]
fn a_configuration_file_round_trips_and_refuses_what_it_does_not_know() {
    let file = ConfigFile::new(
        vec![ConfigTweak {
            id: "reg-tweak".to_owned(),
            title: Some("Registry tweak".to_owned()),
        }],
        std::collections::BTreeMap::from([("boost.config".to_owned(), serde_json::json!({}))]),
    );
    let text = file.to_text().expect("text");
    assert!(text.contains("\"format\": \"volt-configuration\""));
    assert!(!text.contains('\r'));
    assert_eq!(ConfigFile::parse(&text), Ok(file));

    assert_eq!(
        ConfigFile::parse("not json"),
        Err(ConfigError::NotAConfiguration)
    );
    assert_eq!(
        ConfigFile::parse(r#"{"format":"other","version":1}"#),
        Err(ConfigError::NotAConfiguration)
    );
    assert_eq!(
        ConfigFile::parse(r#"{"format":"volt-configuration","version":99}"#),
        Err(ConfigError::NewerVersion(99))
    );
    let huge = format!(
        r#"{{"format":"volt-configuration","version":1,"pad":"{}"}}"#,
        "x".repeat(MAX_CONFIG_BYTES)
    );
    assert_eq!(ConfigFile::parse(&huge), Err(ConfigError::TooLarge));
    let many = (0..501)
        .map(|n| format!(r#"{{"id":"t{n}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(
        ConfigFile::parse(&format!(
            r#"{{"format":"volt-configuration","version":1,"tweaks":[{many}]}}"#
        )),
        Err(ConfigError::TooManyTweaks)
    );
}

#[test]
fn a_file_cannot_smuggle_steps_values_or_commands_into_an_import() {
    // Extra fields on a tweak are ignored: only the id is read.
    let file = ConfigFile::parse(
        r#"{"format":"volt-configuration","version":1,"tweaks":[
            {"id":"reg-tweak","steps":[{"kind":"command","apply":{"program":"evil"}}],
             "value":{"dword":666}}]}"#,
    )
    .expect("parses");
    let system = FakeSystem::new();
    let catalogue = catalogue(REGISTRY_ONLY);
    let engine = engine(&system, &catalogue);
    let plan = engine.plan_import(&file.tweaks);
    let ImportAction::Apply(previews) = &plan[0].action else {
        panic!("expected apply, got {plan:?}");
    };
    // The catalogue's own two steps, with the catalogue's values.
    assert_eq!(previews.len(), 2);
    assert!(previews.iter().all(|p| !p.target.contains("666")));
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    assert_eq!(
        system.registry.get(&registry_ref("Flag")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(system.commands.is_on(&detect()), None);
}

#[test]
fn an_import_plan_says_what_each_tweak_would_do_and_changes_nothing() {
    let system = FakeSystem::new();
    let catalogue = catalogue(&two_tweaks());
    let engine = engine(&system, &catalogue);
    let tweaks: Vec<_> = ["reg-tweak", "nope", "admin-tweak", "reg-tweak"]
        .iter()
        .map(|id| ConfigTweak {
            id: (*id).to_owned(),
            title: None,
        })
        .collect();
    let plan = engine.plan_import(&tweaks);
    assert_eq!(plan.len(), 3, "a repeated id is planned once");
    assert!(matches!(&plan[0].action, ImportAction::Apply(steps) if steps.len() == 2));
    assert_eq!(plan[1].action, ImportAction::Skip(ImportSkip::Unknown));
    assert_eq!(
        plan[2].action,
        ImportAction::Skip(ImportSkip::NeedsAdministrator)
    );
    assert!(system.log.events().is_empty(), "planning only reads");
    assert_eq!(system.registry.get(&registry_ref("Flag")), None);

    // With the rights the admin tweak is planned; once applied a tweak is already in place.
    let admin = self::engine(&system, &catalogue).with_administrator(true);
    assert!(matches!(
        admin.plan_import(&tweaks[2..3])[0].action,
        ImportAction::Apply(_)
    ));
    engine.apply("reg-tweak", Source::Manual).expect("apply");
    assert_eq!(
        engine.plan_import(&tweaks[..1])[0].action,
        ImportAction::AlreadyApplied
    );
}

#[test]
fn a_tweak_this_machine_cannot_read_is_skipped_with_the_reason() {
    let catalogue = catalogue(SAMPLE);
    let tweaks = [ConfigTweak {
        id: "sample-tweak".to_owned(),
        title: None,
    }];
    let machine = machine();
    assert!(matches!(
        engine(&machine, &catalogue).plan_import(&tweaks)[0].action,
        ImportAction::Apply(_)
    ));
    // The service the tweak changes does not exist here.
    let bare = FakeSystem::new();
    assert!(matches!(
        &engine(&bare, &catalogue).plan_import(&tweaks)[0].action,
        ImportAction::Skip(ImportSkip::Unavailable(reason)) if !reason.is_empty()
    ));
}
