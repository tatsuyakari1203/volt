//! The language of the UI, kept in the settings store.
//!
//! The window translates itself: it holds the locale files and applies the choice at once. The
//! shell needs the language only for the tray menu, the tooltip and the Boost notification, which
//! can exist with no window open, so it reads the setting itself. English is the source and the
//! fallback; Vietnamese for those few strings lives in [`crate::tray`] and [`crate::boost`].
//!
//! `System` means "the language of Windows". Only the window can tell (the `WebView` knows the
//! display language), so it reports what `System` resolves to every time it starts and the shell
//! keeps that value for the times it runs without a window.

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
    /// What `System` resolved to the last time a window ran.
    pub system: Lang,
}

impl LanguageSetting {
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
    state.core().map_or(Lang::English, |core| {
        LanguageSetting::load(core.settings.as_ref()).effective()
    })
}

/// The saved language setting.
pub fn language_of(core: &AppCore) -> Result<LanguageSetting, IpcError> {
    Ok(LanguageSetting::load(core.settings.as_ref()))
}

/// Saves what the user chose and what `System` resolves to, and returns what is stored.
pub fn set_language_of(
    core: &AppCore,
    choice: LanguageChoice,
    system: Lang,
) -> Result<LanguageSetting, IpcError> {
    let setting = LanguageSetting { choice, system };
    core.settings.set_typed(LANGUAGE_SETTINGS_KEY, &setting)?;
    Ok(setting)
}

/// Tauri command: the saved language setting.
#[tauri::command]
#[specta::specta]
pub(crate) async fn language_setting(
    state: tauri::State<'_, AppState>,
) -> Result<LanguageSetting, IpcError> {
    blocking(&state, language_of).await
}

/// Tauri command: saves the language choice and what `System` resolves to on this PC.
#[tauri::command]
#[specta::specta]
pub(crate) async fn set_language(
    state: tauri::State<'_, AppState>,
    choice: LanguageChoice,
    system: Lang,
) -> Result<LanguageSetting, IpcError> {
    blocking(&state, move |core| set_language_of(core, choice, system)).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
