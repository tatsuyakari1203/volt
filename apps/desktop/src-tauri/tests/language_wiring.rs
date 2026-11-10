//! The language setting over a real core built on the in-memory fakes and a throwaway store.
//! Nothing here touches this machine.

use std::sync::Arc;

use tempfile::TempDir;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::FakeSystem;
use volt_desktop_lib::catalogue::{EMBEDDED_CATALOGUE, EMBEDDED_CLEANING_RULES};
use volt_desktop_lib::language::{
    LANGUAGE_SETTINGS_KEY, Lang, LanguageChoice, LanguageSetting, language_of, set_language_of,
};
use volt_desktop_lib::wiring::{AppCore, CleaningSetup, build_core_with_cleaning};

fn core_in(dir: &TempDir) -> AppCore {
    build_core_with_cleaning(
        FakeSystem::new().ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
        CleaningSetup {
            rules_toml: EMBEDDED_CLEANING_RULES,
            folders: Arc::new(FakeKnownFolders::new()),
        },
    )
    .expect("core builds")
}

#[test]
fn with_nothing_saved_the_language_follows_the_system_and_defaults_to_english() {
    let dir = TempDir::new().expect("temp dir");
    let core = core_in(&dir);
    let setting = language_of(&core).expect("reads");
    assert_eq!(setting, LanguageSetting::default());
    assert_eq!(setting.choice, LanguageChoice::System);
    assert_eq!(setting.effective(), Lang::English);
}

#[test]
fn a_saved_choice_and_the_system_language_survive_a_restart() {
    let dir = TempDir::new().expect("temp dir");
    {
        let core = core_in(&dir);
        let stored =
            set_language_of(&core, LanguageChoice::System, Lang::Vietnamese).expect("saves");
        assert_eq!(stored.effective(), Lang::Vietnamese);
    }
    // A new core on the same database: the tray reads this at the next start.
    let core = core_in(&dir);
    let setting = language_of(&core).expect("reads");
    assert_eq!(setting.choice, LanguageChoice::System);
    assert_eq!(setting.system, Lang::Vietnamese);
    assert_eq!(setting.effective(), Lang::Vietnamese);

    set_language_of(&core, LanguageChoice::English, Lang::Vietnamese).expect("saves");
    assert_eq!(
        language_of(&core).expect("reads").effective(),
        Lang::English
    );
}

#[test]
fn an_unreadable_stored_value_gives_the_default_instead_of_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let core = core_in(&dir);
    core.settings
        .set(LANGUAGE_SETTINGS_KEY, &serde_json::json!("klingon"))
        .expect("stores");
    assert_eq!(
        language_of(&core).expect("reads"),
        LanguageSetting::default()
    );
}

#[test]
fn the_bindings_describe_the_language_commands() {
    let dir = TempDir::new().expect("temp dir");
    let file = dir.path().join("bindings.ts");
    volt_desktop_lib::export_bindings(&file).expect("bindings export");
    let text = std::fs::read_to_string(&file).expect("read");
    for name in [
        "languageSetting",
        "setLanguage",
        "export type LanguageChoice",
    ] {
        assert!(text.contains(name), "{name} missing");
    }
}
