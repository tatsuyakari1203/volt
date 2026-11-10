//! The result of a scan: what a clean would take, and what was left alone.

use std::fmt;
use std::path::PathBuf;

use super::root::RootError;
use super::rule::{CleanCategory, MIN_AGE_SETTING_MAX, MIN_AGE_SETTING_MIN};
use crate::label::Risk;
use crate::time::Timestamp;

/// How many skipped entries a [`RuleScan`] lists in detail. The counts in
/// [`ScanCounts`] are always exact; only the list is capped, so a folder with
/// a million too-new files cannot make the plan huge.
pub const MAX_SKIPPED_DETAILS: usize = 500;

/// How many items one rule may put in a plan. A scan that reaches it stops
/// looking at that rule and says so ([`RuleScan::truncated`]), so a folder with
/// millions of files cannot make the plan, and the memory behind it, unbounded.
/// What was found is complete and cleanable; scanning again after a clean
/// finds the rest.
pub const MAX_ITEMS_PER_RULE: usize = 25_000;

/// What a planned item is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ItemKind {
    /// A plain file.
    File,
    /// A folder that is empty, or will be once the plan's files in it are
    /// moved. Only planned when the rule removes empty folders.
    EmptyDirectory,
    /// A whole folder with everything in it, moved in one go (a rule with
    /// [`ItemUnit::Entries`](super::ItemUnit::Entries)). Its size is the sum of the files in it.
    Folder,
}

/// One path a clean would move to the Recycle Bin.
///
/// The size and modification time are what the scan saw; a clean moves the
/// item only when they are still the same.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanItem {
    /// Full path, inside the rule's root.
    pub path: PathBuf,
    /// File or empty folder.
    pub kind: ItemKind,
    /// Size in bytes as seen by the scan (logical size; 0 for an empty folder, the sum of the
    /// files for a whole folder).
    pub size_bytes: u64,
    /// How many files the item holds: 1 for a file, the number of files inside for a whole
    /// folder, 0 for an empty folder.
    pub file_count: u64,
    /// Last modification time as seen by the scan (for a whole folder, the newest file in it).
    pub modified: Option<Timestamp>,
}

/// Why a scan left an entry out of the plan. Entries that simply do not match
/// the rule (and entries the rule excludes) are only counted, not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// A junction, symbolic link or other reparse point. Never followed, never
    /// taken.
    Link,
    /// Modified more recently than the rule's minimum age.
    TooNew,
    /// The file system did not report a modification time, so the minimum age
    /// cannot be checked.
    AgeUnknown,
    /// A folder deeper than the rule's maximum depth; its content was not
    /// looked at.
    BeyondMaxDepth,
    /// Windows refused to read the entry or folder (access denied). For a rule
    /// that [`requires_admin`](RuleScan::requires_admin) this is the typed
    /// sign that administrator rights are needed.
    AccessDenied,
    /// The entry or folder could not be read for another reason; the text is
    /// the error class.
    Unreadable(&'static str),
    /// A name that is not a single path component.
    InvalidName,
}

impl fmt::Display for SkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Link => f.write_str("a link or other reparse point, not followed"),
            Self::TooNew => f.write_str("modified more recently than the rule's minimum age"),
            Self::AgeUnknown => f.write_str("modification time unknown"),
            Self::BeyondMaxDepth => f.write_str("deeper than the rule's maximum depth"),
            Self::AccessDenied => f.write_str("could not be read (access denied)"),
            Self::Unreadable(class) => write!(f, "could not be read ({class})"),
            Self::InvalidName => f.write_str("name is not a single path component"),
        }
    }
}

/// An entry the scan left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    /// The entry.
    pub path: PathBuf,
    /// Why it was left out.
    pub reason: SkipReason,
}

/// Exact counts of what a scan saw, for one rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanCounts {
    /// Files looked at.
    pub files_seen: usize,
    /// Folders entered.
    pub folders_entered: usize,
    /// Entries (files or folders) matching an exclude pattern.
    pub excluded: usize,
    /// Files matching no include pattern.
    pub not_matching: usize,
    /// Links and other reparse points left alone.
    pub links: usize,
    /// Entries newer than the minimum age.
    pub too_new: usize,
    /// Entries whose age could not be determined.
    pub age_unknown: usize,
    /// Folders deeper than the maximum depth.
    pub beyond_max_depth: usize,
    /// Entries or folders Windows refused to read (access denied).
    pub access_denied: usize,
    /// Entries or folders that could not be read for another reason.
    pub unreadable: usize,
    /// Entries with an invalid name.
    pub invalid_name: usize,
}

/// Why a rule could not be scanned at all.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanFailure {
    /// The root was refused (unresolvable, too broad or protected).
    Root(RootError),
    /// The root exists but is a link or other reparse point. It is not
    /// followed.
    RootIsLink,
    /// The root exists but is not a directory.
    RootNotDirectory,
    /// Windows refused access to the root, or to a folder on the way to it.
    /// For a rule that [`requires_admin`](RuleScan::requires_admin) this means
    /// the scan needs administrator rights ([`RuleScan::needs_administrator`]).
    RootAccessDenied,
    /// The root could not be examined for another reason; the text is the
    /// error class.
    RootUnreadable(&'static str),
    /// The settings value that overrides the rule's minimum age could not be
    /// read; the text is the error class. The rule is not scanned, because
    /// guessing an age could take files the user meant to keep.
    MinAgeSettingUnreadable(&'static str),
    /// The settings value that overrides the rule's minimum age is not a whole
    /// number of days in the allowed range. The rule is not scanned.
    MinAgeSettingInvalid,
}

impl fmt::Display for ScanFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root(error) => error.fmt(f),
            Self::RootIsLink => {
                f.write_str("the root is a link or other reparse point, which is not followed")
            }
            Self::RootNotDirectory => f.write_str("the root is not a directory"),
            Self::RootAccessDenied => f.write_str("access to the root was denied"),
            Self::RootUnreadable(class) => write!(f, "the root could not be read ({class})"),
            Self::MinAgeSettingUnreadable(class) => write!(
                f,
                "the minimum age setting could not be read ({class}); the rule was not scanned"
            ),
            Self::MinAgeSettingInvalid => write!(
                f,
                "the minimum age setting is not a whole number of days from \
                 {MIN_AGE_SETTING_MIN} to {MIN_AGE_SETTING_MAX}; the rule was not scanned"
            ),
        }
    }
}

/// The scan of one rule.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is an independent fact about the rule or the scan, not a state"
)]
pub struct RuleScan {
    /// The rule's id.
    pub rule_id: String,
    /// The rule's title.
    pub title: String,
    /// The rule's category.
    pub category: CleanCategory,
    /// The rule's risk level.
    pub risk: Risk,
    /// Whether the rule's location can only be cleaned with administrator
    /// rights (a property of the rule, not something this scan found).
    pub requires_admin: bool,
    /// The minimum age in days this scan applied: the rule's own, or the one
    /// from its settings key. An entry modified more recently is never listed.
    pub min_age_days: u32,
    /// The resolved root, when it could be resolved.
    pub root: Option<PathBuf>,
    /// Whether the root exists. A missing root is not a failure: there is
    /// simply nothing to clean (the program may not be installed).
    pub root_exists: bool,
    /// Set when the rule could not be scanned at all; `items` is then empty.
    pub failure: Option<ScanFailure>,
    /// Whether the scan stopped at the item cap ([`MAX_ITEMS_PER_RULE`]) or
    /// was cancelled while this rule was walked: there is more to clean than
    /// `items` lists, and the counts are partial.
    pub truncated: bool,
    /// What a clean would move to the Recycle Bin. A folder is listed after
    /// everything inside it, so cleaning in this order empties it first.
    pub items: Vec<CleanItem>,
    /// Entries left out for a reason worth showing, at most
    /// [`MAX_SKIPPED_DETAILS`]; [`ScanCounts`] has the exact numbers.
    pub skipped: Vec<SkippedEntry>,
    /// Exact counts.
    pub counts: ScanCounts,
    /// Programs of the rule's [`locked_by`](CleaningRule::locked_by) list that
    /// were running during the scan (lower-case executable names). Files they
    /// hold open are skipped by a clean. Empty when none runs, or when the
    /// engine has no way to list processes.
    pub running_programs: Vec<String>,
    /// Whether the user picks the items one by one (the rule's
    /// [`pick`](super::CleaningRule::pick)): `items` is then the list to pick from.
    pub pick: bool,
    /// The environment variable the root was found through, when the tool's cache was moved
    /// with it (see [`Follow`](super::Follow)).
    pub followed_by: Option<String>,
}

impl RuleScan {
    /// Whether this scan was held back by missing administrator rights: the
    /// rule is flagged [`requires_admin`](Self::requires_admin) and Windows
    /// refused access to its root or to something inside it. The result of
    /// such a scan is incomplete (or empty), not "nothing to clean".
    #[must_use]
    pub fn needs_administrator(&self) -> bool {
        self.requires_admin
            && (self.failure == Some(ScanFailure::RootAccessDenied)
                || self.counts.access_denied > 0)
    }

    /// Number of files that would be moved, those inside whole folders included.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.items.iter().fold(0_usize, |sum, item| {
            sum.saturating_add(usize::try_from(item.file_count).unwrap_or(usize::MAX))
        })
    }

    /// Number of empty folders that would be moved.
    #[must_use]
    pub fn directory_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.kind == ItemKind::EmptyDirectory)
            .count()
    }

    /// Total size of the files that would be moved, in bytes.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.items
            .iter()
            .fold(0, |sum, item| sum.saturating_add(item.size_bytes))
    }
}

/// The outcome of a scan: per rule, what a clean would take. Scanning changes
/// nothing. To clean, the caller confirms the plan ([`CleanPlan::confirm`]).
///
/// Plan types have no way to be built from outside this crate (they are
/// `#[non_exhaustive]` and not deserializable), so a plan always comes from a
/// real scan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanPlan {
    /// When the scan started.
    pub scanned_at: Timestamp,
    /// One entry per scanned rule, in the order requested.
    pub rules: Vec<RuleScan>,
    /// Whether the scan was cancelled before it finished: `rules` then holds
    /// only the rules that were started, the last of them possibly
    /// [`truncated`](RuleScan::truncated) by the cancel.
    pub cancelled: bool,
}

impl CleanPlan {
    /// Number of files that would be moved, over all rules.
    #[must_use]
    pub fn total_files(&self) -> usize {
        self.rules.iter().map(RuleScan::file_count).sum()
    }

    /// Total size of those files in bytes.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.rules
            .iter()
            .fold(0, |sum, rule| sum.saturating_add(rule.total_bytes()))
    }

    /// Whether nothing would be moved.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.iter().all(|rule| rule.items.is_empty())
    }

    /// Confirms the whole plan: the caller has shown it to the user and the
    /// user agreed. A rule the user picks by item ([`RuleScan::pick`]) is left
    /// out: only [`confirm_selection`](Self::confirm_selection) takes picked
    /// items, so nothing of it is ever taken by default.
    #[must_use]
    pub fn confirm(&self) -> ConfirmedPlan {
        ConfirmedPlan {
            scanned_at: self.scanned_at,
            rules: self
                .rules
                .iter()
                .filter(|rule| !rule.pick)
                .cloned()
                .collect(),
        }
    }

    /// Confirms only the rules with these ids (the user unticked the others).
    /// Ids that are not in the plan are ignored, and so is a rule picked by
    /// item (see [`confirm`](Self::confirm)).
    #[must_use]
    pub fn confirm_rules<S: AsRef<str>>(&self, rule_ids: &[S]) -> ConfirmedPlan {
        ConfirmedPlan {
            scanned_at: self.scanned_at,
            rules: self
                .rules
                .iter()
                .filter(|rule| !rule.pick)
                .filter(|rule| rule_ids.iter().any(|id| id.as_ref() == rule.rule_id))
                .cloned()
                .collect(),
        }
    }
}

/// What the user chose of one rule: all of it, or only some of its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSelection {
    /// The rule's id.
    pub rule_id: String,
    /// Positions in [`RuleScan::items`] of the items the user picked. `None`: the whole rule,
    /// which is only allowed for a rule the user does not pick by item.
    pub items: Option<Vec<usize>>,
}

/// Why a selection cannot be confirmed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SelectionError {
    /// The rule is not part of the plan.
    #[error("rule \"{0}\" is not part of this scan")]
    NotInPlan(String),
    /// The rule is picked by item and no item was picked.
    #[error("rule \"{0}\" is picked by item: choose at least one item")]
    NothingPicked(String),
    /// Items were named for a rule that is not picked by item.
    #[error("rule \"{0}\" is not picked by item")]
    NotPickable(String),
    /// An item position is not in the rule's list.
    #[error("rule \"{0}\" has no item {1}")]
    NoSuchItem(String, usize),
}

/// A plan the user confirmed. It is the only thing
/// [`CleanEngine::clean`](super::CleanEngine::clean) accepts: cleaning never
/// takes a rule, and it never touches a path that is not listed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedPlan {
    scanned_at: Timestamp,
    rules: Vec<RuleScan>,
}

impl CleanPlan {
    /// Confirms what the user chose: whole rules, and for a rule picked by item only the picked
    /// items. A rule picked by item with nothing picked, items named for a rule that is not
    /// picked by item, an unknown rule and an item that is not in the list are all refused, so
    /// nothing is ever taken that the user did not pick.
    pub fn confirm_selection(
        &self,
        selections: &[RuleSelection],
    ) -> Result<ConfirmedPlan, SelectionError> {
        let mut rules = Vec::with_capacity(selections.len());
        for selection in selections {
            let Some(scan) = self.rules.iter().find(|r| r.rule_id == selection.rule_id) else {
                return Err(SelectionError::NotInPlan(selection.rule_id.clone()));
            };
            let mut scan = scan.clone();
            match (&selection.items, scan.pick) {
                (None, false) => {}
                (None, true) => return Err(SelectionError::NothingPicked(scan.rule_id)),
                (Some(_), false) => return Err(SelectionError::NotPickable(scan.rule_id)),
                (Some(picked), true) => {
                    if picked.is_empty() {
                        return Err(SelectionError::NothingPicked(scan.rule_id));
                    }
                    if let Some(&bad) = picked.iter().find(|&&index| index >= scan.items.len()) {
                        return Err(SelectionError::NoSuchItem(scan.rule_id, bad));
                    }
                    let mut keep = 0_usize;
                    scan.items = std::mem::take(&mut scan.items)
                        .into_iter()
                        .filter(|_| {
                            let wanted = picked.contains(&keep);
                            keep += 1;
                            wanted
                        })
                        .collect();
                }
            }
            rules.push(scan);
        }
        Ok(ConfirmedPlan {
            scanned_at: self.scanned_at,
            rules,
        })
    }
}

impl ConfirmedPlan {
    /// When the scan behind the plan started.
    #[must_use]
    pub fn scanned_at(&self) -> Timestamp {
        self.scanned_at
    }

    /// The confirmed rules, with their items.
    #[must_use]
    pub fn rules(&self) -> &[RuleScan] {
        &self.rules
    }
}
