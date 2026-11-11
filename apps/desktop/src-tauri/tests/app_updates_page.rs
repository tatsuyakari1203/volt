//! The app update commands over a real core and a fake winget: checking, installing one update
//! and what is written to the activity log. Nothing here runs winget or touches this machine's
//! apps.

use std::sync::Arc;

use volt_core::apps::fakes::FakeWinget;
use volt_core::fakes::FakeSystem;
use volt_core::{SystemClock, UpdateManager};
use volt_desktop_lib::api::list_activity;
use volt_desktop_lib::app_updates::{UpdateOutcomeView, check_app_updates_of, update_app_of};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::wiring::{AppCore, build_core};

const TABLE: &str = "\
Name            Id             Version  Available  Source
-----------------------------------------------------------
Example Editor  Acme.Editor    1.2.3    1.3.0      winget
Sample Tool     Contoso.Tool   4.0      4.1.2      winget
";

fn core(winget: &Arc<FakeWinget>) -> (AppCore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    let system = FakeSystem::new();
    let core = build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    let manager = UpdateManager::new(winget.clone(), Arc::new(SystemClock))
        .with_activity_log(core.activity.clone());
    (core.with_updates(manager), dir)
}

#[test]
fn without_the_manager_the_commands_are_unsupported() {
    let dir = tempfile::tempdir().expect("temp dir");
    let core = build_core(
        FakeSystem::new().ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    assert!(check_app_updates_of(&core).is_err());
}

#[test]
fn a_check_lists_the_updates_and_a_missing_winget_is_an_answer_with_a_fix() {
    let winget = Arc::new(FakeWinget::new());
    let (core, _dir) = core(&winget);
    let missing = check_app_updates_of(&core).expect("an answer");
    let problem = missing.problem.expect("a problem");
    assert!(missing.updates.is_empty());
    assert!(!problem.fix.is_empty());
    assert!(!problem.agreements);

    winget.lists(0, TABLE);
    let found = check_app_updates_of(&core).expect("an answer");
    assert!(found.problem.is_none());
    assert_eq!(found.updates.len(), 2);
    assert_eq!(found.updates[0].id, "Acme.Editor");
}

#[test]
fn an_update_is_installed_by_id_and_recorded_as_not_undoable() {
    let winget = Arc::new(FakeWinget::new());
    winget.lists(0, TABLE);
    winget.upgrade_exits("Contoso.Tool", 3010);
    let (core, _dir) = core(&winget);
    check_app_updates_of(&core).expect("check");

    let done = update_app_of(&core, "Acme.Editor").expect("update");
    assert_eq!(done.outcome, UpdateOutcomeView::Updated);
    assert!(done.recorded);
    let restart = update_app_of(&core, "Contoso.Tool").expect("update");
    assert_eq!(restart.outcome, UpdateOutcomeView::NeedsRestart);

    assert!(update_app_of(&core, "Not.There").is_err());
    let page = list_activity(&core, 10, None).expect("activity");
    assert_eq!(page.items.len(), 2);
    let texts: Vec<_> = page
        .items
        .iter()
        .map(|item| item.summary.as_str())
        .collect();
    assert!(
        texts.contains(&"Updated Example Editor to 1.3.0 with winget (cannot be undone by Volt)")
    );
    assert!(
        page.items
            .iter()
            .all(|item| item.entry.is_none() && item.label.as_deref() == Some("apps.update"))
    );
}
