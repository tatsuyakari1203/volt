//! Carrying the tweaks to another machine: the configuration file and the import plan.
//!
//! The file is data. It names tweaks by id and says nothing about what a tweak does: the steps,
//! values, paths and commands all come from this machine's [`Catalogue`](super::Catalogue), so a
//! file can only ask for a tweak Volt already knows, never define or change one. An import is
//! planned first (read-only, see [`TweakEngine::plan_import`]) and each tweak is then applied
//! through [`TweakEngine::apply`], so every change is journaled and can be undone like any other.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::engine::{TweakEngine, TweakState};
use crate::plan::Preview;

/// The `format` field of a configuration file.
pub const CONFIG_FORMAT: &str = "volt-configuration";

/// The file version this build writes and reads. A newer file is refused, not guessed at.
pub const CONFIG_VERSION: u32 = 1;

/// The largest file Volt reads, in bytes. A configuration is a few kilobytes.
pub const MAX_CONFIG_BYTES: usize = 1 << 20;

/// The most tweaks one file may name.
pub const MAX_CONFIG_TWEAKS: usize = 500;

/// The longest tweak id shown back to the user: a longer one cannot be a real id.
const MAX_SHOWN_ID_CHARS: usize = 64;

/// Why a configuration file could not be used. Nothing was changed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The text is larger than [`MAX_CONFIG_BYTES`].
    #[error("the file is too large to be a Volt configuration")]
    TooLarge,
    /// The text is not a Volt configuration file.
    #[error("this is not a Volt configuration file")]
    NotAConfiguration,
    /// The file was written by a newer Volt.
    #[error("this file was written by a newer version of Volt (file version {0})")]
    NewerVersion(u32),
    /// The file names more than [`MAX_CONFIG_TWEAKS`] tweaks.
    #[error("the file names too many tweaks")]
    TooManyTweaks,
    /// The file could not be written as text.
    #[error("the configuration could not be written: {0}")]
    Write(String),
}

/// The two fields read first, so a file of another kind or a newer version is named as such
/// before its body is looked at.
#[derive(Deserialize)]
struct Header {
    format: String,
    version: u32,
}

/// A tweak named in a configuration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigTweak {
    /// The tweak id: the only thing an import reads.
    pub id: String,
    /// The tweak's name when the file was written, for a person reading it. Ignored on import.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A Volt configuration file: the tweaks that were applied and the settings worth carrying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigFile {
    /// Always [`CONFIG_FORMAT`].
    pub format: String,
    /// The file version, [`CONFIG_VERSION`] when written by this build.
    pub version: u32,
    /// The tweaks that were applied, by id.
    #[serde(default)]
    pub tweaks: Vec<ConfigTweak>,
    /// Settings by key. The importer picks the keys it knows and checks each value; anything else
    /// is ignored.
    #[serde(default)]
    pub settings: BTreeMap<String, serde_json::Value>,
}

impl ConfigFile {
    /// A file of the current version.
    #[must_use]
    pub fn new(tweaks: Vec<ConfigTweak>, settings: BTreeMap<String, serde_json::Value>) -> Self {
        Self {
            format: CONFIG_FORMAT.to_owned(),
            version: CONFIG_VERSION,
            tweaks,
            settings,
        }
    }

    /// The file as readable, indented text (JSON).
    pub fn to_text(&self) -> Result<String, ConfigError> {
        serde_json::to_string_pretty(self)
            .map(|mut text| {
                text.push('\n');
                text
            })
            .map_err(|error| ConfigError::Write(error.to_string()))
    }

    /// Reads a file's text. Anything that is not a configuration of a version this build knows is
    /// an error; unknown extra fields are ignored.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge);
        }
        let header: Header =
            serde_json::from_str(text).map_err(|_| ConfigError::NotAConfiguration)?;
        if header.format != CONFIG_FORMAT {
            return Err(ConfigError::NotAConfiguration);
        }
        if header.version > CONFIG_VERSION {
            return Err(ConfigError::NewerVersion(header.version));
        }
        let file: Self = serde_json::from_str(text).map_err(|_| ConfigError::NotAConfiguration)?;
        if file.tweaks.len() > MAX_CONFIG_TWEAKS {
            return Err(ConfigError::TooManyTweaks);
        }
        Ok(file)
    }
}

/// Why a tweak of a file is not applied on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportSkip {
    /// This Volt has no tweak with that id.
    Unknown,
    /// The tweak needs administrator rights and Volt does not run with them.
    NeedsAdministrator,
    /// The tweak cannot be applied here: this machine lacks something it changes, or its state
    /// cannot be read. Holds the reason.
    Unavailable(String),
}

/// What an import would do with one tweak of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ImportAction {
    /// Apply it: the changes it would make now.
    Apply(Vec<Preview>),
    /// Already in place on this machine; nothing to do.
    AlreadyApplied,
    /// Leave it out.
    Skip(ImportSkip),
}

/// One tweak of a file and what an import would do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TweakImport {
    /// The id as written in the file (cut to a sensible length for an id Volt does not know).
    pub id: String,
    /// What would happen.
    pub action: ImportAction,
}

impl TweakEngine {
    /// The tweaks worth carrying to another machine: those Volt applied that are in place now.
    /// A tweak that was changed since is left out (it is not in effect), and so is one that is on
    /// because Windows or the user set it (Volt did not apply it). Read-only.
    #[must_use]
    pub fn export_tweaks(&self) -> Vec<ConfigTweak> {
        self.catalogue()
            .tweaks()
            .iter()
            .filter(|tweak| {
                self.applied_by_volt(&tweak.id).unwrap_or(false)
                    && self.state(&tweak.id) == Ok(TweakState::Applied)
            })
            .map(|tweak| ConfigTweak {
                id: tweak.id.clone(),
                title: Some(tweak.title.clone()),
            })
            .collect()
    }

    /// What importing these tweaks would do on this machine, in file order, each id once.
    /// Read-only: nothing is applied, and the file decides nothing but which ids to consider.
    #[must_use]
    pub fn plan_import(&self, tweaks: &[ConfigTweak]) -> Vec<TweakImport> {
        let mut seen = HashSet::new();
        tweaks
            .iter()
            .filter(|entry| seen.insert(entry.id.as_str()))
            .map(|entry| {
                let action = self.plan_import_of(&entry.id);
                // An id Volt does not know is text from the file: cut to a sensible length.
                let id = if action == ImportAction::Skip(ImportSkip::Unknown) {
                    entry.id.chars().take(MAX_SHOWN_ID_CHARS).collect()
                } else {
                    entry.id.clone()
                };
                TweakImport { id, action }
            })
            .collect()
    }

    fn plan_import_of(&self, id: &str) -> ImportAction {
        let Some(tweak) = self.catalogue().get(id) else {
            return ImportAction::Skip(ImportSkip::Unknown);
        };
        if tweak.requires_admin && !self.has_administrator() {
            return ImportAction::Skip(ImportSkip::NeedsAdministrator);
        }
        match self.plan(id) {
            Ok(plan) if plan.is_empty() => ImportAction::AlreadyApplied,
            Ok(plan) => {
                ImportAction::Apply(plan.items.into_iter().map(|item| item.preview).collect())
            }
            Err(error) => ImportAction::Skip(ImportSkip::Unavailable(error.to_string())),
        }
    }
}
