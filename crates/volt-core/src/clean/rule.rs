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
    /// Shader caches of Windows and graphics drivers, rebuilt as programs need them.
    ShaderCache,
    /// Caches of chat applications (never messages, files or sign-in state).
    ChatAppCache,
    /// Package manager and build tool caches (npm, pnpm, pip, cargo, `NuGet`, Gradle, Yarn).
    DeveloperCache,
    /// Caches of AI models (Hugging Face, torch hub, Ollama). Large, and downloaded again only
    /// at the cost of the whole download, so the user picks them one by one.
    AiModelCache,
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
            Self::ShaderCache => "Shader caches",
            Self::ChatAppCache => "Chat app caches",
            Self::DeveloperCache => "Developer caches",
            Self::AiModelCache => "AI model caches",
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

/// What one item of a rule's plan is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum ItemUnit {
    /// Every matching file is an item (the default). Folders are walked, and removed when empty
    /// if the rule says so.
    #[default]
    Files,
    /// Every entry directly inside the root is an item, and a folder is taken whole, in one move
    /// to the Recycle Bin, whatever it holds. The rule's `include` patterns are matched against
    /// the entry's name only. For stores with hundreds of thousands of small files, and for
    /// stores that are only useful as a whole (a model in a cache).
    Entries,
}

/// A documented environment variable that moves the folder a rule cleans.
///
/// When the variable is set to a plausible folder, the rule's root becomes that folder plus
/// `path` instead of the default location, because that is where the tool keeps its cache now.
/// A value that is not a plausible cache folder (a drive root, the profile, a system folder) is
/// ignored and the default location is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Follow {
    /// The variable's name, such as `CARGO_HOME`.
    pub var: String,
    /// The path below the variable's folder, one entry per component. Empty: the folder itself.
    pub path: Vec<String>,
    /// Where the tool documents the variable (the document and section), shown next to it.
    pub source: String,
}

/// Names no rule that works in a folder the user chose ever offers: the output folders of image
/// generators (`ComfyUI` keeps generated images in `output`).
#[must_use]
pub fn is_output_folder_name(name: &str) -> bool {
    name.eq_ignore_ascii_case("output") || name.eq_ignore_ascii_case("outputs")
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact the catalogue states about the rule, not a state"
)]
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
    /// Whether the elevated helper removes what this rule takes for good, with no Recycle Bin
    /// (a machine-wide rule: the files of other accounts cannot go to one account's bin). Only a
    /// rule that `requires_admin` may say so, and every surface that offers it says "deleted
    /// permanently".
    pub permanent: bool,
    /// Whether the helper asks a person on its own surface before every clean of this rule,
    /// because a normal user can create files in its folder (`C:\Windows\Temp`): what is there
    /// may be planted. The helper also asks, whatever this says, when the root's access list
    /// shows that a non-administrator can write to it. Needs `requires_admin`.
    pub confirm_each_time: bool,
    /// The settings key that may override `constraints.min_age_days`, for rules
    /// whose age the user can configure (temp-like rules). `None`: the age is
    /// fixed by the rule.
    pub min_age_setting: Option<String>,
    /// Executable names (lower case, such as `chrome.exe`) of programs that keep
    /// the files of this rule open while they run. A scan says which of them are
    /// running, because a clean then skips the files they hold. Empty: none.
    pub locked_by: Vec<String>,
    /// What one item is: a file (default) or a whole entry of the root.
    pub unit: ItemUnit,
    /// Whether the user picks the items one by one. A scan then lists every item with its
    /// size, nothing is selected by default and a clean takes only the items the user picked.
    /// Needs `unit = "entries"`.
    pub pick: bool,
    /// Environment variables that move the root, in order of precedence: the first one that is
    /// set to a plausible folder wins.
    pub follow: Vec<Follow>,
    /// For a rule that works in a folder the user chose: names of files or folders that must
    /// exist directly in the chosen folder for it to count as an install of the tool (such as
    /// `main.py`). Checked read-only on every scan and clean. Empty for every other rule.
    pub install_markers: Vec<String>,
}

impl CleaningRule {
    /// Whether the rule works in a folder the user chose.
    #[must_use]
    pub fn needs_chosen_folder(&self) -> bool {
        self.root.location == Location::ChosenFolder
    }

    /// Whether the entry at `relative` (path components below the root) or one
    /// of its parent folders matches an exclude pattern, or is in an output folder of a rule
    /// that works in a folder the user chose.
    #[must_use]
    pub fn excludes<S: AsRef<str>>(&self, relative: &[S]) -> bool {
        // Output folders of image generators are never offered or taken, in any mode, whatever
        // the patterns say: that is where the user's own images are.
        if self.needs_chosen_folder()
            && relative
                .iter()
                .any(|part| is_output_folder_name(part.as_ref()))
        {
            return true;
        }
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
