//! Machine-wide cleaning through the elevated helper: what a request names, what a scan or a clean
//! reports, and which rules the helper serves.
//!
//! The helper serves the rules of its OWN catalogue that need administrator rights
//! ([`machine_rule`]). A request names a rule id and, at most, a minimum age that can only raise the
//! catalogue's ([`effective_min_age`]); it carries no path, so there is nothing to talk the helper
//! into deleting. A scan reports counts, sizes and the reasons things were left out, never a file
//! name: a person previews numbers, and what a later clean deletes is exactly what that scan found
//! (and that is still unchanged and old enough).
//!
//! Deletion is permanent. The wording of every surface says so, and the activity record of a run
//! is not undoable.

use serde::{Deserialize, Serialize};

use super::loader::CleaningRules;
use super::rule::{CleaningRule, ItemUnit, MIN_AGE_SETTING_MAX};

/// How many entries of a rule one machine scan may look at.
pub const MACHINE_MAX_ENTRIES: usize = 200_000;

/// How long one machine scan or clean may run before the helper stops it, in seconds.
pub const MACHINE_DEADLINE_SECS: u64 = 100;

/// The rule with this id when the helper serves it: it needs administrator rights, works on files
/// (not whole entries) and is not picked item by item.
#[must_use]
pub fn machine_rule<'a>(rules: &'a CleaningRules, id: &str) -> Option<&'a CleaningRule> {
    rules
        .get(id)
        .filter(|rule| rule.requires_admin && rule.unit == ItemUnit::Files && !rule.pick)
}

/// The minimum age, in days, a machine scan applies: the catalogue's own is a floor, and a request
/// may only raise it. `None` for a value that is not a plausible age.
#[must_use]
pub fn effective_min_age(rule: &CleaningRule, requested: Option<u32>) -> Option<u32> {
    let floor = rule.constraints.min_age_days;
    match requested {
        None => Some(floor),
        Some(days) if days > MIN_AGE_SETTING_MAX => None,
        Some(days) => Some(days.max(floor)),
    }
}

/// Whether a clean of this rule must be confirmed by a person on the helper's own surface: the
/// catalogue says so, or the scan found that a non-administrator can write to the root.
#[must_use]
pub fn needs_confirmation(rule: &CleaningRule, root_writable_by_users: bool) -> bool {
    rule.confirm_each_time || root_writable_by_users
}

/// The paths Windows has queued to rename or delete at the next restart, from the entries of the
/// multi-string value `PendingFileRenameOperations` (source and destination alternate; a
/// destination may carry a leading `!`). Lower case, in the plain `C:\...` form, so they compare
/// with a path built from a scan. An item on the list is left for Windows.
#[must_use]
pub fn pending_paths(entries: &[String]) -> Vec<String> {
    entries
        .iter()
        .filter_map(|entry| {
            let text = entry.trim().trim_start_matches('!');
            let text = text.strip_prefix(r"\??\").unwrap_or(text);
            let text = text.trim_end_matches('\\');
            (!text.is_empty()).then(|| text.to_lowercase())
        })
        .collect()
}

/// What a scan left out, by reason. Exact counts; no name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSkips {
    /// Modified more recently than the minimum age.
    pub too_new: u64,
    /// No modification time, so the age cannot be checked.
    pub age_unknown: u64,
    /// A link, junction, mount point or other reparse point: never followed.
    pub links: u64,
    /// A folder that a non-administrator owns or can write to, with everything in it.
    pub user_writable: u64,
    /// Windows refused to read it.
    pub access_denied: u64,
    /// It could not be read for another reason, or changed while it was read.
    pub unreadable: u64,
    /// A name that is not a plain file name.
    pub invalid_name: u64,
    /// Folders deeper than the rule's depth.
    pub too_deep: u64,
    /// Waiting for a restart: Windows has a rename or delete of it queued
    /// (`PendingFileRenameOperations`), so it is left for Windows.
    #[serde(default)]
    pub pending_restart: u64,
}

impl ScanSkips {
    /// Everything left out.
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.too_new,
            self.age_unknown,
            self.links,
            self.user_writable,
            self.access_denied,
            self.unreadable,
            self.invalid_name,
            self.too_deep,
            self.pending_restart,
        ]
        .iter()
        .fold(0, |sum, count| sum.saturating_add(*count))
    }
}

/// What a clean left in place, by reason. Exact counts; no name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplySkips {
    /// It was gone already.
    pub gone: u64,
    /// It is not what the scan saw (another size, time or kind of entry, or another file under
    /// the same name).
    pub changed: u64,
    /// It, or a folder above it, is a link or other reparse point now.
    pub links: u64,
    /// The file has more than one name (a hard link), so deleting it could reach something that
    /// lives outside the rule's folder.
    pub hard_links: u64,
    /// A folder planned for removal has content again.
    pub not_empty: u64,
    /// In use by a program, or Windows refused the delete.
    pub locked: u64,
    /// A folder above it is now owned or writable by a non-administrator.
    pub user_writable: u64,
    /// Windows queued a rename or delete of it for the next restart since the scan.
    #[serde(default)]
    pub pending_restart: u64,
    /// It could not be deleted for another reason.
    pub failed: u64,
}

impl ApplySkips {
    /// Everything left in place.
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.gone,
            self.changed,
            self.links,
            self.hard_links,
            self.not_empty,
            self.locked,
            self.user_writable,
            self.pending_restart,
            self.failed,
        ]
        .iter()
        .fold(0, |sum, count| sum.saturating_add(*count))
    }
}

/// Why a rule could not be scanned at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MachineProblem {
    /// The folder does not exist: nothing to clean.
    RootMissing,
    /// The folder, or a folder on the way to it, is a link or is not a folder, so it is not used.
    RootRefused,
    /// Windows refused access to the folder.
    AccessDenied,
    /// The folder could not be read for another reason.
    Unreadable,
    /// The minimum age asked for is not a plausible number of days.
    BadAge,
    /// The scan did not finish in time (the helper stopped it).
    TimedOut,
}

/// Answer to `clean_scan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanScanReply {
    /// Names this scan when it is applied: 128 random bits as 32 hex digits, valid for one
    /// `clean_apply` on this connection. Empty when `problem` is set.
    pub scan: String,
    /// The rule's id.
    pub rule: String,
    /// Files a clean would delete.
    pub files: u64,
    /// Empty folders a clean would delete.
    pub folders: u64,
    /// Size of those files, in bytes.
    pub bytes: u64,
    /// The minimum age, in days, this scan applied (the catalogue's, or a higher one asked for).
    pub min_age_days: u32,
    /// Whether the scan stopped at a limit (items, entries or time): there is more than this.
    pub truncated: bool,
    /// Whether the clean will be confirmed by a person on the helper's own surface.
    pub confirm: bool,
    /// What was left out.
    pub skipped: ScanSkips,
    /// Why nothing was scanned, when that is so.
    pub problem: Option<MachineProblem>,
}

/// Answer to `clean_apply`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanApplyReply {
    /// The rule's id.
    pub rule: String,
    /// Files deleted.
    pub files: u64,
    /// Empty folders deleted.
    pub folders: u64,
    /// Size of the files deleted, in bytes.
    pub bytes: u64,
    /// What was left in place.
    pub skipped: ApplySkips,
    /// Whether the helper stopped before it had looked at every item (the time limit).
    pub stopped: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &str = r#"
[[rule]]
id = "machine-thing"
title = "Machine thing"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]
requires_admin = true
permanent = true

[rule.root]
location = "windows"
path = "Temp"

[rule.constraints]
min_age_days = 7
max_depth = 3

[[rule]]
id = "user-thing"
title = "User thing"
description = "d"
category = "temporary_files"
risk = "safe"
include = ["**"]

[rule.root]
location = "local_app_data"
path = "X"

[rule.constraints]
min_age_days = 1
max_depth = 3
"#;

    fn rules() -> CleaningRules {
        CleaningRules::from_toml(RULES).expect("rules")
    }

    #[test]
    fn only_rules_that_need_administrator_rights_are_served() {
        let rules = rules();
        assert!(machine_rule(&rules, "machine-thing").is_some());
        assert!(machine_rule(&rules, "user-thing").is_none());
        assert!(machine_rule(&rules, "nothing").is_none());
    }

    #[test]
    fn a_request_can_raise_the_minimum_age_and_never_lower_it() {
        let rules = rules();
        let rule = machine_rule(&rules, "machine-thing").expect("rule");
        assert_eq!(effective_min_age(rule, None), Some(7));
        assert_eq!(effective_min_age(rule, Some(0)), Some(7));
        assert_eq!(effective_min_age(rule, Some(3)), Some(7));
        assert_eq!(effective_min_age(rule, Some(30)), Some(30));
        assert_eq!(
            effective_min_age(rule, Some(MIN_AGE_SETTING_MAX)),
            Some(3650)
        );
        assert_eq!(effective_min_age(rule, Some(MIN_AGE_SETTING_MAX + 1)), None);
    }

    #[test]
    fn permanent_and_confirm_each_time_need_administrator_rights() {
        let text = RULES.replace(
            "requires_admin = true\npermanent = true",
            "permanent = true",
        );
        let error = CleaningRules::from_toml(&text).expect_err("refused");
        assert!(error.to_string().contains("permanent"), "{error}");
    }

    #[test]
    fn a_writable_root_forces_the_confirmation() {
        let rules = rules();
        let rule = machine_rule(&rules, "machine-thing").expect("rule");
        assert!(!needs_confirmation(rule, false));
        assert!(needs_confirmation(rule, true));
    }

    #[test]
    fn the_pending_list_is_read_as_plain_lower_case_paths() {
        let entries = [
            r"\??\C:\Windows\Temp\Old.tmp".to_owned(),
            String::new(),
            r"\??\C:\Windows\Temp\A.tmp".to_owned(),
            r"!\??\C:\Windows\Temp\B.tmp\".to_owned(),
        ];
        assert_eq!(
            pending_paths(&entries),
            [
                r"c:\windows\temp\old.tmp",
                r"c:\windows\temp\a.tmp",
                r"c:\windows\temp\b.tmp"
            ]
        );
    }

    #[test]
    fn counts_add_up() {
        let skips = ScanSkips {
            too_new: 2,
            links: 1,
            ..ScanSkips::default()
        };
        assert_eq!(skips.total(), 3);
        let skips = ApplySkips {
            locked: 4,
            hard_links: 1,
            ..ApplySkips::default()
        };
        assert_eq!(skips.total(), 5);
    }
}
