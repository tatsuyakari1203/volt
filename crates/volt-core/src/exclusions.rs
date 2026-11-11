//! The exclusion list: processes the user protects from memory trimming.
//!
//! Trimming a working set moves a program's pages out of RAM, and the program pays for it by
//! reloading them. An advanced user who runs a game, a database or a build can put its executable
//! on this list; every trim path (Boost, the Memory page, the tray Boost) goes through the
//! [`MemoryCleaner`](crate::MemoryCleaner), which hands the list to the [`Memory`](crate::Memory)
//! port, and the port leaves a listed process alone.
//!
//! A process is matched by its executable name, ignoring case. Names are stored normalised (see
//! [`ExclusionList::normalize`]), so the same program is never listed twice.

use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::settings::{Settings, SettingsExt};

/// The settings key that holds the list.
pub const EXCLUSIONS_SETTINGS_KEY: &str = "memory.exclusions";

/// The most names the list keeps. A limit nobody reaches by hand; it bounds what a damaged
/// settings value can make a trim loop over.
pub const MAX_EXCLUSIONS: usize = 200;

/// The longest executable name Windows allows for a file.
const MAX_NAME_CHARS: usize = 255;

/// Executable names that are never trimmed, in normalised form (lower case, sorted, no
/// duplicates).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExclusionList {
    names: Vec<String>,
}

impl ExclusionList {
    /// An empty list: nothing is protected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Turns what the user typed or picked into the stored form.
    ///
    /// The text is trimmed, a path or quotes around it are dropped (only the file name counts),
    /// it is lower-cased and `.exe` is added when there is no extension, so `Chrome`,
    /// `"C:\Program Files\Google\chrome.exe"` and `chrome.exe` all mean the same thing. Empty
    /// names, names with characters Windows does not allow in a file name and over-long names are
    /// [`PortError::InvalidInput`].
    pub fn normalize(input: &str) -> Result<String, PortError> {
        let trimmed = input.trim().trim_matches('"').trim();
        let file_name = trimmed.rsplit(['\\', '/']).next().unwrap_or(trimmed).trim();
        if file_name.is_empty() {
            return Err(PortError::InvalidInput(
                "the executable name is empty".to_owned(),
            ));
        }
        if file_name.chars().count() > MAX_NAME_CHARS
            || file_name
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(PortError::InvalidInput(
                "the executable name has characters a file name cannot have".to_owned(),
            ));
        }
        let mut name = file_name.to_lowercase();
        if !name.contains('.') {
            name.push_str(".exe");
        }
        Ok(name)
    }

    /// Adds `input` (see [`ExclusionList::normalize`]). Returns whether the list changed: adding a
    /// name that is already there is not an error. A full list refuses with
    /// [`PortError::InvalidInput`].
    pub fn add(&mut self, input: &str) -> Result<bool, PortError> {
        let name = Self::normalize(input)?;
        match self.names.binary_search(&name) {
            Ok(_) => Ok(false),
            Err(at) => {
                if self.names.len() >= MAX_EXCLUSIONS {
                    return Err(PortError::InvalidInput(format!(
                        "the list holds at most {MAX_EXCLUSIONS} programs"
                    )));
                }
                self.names.insert(at, name);
                Ok(true)
            }
        }
    }

    /// Removes `input` (see [`ExclusionList::normalize`]). Returns whether it was on the list.
    pub fn remove(&mut self, input: &str) -> bool {
        let Ok(name) = Self::normalize(input) else {
            return false;
        };
        match self.names.binary_search(&name) {
            Ok(at) => {
                self.names.remove(at);
                true
            }
            Err(_) => false,
        }
    }

    /// Whether a process with this image name (as the process list gives it, such as
    /// `Chrome.exe`) is protected. Case does not matter.
    #[must_use]
    pub fn protects(&self, image_name: &str) -> bool {
        !self.names.is_empty() && self.names.binary_search(&image_name.to_lowercase()).is_ok()
    }

    /// The protected names, sorted.
    #[must_use]
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Whether nothing is protected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    /// The stored list; empty when none is stored. A stored value that cannot be read is an
    /// error, not an empty list: an unreadable list must not turn into "protect nothing" for a
    /// run that is about to trim.
    pub fn load(settings: &dyn Settings) -> Result<Self, PortError> {
        let stored = settings
            .get_typed::<Vec<String>>(EXCLUSIONS_SETTINGS_KEY)?
            .unwrap_or_default();
        // Normalised again, so a hand-edited value still matches and cannot hold duplicates.
        let mut list = Self::new();
        for name in stored.iter().take(MAX_EXCLUSIONS) {
            // A name that is no longer valid cannot match a process; it is dropped.
            let _ = list.add(name);
        }
        Ok(list)
    }

    /// Stores the list.
    pub fn save(&self, settings: &dyn Settings) -> Result<(), PortError> {
        settings.set_typed(EXCLUSIONS_SETTINGS_KEY, &self.names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::FakeSettings;

    #[test]
    fn names_are_normalised_to_one_form() {
        for input in [
            "Chrome",
            " chrome.exe ",
            "CHROME.EXE",
            r#""C:\Program Files\Google\Chrome.exe""#,
            "C:/apps/chrome",
        ] {
            assert_eq!(ExclusionList::normalize(input).as_deref(), Ok("chrome.exe"));
        }
        assert_eq!(
            ExclusionList::normalize("tool.bin").as_deref(),
            Ok("tool.bin")
        );
    }

    #[test]
    fn empty_and_impossible_names_are_refused() {
        for input in ["", "   ", r"C:\apps\", "a|b.exe", "what?.exe", "x\u{7}.exe"] {
            assert!(
                matches!(
                    ExclusionList::normalize(input),
                    Err(PortError::InvalidInput(_))
                ),
                "{input:?}"
            );
        }
        let long = format!("{}.exe", "a".repeat(300));
        assert!(ExclusionList::normalize(&long).is_err());
    }

    #[test]
    fn adding_twice_keeps_one_entry_and_the_list_stays_sorted() {
        let mut list = ExclusionList::new();
        assert_eq!(list.add("Zed"), Ok(true));
        assert_eq!(list.add("alpha.exe"), Ok(true));
        assert_eq!(list.add("ZED.EXE"), Ok(false));
        assert_eq!(list.names(), ["alpha.exe", "zed.exe"]);
    }

    #[test]
    fn a_process_is_protected_whatever_the_case_of_its_name() {
        let mut list = ExclusionList::new();
        list.add("game").expect("add");
        assert!(list.protects("Game.EXE"));
        assert!(!list.protects("game2.exe"));
        assert!(!ExclusionList::new().protects("game.exe"));
    }

    #[test]
    fn removing_forgets_a_name_by_any_spelling() {
        let mut list = ExclusionList::new();
        list.add("game.exe").expect("add");
        assert!(!list.remove("other"));
        assert!(list.remove("GAME"));
        assert!(list.is_empty());
        assert!(!list.remove(""));
    }

    #[test]
    fn a_full_list_refuses_one_more_name() {
        let mut list = ExclusionList::new();
        for index in 0..MAX_EXCLUSIONS {
            list.add(&format!("app{index}")).expect("room");
        }
        assert!(list.add("one-too-many").is_err());
        assert_eq!(list.add("app1"), Ok(false));
    }

    #[test]
    fn the_list_survives_a_save_and_a_load() {
        let settings = FakeSettings::new();
        assert!(ExclusionList::load(&settings).expect("load").is_empty());
        let mut list = ExclusionList::new();
        list.add("game").expect("add");
        list.add("db.exe").expect("add");
        list.save(&settings).expect("save");
        assert_eq!(ExclusionList::load(&settings).expect("load"), list);
    }

    #[test]
    fn a_stored_value_of_the_wrong_shape_is_an_error_and_not_an_empty_list() {
        let settings = FakeSettings::new();
        settings
            .set(EXCLUSIONS_SETTINGS_KEY, &serde_json::json!({"a": 1}))
            .expect("set");
        assert!(ExclusionList::load(&settings).is_err());
    }

    #[test]
    fn a_hand_edited_value_is_normalised_when_loaded() {
        let settings = FakeSettings::new();
        settings
            .set(
                EXCLUSIONS_SETTINGS_KEY,
                &serde_json::json!(["Game", "game.exe", "", "db.exe"]),
            )
            .expect("set");
        let list = ExclusionList::load(&settings).expect("load");
        assert_eq!(list.names(), ["db.exe", "game.exe"]);
    }
}
