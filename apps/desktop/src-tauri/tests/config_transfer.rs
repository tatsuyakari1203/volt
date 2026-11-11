//! Tweak drift and the configuration file, through the functions the Tauri commands call.
//!
//! The wiring is the real one (real engine, real `SQLite` store in a temp directory) over the
//! in-memory fakes of `volt-core`, so nothing here touches this machine's registry, services or
//! files. A test catalogue replaces the real one for the same reason.

use std::path::Path;

use tempfile::TempDir;
use volt_core::fakes::FakeSystem;
use volt_core::{
    ConfigFile, ConfigTweak, RegistryHive, RegistryValue, RegistryValueRef, TweakEngine,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{
    ApplyStatus, ChangeState, TweakStateView, apply_tweak, list_activity, revert_tweak,
};
use volt_desktop_lib::boost::{boost_config_of, set_boost_config_of};
use volt_desktop_lib::exclusions::{add_memory_exclusion_of, memory_exclusions_of};
use volt_desktop_lib::memory::MemoryAreaView;
use volt_desktop_lib::transfer::{
    ImportActionView, ImportStatus, SettingStatus, SkipView, accept_tweak_state_of, apply_import,
    export_configuration_text, list_tweak_drift_of, preview_import, read_config_file,
};
use volt_desktop_lib::wiring::{AppCore, build_core};

const CATALOGUE: &str = r#"
[[tweak]]
id = "flag-tweak"
title = "Flag tweak"
description = "Sets two values."
category = "appearance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "A preference."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Transfer'
name = "Flag"
value = { dword = 0 }

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = 'Software\VoltTest\Transfer'
name = "Other"
value = { dword = 5 }

[[tweak]]
id = "machine-tweak"
title = "Machine tweak"
description = "Needs administrator rights."
category = "security"
risk = "advanced"
evidence = "anecdotal"
evidence_note = "A test."
requires_admin = true

[[tweak.steps]]
kind = "registry_set"
hive = "HKLM"
subkey = 'Software\VoltTest\Transfer'
name = "Machine"
value = { dword = 1 }
"#;

fn value(name: &str) -> RegistryValueRef {
    RegistryValueRef::new(
        RegistryHive::CurrentUser,
        r"Software\VoltTest\Transfer",
        name,
    )
}

fn core_at(system: &FakeSystem, dir: &TempDir) -> AppCore {
    build_core(system.ports(), &dir.path().join("volt.db"), CATALOGUE).expect("core builds")
}

/// A core on a fresh fake machine with its own store.
fn machine() -> (FakeSystem, TempDir, AppCore) {
    let system = FakeSystem::new();
    let dir = TempDir::new().expect("temp dir");
    let core = core_at(&system, &dir);
    (system, dir, core)
}

fn engine_ids(engine: &TweakEngine) -> Vec<String> {
    engine.export_tweaks().into_iter().map(|t| t.id).collect()
}

// ---- drift -------------------------------------------------------------------------------------

#[test]
fn a_tweak_changed_behind_volts_back_is_listed_and_can_be_reapplied() {
    let (system, _dir, core) = machine();
    assert!(list_tweak_drift_of(&core).expect("drift").is_empty());
    apply_tweak(&core, "flag-tweak").expect("apply");
    assert!(list_tweak_drift_of(&core).expect("drift").is_empty());

    // A Windows update resets one value.
    system.registry.set(&value("Flag"), RegistryValue::Dword(1));
    let drift = list_tweak_drift_of(&core).expect("drift");
    assert_eq!(drift.len(), 1);
    assert_eq!(drift[0].id, "flag-tweak");
    assert_eq!(drift[0].state, TweakStateView::Partial);
    assert_eq!(drift[0].steps.len(), 2);
    assert!(!drift[0].steps[0].at_target);
    assert!(drift[0].steps[1].at_target);

    // Reapply is a normal apply, through the journal.
    let result = apply_tweak(&core, "flag-tweak").expect("reapply");
    assert_eq!(result.status, ApplyStatus::Applied);
    assert!(list_tweak_drift_of(&core).expect("drift").is_empty());
    assert_eq!(
        system.registry.get(&value("Flag")),
        Some(RegistryValue::Dword(0))
    );
}

#[test]
fn accepting_the_new_state_keeps_the_value_and_the_history_but_not_the_undo() {
    let (system, _dir, core) = machine();
    apply_tweak(&core, "flag-tweak").expect("apply");
    system.registry.set(&value("Flag"), RegistryValue::Dword(1));

    let result = accept_tweak_state_of(&core, "flag-tweak").expect("accept");
    assert_eq!(result.accepted, 2);
    assert_eq!(result.state, TweakStateView::Partial);
    assert!(list_tweak_drift_of(&core).expect("drift").is_empty());
    // Nothing on the system changed.
    assert_eq!(
        system.registry.get(&value("Flag")),
        Some(RegistryValue::Dword(1))
    );
    // The apply stays in the history, flagged as no longer undoable, with the reason.
    let page = list_activity(&core, 10, None).expect("activity");
    let line = page
        .items
        .iter()
        .find(|item| item.label.as_deref() == Some("flag-tweak"))
        .expect("the apply is still listed");
    assert_eq!(line.state, ChangeState::CannotUndo);
    assert_eq!(
        line.reason_phrase.as_ref().map(|p| p.code.as_str()),
        Some("undo.accepted")
    );
    // Undo has nothing left to restore, and nothing can be accepted twice.
    assert_eq!(
        revert_tweak(&core, "flag-tweak").expect("revert").restored,
        0
    );
    assert!(matches!(
        accept_tweak_state_of(&core, "flag-tweak"),
        Err(IpcError::InvalidInput { .. })
    ));
}

// ---- export and import ---------------------------------------------------------------------------

#[test]
fn a_configuration_made_on_one_machine_previews_and_imports_on_another_through_the_journal() {
    let (_system_a, _dir_a, a) = machine();
    apply_tweak(&a, "flag-tweak").expect("apply");
    set_boost_config_of(
        &a,
        &volt_desktop_lib::boost::BoostConfigView {
            areas: vec![MemoryAreaView::WorkingSets, MemoryAreaView::StandbyList],
            move_temp_to_recycle_bin: true,
        },
    )
    .expect("boost config");
    add_memory_exclusion_of(&a, "game.exe").expect("exclusion");
    let export = export_configuration_text(&a).expect("export");
    assert_eq!(export.tweaks, 1);
    assert_eq!(export.settings, 2);
    assert!(export.file_name.starts_with("volt-configuration-"));
    assert!(
        Path::new(&export.file_name)
            .extension()
            .is_some_and(|ext| ext == "json")
    );
    assert!(!export.text.contains('\r'));
    // What stays on the machine does not travel.
    for left_out in ["ui.language", "clean.chosen_folders", "process.rules"] {
        assert!(!export.text.contains(left_out), "{left_out} was exported");
    }
    let file = ConfigFile::parse(&export.text).expect("the export reads back");

    let (system_b, _dir_b, b) = machine();
    let preview = preview_import(&b, &file).expect("preview");
    // Nothing was changed by looking.
    assert_eq!(system_b.registry.get(&value("Flag")), None);
    assert_eq!(preview.tweaks.len(), 1);
    assert_eq!(preview.tweaks[0].title.as_deref(), Some("Flag tweak"));
    let ImportActionView::Apply { steps } = &preview.tweaks[0].action else {
        panic!("expected apply: {preview:?}");
    };
    assert_eq!(steps.len(), 2);
    assert_eq!(preview.settings.len(), 2);
    assert!(
        preview
            .settings
            .iter()
            .all(|setting| setting.status == SettingStatus::Change)
    );
    assert_eq!(preview.ignored_settings, 0);

    let result = apply_import(&b, &file).expect("import");
    assert_eq!(result.tweaks[0].status, ImportStatus::Applied);
    assert_eq!(result.settings_stored, 2);
    assert_eq!(
        system_b.registry.get(&value("Flag")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(memory_exclusions_of(&b).expect("list"), ["game.exe"]);
    let boost = boost_config_of(&b).expect("boost");
    assert_eq!(boost.areas.len(), 2);
    assert!(boost.move_temp_to_recycle_bin);

    // Imported through the journal: the usual undo works and the history lists it.
    assert_eq!(engine_ids(&b.engine), ["flag-tweak"]);
    assert!(
        list_activity(&b, 10, None)
            .expect("activity")
            .items
            .iter()
            .any(|item| item.label.as_deref() == Some("flag-tweak"))
    );
    revert_tweak(&b, "flag-tweak").expect("undo the import");
    assert_eq!(system_b.registry.get(&value("Flag")), None);

    // Importing it again changes the settings no more.
    let again = preview_import(&b, &file).expect("preview");
    assert!(
        again
            .settings
            .iter()
            .all(|setting| setting.status == SettingStatus::Same)
    );
}

#[test]
fn an_import_skips_what_is_unknown_or_needs_rights_and_never_takes_values_from_the_file() {
    let text = r#"{
      "format": "volt-configuration",
      "version": 1,
      "tweaks": [
        { "id": "flag-tweak", "steps": [{ "kind": "command", "apply": { "program": "evil" } }],
          "value": { "dword": 666 } },
        { "id": "machine-tweak" },
        { "id": "no-such-tweak" },
        { "id": "flag-tweak" }
      ],
      "settings": {
        "boost.config": { "areas": ["NotAnArea"] },
        "memory.exclusions": ["Good", "bad<name", "C:\\Tools\\TOOL.EXE"],
        "ui.language": { "choice": "vi" },
        "clean.chosen_folders": { "x": "C:\\Windows" },
        "made.up": 1
      }
    }"#;
    let file = ConfigFile::parse(text).expect("parses");
    let (system, _dir, core) = machine();
    let preview = preview_import(&core, &file).expect("preview");

    assert_eq!(preview.tweaks.len(), 3, "a repeated id is listed once");
    assert!(matches!(
        &preview.tweaks[0].action,
        ImportActionView::Apply { steps } if steps.len() == 2
    ));
    assert!(matches!(
        &preview.tweaks[1].action,
        ImportActionView::Skip {
            reason: SkipView::NeedsAdministrator,
            ..
        }
    ));
    assert!(matches!(
        &preview.tweaks[2].action,
        ImportActionView::Skip {
            reason: SkipView::Unknown,
            ..
        }
    ));
    assert_eq!(preview.tweaks[2].title, None);
    // Only keys Volt carries are read; the rest is counted and ignored.
    assert_eq!(preview.ignored_settings, 3);
    let boost = preview
        .settings
        .iter()
        .find(|s| s.key == "boost.config")
        .expect("boost");
    assert_eq!(boost.status, SettingStatus::Invalid);
    let exclusions = preview
        .settings
        .iter()
        .find(|s| s.key == "memory.exclusions")
        .expect("exclusions");
    assert_eq!(exclusions.status, SettingStatus::Change);

    let result = apply_import(&core, &file).expect("import");
    let statuses: Vec<_> = result.tweaks.iter().map(|t| t.status).collect();
    assert_eq!(
        statuses,
        [
            ImportStatus::Applied,
            ImportStatus::Skipped,
            ImportStatus::Skipped
        ]
    );
    // The catalogue's values were written, never the file's.
    assert_eq!(
        system.registry.get(&value("Flag")),
        Some(RegistryValue::Dword(0))
    );
    assert_eq!(
        system.registry.get(&value("Other")),
        Some(RegistryValue::Dword(5))
    );
    // Valid names were added in their normal form; the bad one and the invalid Boost value were
    // dropped, and the settings Volt does not carry were not stored.
    assert_eq!(
        memory_exclusions_of(&core).expect("list"),
        ["good.exe", "tool.exe"]
    );
    assert_eq!(result.settings_stored, 1);
    assert!(
        core.settings.get("ui.language").expect("read").is_none()
            && core.settings.get("made.up").expect("read").is_none()
            && core
                .settings
                .get("clean.chosen_folders")
                .expect("read")
                .is_none()
    );
    assert!(core.settings.get("boost.config").expect("read").is_none());
}

#[test]
fn a_tweak_that_already_matches_is_not_applied_again() {
    let (_system, _dir, core) = machine();
    apply_tweak(&core, "flag-tweak").expect("apply");
    let file = ConfigFile::new(
        vec![ConfigTweak {
            id: "flag-tweak".to_owned(),
            title: None,
        }],
        std::collections::BTreeMap::new(),
    );
    let preview = preview_import(&core, &file).expect("preview");
    assert_eq!(preview.tweaks[0].action, ImportActionView::AlreadyApplied);
    let batches_before = list_activity(&core, 50, None)
        .expect("activity")
        .items
        .len();
    let result = apply_import(&core, &file).expect("import");
    assert_eq!(result.tweaks[0].status, ImportStatus::AlreadyApplied);
    assert_eq!(
        list_activity(&core, 50, None)
            .expect("activity")
            .items
            .len(),
        batches_before
    );
}

// ---- reading a file --------------------------------------------------------------------------------

fn write(dir: &Path, name: &str, text: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).expect("write");
    path
}

#[test]
fn a_file_that_is_not_a_configuration_is_refused_with_a_reason() {
    let dir = TempDir::new().expect("temp dir");
    let good = write(
        dir.path(),
        "good.json",
        r#"{"format":"volt-configuration","version":1}"#,
    );
    assert!(read_config_file(&good).is_ok());

    for (name, text) in [
        ("text.json", "hello"),
        ("other.json", r#"{"format":"something-else","version":1}"#),
        (
            "newer.json",
            r#"{"format":"volt-configuration","version":2}"#,
        ),
    ] {
        let path = write(dir.path(), name, text);
        assert!(
            matches!(read_config_file(&path), Err(IpcError::InvalidInput { .. })),
            "{name}"
        );
    }
    let huge = write(dir.path(), "huge.json", &" ".repeat(2 * 1024 * 1024));
    assert!(matches!(
        read_config_file(&huge),
        Err(IpcError::InvalidInput { .. })
    ));
    let binary = dir.path().join("binary.json");
    std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x12]).expect("write");
    assert!(matches!(
        read_config_file(&binary),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        read_config_file(&dir.path().join("missing.json")),
        Err(IpcError::NotFound { .. })
    ));
}
