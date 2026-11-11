//! Folders the user chose for rules that need one (a `ComfyUI` install, for its temp folder).
//!
//! The choice is kept in the settings store under [`CHOSEN_FOLDERS_SETTING`], one JSON object
//! that maps a rule id to a folder. A choice is only a starting point: every scan and every clean
//! judges the rule's root below it again with the same safety checks as any other root.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

use super::root::{RootError, resolve_root};
use super::rule::CleaningRule;
use super::{CleanEngine, UnknownRule};
use crate::activity::error_class;
use crate::ports::EntryKind;

/// The settings key that holds the chosen folders: an object from rule id to absolute folder.
pub const CHOSEN_FOLDERS_SETTING: &str = "clean.chosen_folders";

/// Why a folder could not be chosen for a rule, or forgotten.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum FolderError {
    /// The rule is not known.
    #[error(transparent)]
    UnknownRule(#[from] UnknownRule),
    /// The rule does not work in a folder the user chooses.
    #[error("rule \"{0}\" does not work in a folder you choose")]
    NotChoosable(String),
    /// The engine has no settings to keep the choice in.
    #[error("the choice cannot be saved in this build")]
    NoSettings,
    /// The folder is not a place the rule may clean in.
    #[error("this folder cannot be used: {0}")]
    Refused(RootError),
    /// The folder does not exist, is not a plain folder (a link, a file) or has a name that is
    /// not text.
    #[error("this is not an existing, plain folder")]
    NotAFolder,
    /// The settings could not be read or written; the text is the error class.
    #[error("the choice could not be saved ({0})")]
    Settings(&'static str),
}

impl CleanEngine {
    /// The folder chosen for `rule`, or `None` when the rule needs none or none is chosen. A
    /// setting that cannot be read is the error class.
    pub(super) fn chosen_folder_of(
        &self,
        rule: &CleaningRule,
    ) -> Result<Option<PathBuf>, &'static str> {
        if !rule.needs_chosen_folder() {
            return Ok(None);
        }
        let map = self.read_chosen()?;
        Ok(map.get(&rule.id).and_then(Value::as_str).map(PathBuf::from))
    }

    fn read_chosen(&self) -> Result<Map<String, Value>, &'static str> {
        let Some(settings) = &self.settings else {
            return Ok(Map::new());
        };
        match settings.get(CHOSEN_FOLDERS_SETTING) {
            Ok(None) => Ok(Map::new()),
            Ok(Some(Value::Object(map))) => Ok(map),
            Ok(Some(_)) => Err("invalid input"),
            Err(error) => Err(error_class(&error)),
        }
    }

    /// Whether the chosen folder has every marker the rule declares. Read-only: only whether each
    /// name exists directly in the folder is asked. A marker that is a link, or cannot be read,
    /// counts as missing.
    pub(super) fn check_install(
        &self,
        rule: &CleaningRule,
        folder: &Path,
    ) -> Result<(), RootError> {
        let missing: Vec<String> = rule
            .install_markers
            .iter()
            .filter(|name| {
                !matches!(
                    self.file_system.inspect(&folder.join(name)),
                    Ok(Some(info)) if info.kind != EntryKind::ReparsePoint
                )
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(RootError::NotInstall(missing))
        }
    }

    /// The root of a rule, judged now: [`resolve_root`] plus, for a rule in a chosen folder, the
    /// check that the folder still looks like the tool's install.
    pub(super) fn resolve_checked(
        &self,
        rule: &CleaningRule,
        chosen: Option<&Path>,
    ) -> Result<super::root::ResolvedRoot, RootError> {
        let resolved = resolve_root(rule, self.folders.as_ref(), chosen)?;
        if let Some(folder) = chosen.filter(|_| rule.needs_chosen_folder()) {
            self.check_install(rule, folder)?;
        }
        Ok(resolved)
    }

    fn choosable(&self, rule_id: &str) -> Result<&CleaningRule, FolderError> {
        let rule = self
            .rules
            .get(rule_id)
            .ok_or_else(|| UnknownRule(rule_id.to_owned()))?;
        if !rule.needs_chosen_folder() {
            return Err(FolderError::NotChoosable(rule_id.to_owned()));
        }
        Ok(rule)
    }

    /// The folder the user chose for the rule, if any.
    pub fn chosen_folder(&self, rule_id: &str) -> Result<Option<PathBuf>, FolderError> {
        let rule = self.choosable(rule_id)?;
        self.chosen_folder_of(rule).map_err(FolderError::Settings)
    }

    /// Chooses `folder` for the rule and keeps the choice in the settings, replacing an earlier
    /// one. Refused: a rule that needs no folder, a path that is not absolute, a folder whose
    /// rule root would be refused (a drive root, the profile, a system folder, or in an output
    /// folder), a folder that does not exist as a plain folder and a folder that lacks the files or
    /// folders the rule says an install of the tool has ([`RootError::NotInstall`]). Nothing is scanned or
    /// changed in the folder.
    pub fn choose_folder(&self, rule_id: &str, folder: &Path) -> Result<(), FolderError> {
        let rule = self.choosable(rule_id)?;
        let Some(settings) = &self.settings else {
            return Err(FolderError::NoSettings);
        };
        let text = folder.to_str().ok_or(FolderError::NotAFolder)?;
        if !folder.is_absolute() {
            return Err(FolderError::Refused(RootError::NotAbsolute));
        }
        resolve_root(rule, self.folders.as_ref(), Some(folder)).map_err(FolderError::Refused)?;
        match self.file_system.inspect(folder) {
            Ok(Some(info)) if info.kind == EntryKind::Directory => {}
            Ok(_) => return Err(FolderError::NotAFolder),
            Err(error) => return Err(FolderError::Settings(error_class(&error))),
        }
        self.check_install(rule, folder)
            .map_err(FolderError::Refused)?;
        let mut map = self.read_chosen().map_err(FolderError::Settings)?;
        map.insert(rule.id.clone(), Value::String(text.to_owned()));
        settings
            .set(CHOSEN_FOLDERS_SETTING, &Value::Object(map))
            .map_err(|error| FolderError::Settings(error_class(&error)))
    }

    /// Forgets the folder chosen for the rule. Nothing happens when none was chosen. The folder
    /// itself is never touched.
    pub fn forget_folder(&self, rule_id: &str) -> Result<(), FolderError> {
        let rule = self.choosable(rule_id)?;
        let Some(settings) = &self.settings else {
            return Err(FolderError::NoSettings);
        };
        let mut map = self.read_chosen().map_err(FolderError::Settings)?;
        if map.remove(&rule.id).is_none() {
            return Ok(());
        }
        let result = if map.is_empty() {
            settings.remove(CHOSEN_FOLDERS_SETTING).map(|_| ())
        } else {
            settings.set(CHOSEN_FOLDERS_SETTING, &Value::Object(map))
        };
        result.map_err(|error| FolderError::Settings(error_class(&error)))
    }
}
