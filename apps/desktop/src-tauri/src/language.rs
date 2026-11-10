//! The language of the UI, kept in the settings store.
//!
//! The window translates itself: it holds the locale files and applies the choice at once. The
//! shell needs the language only for the tray menu, the tooltip and the Boost notification, which
//! can exist with no window open, so it reads the setting itself. English is the source and the
//! fallback; Vietnamese for those few strings lives in [`crate::tray`] and [`crate::boost`].
//!
//! `System` means "the display language of Windows". The shell reads it from Windows
//! ([`volt_win::display_languages`]) every time it needs it and hands it to the window with the
//! setting, so the window never guesses it from the browser language list and the tray, which
//! can run with no window, always agrees with the window.

use serde::{Deserialize, Serialize};
use specta::Type;
use volt_core::{Settings, SettingsExt};

use crate::IpcError;
use crate::commands::blocking;
use crate::wiring::{AppCore, AppState};

/// The settings key that holds the [`LanguageSetting`].
pub const LANGUAGE_SETTINGS_KEY: &str = "ui.language";

/// A language the app is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
pub enum Lang {
    /// English, the source language and the fallback.
    #[default]
    #[serde(rename = "en")]
    English,
    /// Vietnamese.
    #[serde(rename = "vi")]
    Vietnamese,
}

/// What the user chose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
pub enum LanguageChoice {
    /// Follow the language of Windows.
    #[default]
    #[serde(rename = "system")]
    System,
    /// Always English.
    #[serde(rename = "en")]
    English,
    /// Always Vietnamese.
    #[serde(rename = "vi")]
    Vietnamese,
}

/// The stored language setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct LanguageSetting {
    /// What the user chose.
    pub choice: LanguageChoice,
    /// What `System` stands for: the display language of Windows, filled in when the setting is
    /// read (what was stored is not trusted: the user can change Windows between two runs).
    pub system: Lang,
}

impl Lang {
    /// The language of a BCP 47 name such as `vi-VN`: Vietnamese for `vi` and its regions,
    /// English for everything else (English is the fallback).
    #[must_use]
    pub fn from_tag(tag: &str) -> Self {
        let tag = tag.to_ascii_lowercase();
        if tag == "vi" || tag.starts_with("vi-") {
            Self::Vietnamese
        } else {
            Self::English
        }
    }
}

/// The display language of Windows: its first preferred display language, English when Windows
/// does not say. Read-only.
#[must_use]
pub fn system_language() -> Lang {
    volt_win::display_languages()
        .first()
        .map_or(Lang::English, |tag| Lang::from_tag(tag))
}

impl LanguageSetting {
    /// The same setting with `system` as the language `System` stands for.
    #[must_use]
    pub const fn with_system(self, system: Lang) -> Self {
        Self { system, ..self }
    }

    /// The language to write in: the choice, or the system language for `System`.
    #[must_use]
    pub const fn effective(self) -> Lang {
        match self.choice {
            LanguageChoice::System => self.system,
            LanguageChoice::English => Lang::English,
            LanguageChoice::Vietnamese => Lang::Vietnamese,
        }
    }

    /// The stored setting, or the default (follow the system, English) when none is stored or it
    /// cannot be read: a wrong language is a nuisance, never a reason to fail.
    #[must_use]
    pub fn load(settings: &dyn Settings) -> Self {
        match settings.get_typed::<Self>(LANGUAGE_SETTINGS_KEY) {
            Ok(Some(setting)) => setting,
            Ok(None) | Err(_) => Self::default(),
        }
    }
}

/// The language the shell writes its own few strings in right now.
#[must_use]
pub(crate) fn current(state: &AppState) -> Lang {
    state.core().map_or_else(
        |_| system_language(),
        |core| {
            LanguageSetting::load(core.settings.as_ref())
                .with_system(system_language())
                .effective()
        },
    )
}

/// The saved language setting, with `system` as the language `System` stands for.
pub fn language_of(core: &AppCore, system: Lang) -> Result<LanguageSetting, IpcError> {
    Ok(LanguageSetting::load(core.settings.as_ref()).with_system(system))
}

/// Saves what the user chose, with `system` as what `System` stands for, and returns it.
pub fn set_language_of(
    core: &AppCore,
    choice: LanguageChoice,
    system: Lang,
) -> Result<LanguageSetting, IpcError> {
    let setting = LanguageSetting { choice, system };
    core.settings.set_typed(LANGUAGE_SETTINGS_KEY, &setting)?;
    Ok(setting)
}

/// Tauri command: the saved language setting and the display language of Windows.
#[tauri::command]
#[specta::specta]
pub(crate) async fn language_setting(
    state: tauri::State<'_, AppState>,
) -> Result<LanguageSetting, IpcError> {
    blocking(&state, |core| language_of(core, system_language())).await
}

/// Tauri command: saves the language choice and updates the tray menu at once, so the menu does
/// not wait for the next start.
#[tauri::command]
#[specta::specta]
pub(crate) async fn set_language(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    choice: LanguageChoice,
) -> Result<LanguageSetting, IpcError> {
    let setting = blocking(&state, move |core| {
        set_language_of(core, choice, system_language())
    })
    .await?;
    crate::tray::apply_language(&app, setting.effective());
    Ok(setting)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_language_name_maps_to_vietnamese_or_the_english_fallback() {
        for tag in ["vi", "vi-VN", "VI-vn"] {
            assert_eq!(Lang::from_tag(tag), Lang::Vietnamese, "{tag}");
        }
        for tag in ["en-US", "fr-FR", "vil", "", "viet"] {
            assert_eq!(Lang::from_tag(tag), Lang::English, "{tag}");
        }
    }

    #[test]
    fn a_stored_system_language_is_replaced_by_the_one_windows_reports() {
        let stored = LanguageSetting {
            choice: LanguageChoice::System,
            system: Lang::English,
        };
        assert_eq!(
            stored.with_system(Lang::Vietnamese).effective(),
            Lang::Vietnamese
        );
    }

    #[test]
    fn the_effective_language_follows_the_choice_and_falls_back_to_the_system_one() {
        let system = LanguageSetting {
            choice: LanguageChoice::System,
            system: Lang::Vietnamese,
        };
        assert_eq!(system.effective(), Lang::Vietnamese);
        let english = LanguageSetting {
            choice: LanguageChoice::English,
            system: Lang::Vietnamese,
        };
        assert_eq!(english.effective(), Lang::English);
        let vietnamese = LanguageSetting {
            choice: LanguageChoice::Vietnamese,
            system: Lang::English,
        };
        assert_eq!(vietnamese.effective(), Lang::Vietnamese);
        assert_eq!(LanguageSetting::default().effective(), Lang::English);
    }

    #[test]
    fn the_setting_is_stored_as_short_language_codes() {
        let setting = LanguageSetting {
            choice: LanguageChoice::Vietnamese,
            system: Lang::English,
        };
        let json = serde_json::to_string(&setting).expect("serialize");
        assert_eq!(json, r#"{"choice":"vi","system":"en"}"#);
        let back: LanguageSetting = serde_json::from_str(r#"{"choice":"system"}"#).expect("parse");
        assert_eq!(back, LanguageSetting::default());
    }
}
