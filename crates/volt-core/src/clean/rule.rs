//! The cleaning rule model: a rule is data that says exactly where it looks,
//! what it may take and what it must leave alone.

use std::fmt;

use serde::Deserialize;

use super::folders::Location;
use super::glob::Pattern;
use crate::label::Risk;

/// Deepest folder level a rule may descend to, whatever it declares.
pub const MAX_DEPTH_LIMIT: u32 = 32;

/// Smallest minimum age, in days, a settings value may ask for. Zero would let a
/// clean take files a program wrote a moment ago.
pub const MIN_AGE_SETTING_MIN: u32 = 1;

/// Largest minimum age, in days, a settings value may ask for.
pub const MIN_AGE_SETTING_MAX: u32 = 3650;

/// The group a rule belongs to in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CleanCategory {
    /// Temporary files programs left behind.
    TemporaryFiles,
    /// Caches of applications, rebuilt on demand.
    ApplicationCache,
    /// Caches of web browsers.
    BrowserCache,
    /// Log files.
    Logs,
    /// Crash dumps and error reports.
    CrashReports,
    /// Leftovers of Windows Update.
    WindowsUpdate,
}

impl fmt::Display for CleanCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TemporaryFiles => "Temporary files",
            Self::ApplicationCache => "Application caches",
            Self::BrowserCache => "Browser caches",
            Self::Logs => "Logs",
            Self::CrashReports => "Crash reports",
            Self::WindowsUpdate => "Windows Update leftovers",
        })
    }
}

/// Where a rule starts: a named location and a relative path below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRoot {
    pub(super) location: Location,
    pub(super) relative: Vec<String>,
}

impl RuleRoot {
    /// The named location.
    #[must_use]
    pub fn location(&self) -> Location {
        self.location
    }

    /// The path below the location, one entry per component. Empty only for a
    /// location that allows it ([`Location::allows_empty_path`]).
    #[must_use]
    pub fn relative(&self) -> &[String] {
        &self.relative
    }
}

impl fmt::Display for RuleRoot {
    /// `location/relative/path`, naming no real folder, so it is safe to show
    /// and to log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.location)?;
        for part in &self.relative {
            write!(f, "/{part}")?;
        }
        Ok(())
    }
}

/// Limits a rule puts on itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Constraints {
    /// A file or folder is only taken when it was last modified at least this
    /// many days ago. `0` means no minimum.
    pub min_age_days: u32,
    /// How many folder levels below the root are searched: `0` looks only at
    /// the entries directly inside the root. At most [`MAX_DEPTH_LIMIT`].
    pub max_depth: u32,
    /// Whether folders left empty (after the rule's files are taken) are
    /// removed too.
    pub remove_empty_dirs: bool,
}

/// One cleaning rule. Built only by [`CleaningRules::from_toml`], so every rule
/// in existence passed validation.
///
/// [`CleaningRules::from_toml`]: super::CleaningRules::from_toml
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleaningRule {
    /// Unique id: lower-case letters and digits separated by single hyphens.
    pub id: String,
    /// Short name for the UI.
    pub title: String,
    /// What the rule removes and why that is safe.
    pub description: String,
    /// The group of the rule.
    pub category: CleanCategory,
    /// How dangerous it is to clean what the rule finds.
    pub risk: Risk,
    /// Where it looks.
    pub root: RuleRoot,
    /// A file is a candidate only when it matches at least one of these.
    pub include: Vec<Pattern>,
    /// Entries matching one of these are left alone; a matching folder is not
    /// entered.
    pub exclude: Vec<Pattern>,
    /// Age, depth and empty-folder limits.
    pub constraints: Constraints,
    /// Whether this location can only be cleaned with administrator rights.
    /// The rule is still scanned (it may turn out to be readable); a scan that
    /// is refused access says so in a typed way (see
    /// [`RuleScan::needs_administrator`](super::RuleScan::needs_administrator)).
    pub requires_admin: bool,
    /// The settings key that may override `constraints.min_age_days`, for rules
    /// whose age the user can configure (temp-like rules). `None`: the age is
    /// fixed by the rule.
    pub min_age_setting: Option<String>,
    /// Executable names (lower case, such as `chrome.exe`) of programs that keep
    /// the files of this rule open while they run. A scan says which of them are
    /// running, because a clean then skips the files they hold. Empty: none.
    pub locked_by: Vec<String>,
}

impl CleaningRule {
    /// Whether the entry at `relative` (path components below the root) or one
    /// of its parent folders matches an exclude pattern.
    #[must_use]
    pub fn excludes<S: AsRef<str>>(&self, relative: &[S]) -> bool {
        (1..=relative.len()).any(|end| {
            self.exclude
                .iter()
                .any(|pattern| pattern.matches(&relative[..end]))
        })
    }

    /// Whether a file at `relative` matches an include pattern.
    #[must_use]
    pub fn includes<S: AsRef<str>>(&self, relative: &[S]) -> bool {
        self.include.iter().any(|pattern| pattern.matches(relative))
    }
}
