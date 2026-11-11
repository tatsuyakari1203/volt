//! Profiles through the functions the Tauri commands and the timer call, over a real core built on
//! the in-memory fakes and a throwaway store. Nothing here touches this machine: the registry,
//! the process list, the fullscreen and battery signals and the clock are fakes.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::fakes::{FakeEnvironment, FakeSystem, ManualClock};
use volt_core::{
    Automation, ConfigFile, Processes, Profiles, RegistryHive, RegistryValue, RegistryValueRef,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{ActivitySource, RiskLevel, list_activity};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::language::Lang;
use volt_desktop_lib::profiles::{
    AppTriggerView, StepStatusView, SwitchActionView, delete_profile_of, list_profiles_of,
    notification_body, poll_of, preview_switch_of, save_profile_of, switch_of, tooltip_line,
    undo_switch_of,
};
use volt_desktop_lib::transfer::{
    SettingStatus, apply_import, export_configuration_text, preview_import,
};
use volt_desktop_lib::wiring::{AppCore, build_core};

const NOW: i64 = 1_800_000_000_000;

struct Rig {
    core: AppCore,
    system: FakeSystem,
    env: Arc<FakeEnvironment>,
    _dir: TempDir,
}

fn rig() -> Rig {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    let core = build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    let env = Arc::new(FakeEnvironment::new());
    let clock = Arc::new(ManualClock::new(NOW));
    let automation = Automation::new(
        core.settings.clone(),
        core.activity.clone(),
        clock.clone(),
        env.clone(),
    );
    let profiles = Profiles::new(
        core.settings.clone(),
        core.activity.clone(),
        clock,
        core.engine.clone(),
        system.processes.clone(),
    );
    Rig {
        core: core.with_automation(automation).with_profiles(profiles),
        system,
        env,
        _dir: dir,
    }
}

fn registry_value(system: &FakeSystem, subkey: &str, name: &str) -> Option<RegistryValue> {
    system.registry.get(&RegistryValueRef::new(
        RegistryHive::CurrentUser,
        subkey,
        name,
    ))
}

const EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";

fn tweaks_of(profile: &str, view: &volt_desktop_lib::profiles::ProfilesView) -> Vec<String> {
    view.profiles
        .iter()
        .find(|p| p.id == profile)
        .expect("profile")
        .tweaks
        .iter()
        .map(|t| t.id.clone())
        .collect()
}

#[test]
fn the_suggested_profiles_are_real_tweaks_without_administrator_rights_and_none_is_active() {
    let rig = rig();
    let view = list_profiles_of(&rig.core).expect("list");
    assert!(view.suggested);
    assert_eq!(view.active, None);
    let names: Vec<&str> = view.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Gaming", "Work", "AI workload", "Battery"]);
    for profile in &view.profiles {
        // A preview needs every tweak to exist and to be usable without administrator rights.
        let preview = preview_switch_of(&rig.core, Some(&profile.id)).expect("preview");
        assert!(
            preview.left_out.is_empty(),
            "{}: {:?}",
            profile.id,
            preview.left_out
        );
        assert_eq!(
            preview.items.len() + preview.in_place as usize,
            profile.tweaks.len(),
            "{}",
            profile.id
        );
    }
    // Looking changed nothing.
    assert_eq!(registry_value(&rig.system, EXPLORER, "HideFileExt"), None);
}

#[test]
fn the_preview_names_risky_tweaks_and_a_switch_follows_it_and_is_undone_as_one() {
    let rig = rig();
    let preview = preview_switch_of(&rig.core, Some("gaming")).expect("preview");
    let risky: Vec<&str> = preview
        .items
        .iter()
        .filter(|item| item.risk != RiskLevel::Safe)
        .map(|item| item.tweak.as_str())
        .collect();
    assert_eq!(risky, ["end-hung-apps-faster"]);
    assert!(
        preview
            .items
            .iter()
            .all(|item| item.action == SwitchActionView::TurnOn && !item.steps.is_empty())
    );
    // Gaming turns the memory rule on.
    let automation = preview.automation.expect("automation change");
    assert!(!automation.before.memory.enabled && automation.after.memory.enabled);

    let result = switch_of(&rig.core, Some("gaming")).expect("switch");
    assert!(result.complete);
    assert!(
        result
            .steps
            .iter()
            .all(|s| s.status == StepStatusView::Done)
    );
    let after = list_profiles_of(&rig.core).expect("list");
    assert_eq!(after.active.as_deref(), Some("gaming"));
    let last = after.last_switch.expect("recorded");
    assert_eq!(last.applied.len(), 5);
    assert_eq!(last.source, ActivitySource::Manual);
    assert!(last.automation_changed);

    // Work: Gaming's tweaks are turned off from the journal, Work's applied; the preview of a
    // turn-off says what is put back.
    let to_work = preview_switch_of(&rig.core, Some("work")).expect("preview");
    let off = to_work
        .items
        .iter()
        .find(|item| item.action == SwitchActionView::TurnOff)
        .expect("a turn-off");
    assert!(!off.restores.is_empty());
    let switched = switch_of(&rig.core, Some("work")).expect("switch");
    assert!(switched.complete);
    assert_eq!(
        list_profiles_of(&rig.core).expect("list").active.as_deref(),
        Some("work")
    );

    let undone = undo_switch_of(&rig.core).expect("undo");
    assert!(undone.complete);
    let state = list_profiles_of(&rig.core).expect("list");
    assert_eq!(state.active.as_deref(), Some("gaming"));
    assert!(state.last_switch.is_none());
    assert!(matches!(
        undo_switch_of(&rig.core),
        Err(IpcError::NotFound { .. })
    ));
    // The switch left a line in the history next to the tweaks it changed.
    let page = list_activity(&rig.core, 100, None).expect("activity");
    assert!(
        page.items
            .iter()
            .any(|item| item.label.as_deref() == Some("profile.switch"))
    );
}

#[test]
fn a_profile_cannot_hold_a_tweak_that_needs_administrator_rights() {
    let rig = rig();
    let mut view = list_profiles_of(&rig.core).expect("list").profiles[0].clone();
    view.id = String::new();
    view.name = "Mine".to_owned();
    view.tweaks = vec![volt_desktop_lib::profiles::ProfileTweakView {
        id: "turn-off-search-indexing".to_owned(),
        enabled: true,
    }];
    assert!(matches!(
        save_profile_of(&rig.core, &view),
        Err(IpcError::InvalidInput { .. })
    ));
    view.tweaks.clear();
    view.trigger = Some(AppTriggerView {
        exe: "game".to_owned(),
        return_on_exit: true,
    });
    assert!(matches!(
        save_profile_of(&rig.core, &view),
        Err(IpcError::InvalidInput { .. })
    ));
    view.trigger = None;
    let saved = save_profile_of(&rig.core, &view).expect("saves");
    assert!(!saved.suggested);
    assert_eq!(saved.profiles.len(), 5);
    assert!(delete_profile_of(&rig.core, "profile-1").is_ok());
}

#[test]
fn an_app_starting_activates_its_profile_and_the_tray_says_so() {
    let rig = rig();
    let mut gaming = list_profiles_of(&rig.core).expect("list").profiles[0].clone();
    gaming.trigger = Some(AppTriggerView {
        exe: "Game.exe".to_owned(),
        return_on_exit: true,
    });
    save_profile_of(&rig.core, &gaming).expect("saves");
    assert!(volt_desktop_lib::profiles::watching(&rig.core));

    assert!(poll_of(&rig.core).expect("poll").is_empty());
    rig.system.processes.add(7, "game.exe", 1 << 20);
    let events = poll_of(&rig.core).expect("poll");
    assert_eq!(events.len(), 1);
    assert_eq!(
        list_profiles_of(&rig.core).expect("list").active.as_deref(),
        Some("gaming")
    );
    let line = tooltip_line(&rig.core, &events[0], Lang::English).expect("tooltip");
    assert_eq!(line, "Volt: profile Gaming is on");
    let body = notification_body(&rig.core, &events[0], Lang::English).expect("notice");
    assert!(body.contains("Gaming"), "{body}");
    // The automatic changes are recorded as automation.
    let page = list_activity(&rig.core, 100, None).expect("activity");
    assert!(
        page.items
            .iter()
            .any(|item| item.source == ActivitySource::Automation)
    );

    rig.system.processes.terminate(7, 0).expect("exits");
    let events = poll_of(&rig.core).expect("poll");
    assert_eq!(events.len(), 1);
    assert_eq!(list_profiles_of(&rig.core).expect("list").active, None);
    assert_eq!(registry_value(&rig.system, EXPLORER, "HideFileExt"), None);
}

#[test]
fn an_automatic_switch_respects_the_pause_rules() {
    let rig = rig();
    let mut gaming = list_profiles_of(&rig.core).expect("list").profiles[0].clone();
    gaming.trigger = Some(AppTriggerView {
        exe: "game.exe".to_owned(),
        return_on_exit: false,
    });
    save_profile_of(&rig.core, &gaming).expect("saves");
    assert!(poll_of(&rig.core).expect("poll").is_empty());
    rig.env.set_fullscreen(true);
    rig.system.processes.add(7, "game.exe", 1);
    let events = poll_of(&rig.core).expect("poll");
    assert_eq!(events.len(), 1);
    assert!(tooltip_line(&rig.core, &events[0], Lang::English).is_none());
    let view = list_profiles_of(&rig.core).expect("list");
    assert_eq!(view.active, None);
    assert!(view.last_skip.is_some());
    assert_eq!(registry_value(&rig.system, EXPLORER, "HideFileExt"), None);
}

#[test]
fn profiles_travel_in_the_configuration_as_data_naming_known_ids_only() {
    let a = rig();
    let mut mine = list_profiles_of(&a.core).expect("list").profiles[1].clone();
    mine.id = String::new();
    mine.name = "Mine".to_owned();
    mine.trigger = Some(AppTriggerView {
        exe: "editor.exe".to_owned(),
        return_on_exit: true,
    });
    save_profile_of(&a.core, &mine).expect("saves");
    let export = export_configuration_text(&a.core).expect("export");
    assert!(export.text.contains("profiles.list"), "{}", export.text);
    assert!(!export.text.contains('\r'));
    // The active profile is not part of it.
    switch_of(&a.core, Some("profile-1")).expect("switch");
    assert!(
        !export_configuration_text(&a.core)
            .expect("export")
            .text
            .contains("active")
    );

    // A file edited by hand: a tweak this build does not have, one that needs administrator
    // rights, a folder in the app name and an id that is not an id.
    let mut file = ConfigFile::parse(&export.text).expect("reads back");
    let stored = file.settings.get_mut("profiles.list").expect("profiles");
    let list = stored.as_array_mut().expect("a list");
    let mut wild = list[list.len() - 1].clone();
    wild["id"] = "wild".into();
    wild["tweaks"] = serde_json::json!([
        { "id": "no-such-tweak", "enabled": true },
        { "id": "turn-off-search-indexing", "enabled": true },
        { "id": "show-file-extensions", "enabled": true }
    ]);
    wild["trigger"] = serde_json::json!({ "exe": "C:\\x\\y.exe", "return_on_exit": true });
    let mut bad = wild.clone();
    bad["id"] = "Not An Id".into();
    list.push(wild);
    list.push(bad);

    let b = rig();
    let preview = preview_import(&b.core, &file).expect("preview");
    let setting = preview
        .settings
        .iter()
        .find(|s| s.key == "profiles.list")
        .expect("profiles are known");
    assert_eq!(setting.status, SettingStatus::Change);
    assert_eq!(
        setting.new_phrase.as_ref().map(|p| p.code.as_str()),
        Some("transfer.profiles")
    );
    // Looking changed nothing.
    assert!(list_profiles_of(&b.core).expect("list").suggested);

    let result = apply_import(&b.core, &file).expect("import");
    assert_eq!(result.settings_stored, 1);
    let view = list_profiles_of(&b.core).expect("list");
    assert_eq!(view.active, None, "an import activates nothing");
    assert_eq!(tweaks_of("wild", &view), ["show-file-extensions"]);
    let wild = view.profiles.iter().find(|p| p.id == "wild").expect("kept");
    assert_eq!(wild.trigger, None);
    assert!(view.profiles.iter().any(|p| p.name == "Mine"));
    assert!(!view.profiles.iter().any(|p| p.id == "Not An Id"));
    // Importing the same file again changes nothing.
    let again = preview_import(&b.core, &file).expect("preview");
    let same = again.settings.iter().find(|s| s.key == "profiles.list");
    assert_eq!(same.map(|s| s.status), Some(SettingStatus::Same));
}

#[test]
fn a_switch_that_stops_part_way_says_what_was_and_was_not_changed() {
    let rig = rig();
    // The second tweak of Work cannot be written.
    rig.system.registry.fail_on(
        &RegistryValueRef::new(
            RegistryHive::CurrentUser,
            r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced",
            "ShowSecondsInSystemClock",
        ),
        volt_core::PortError::AccessDenied("test".to_owned()),
    );
    let result = switch_of(&rig.core, Some("work")).expect("switch");
    assert!(!result.complete);
    let statuses: Vec<StepStatusView> = result.steps.iter().map(|s| s.status).collect();
    assert_eq!(statuses[0], StepStatusView::Done);
    assert_ne!(statuses[1], StepStatusView::Done);
    assert_eq!(
        *statuses.last().expect("steps"),
        StepStatusView::NotAttempted
    );
    assert!(result.steps[1].detail.is_some());
    let view = list_profiles_of(&rig.core).expect("list");
    assert_eq!(view.active, None);
    assert_eq!(
        view.last_switch.map(|r| (r.complete, r.applied.len())),
        Some((false, 1))
    );
    // What it did can be undone, and nothing is left half-written.
    assert!(undo_switch_of(&rig.core).expect("undo").complete);
    assert_eq!(registry_value(&rig.system, EXPLORER, "HideFileExt"), None);
}
