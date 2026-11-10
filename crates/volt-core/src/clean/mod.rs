//! The junk scan and clean engine.
//!
//! Deleting files is the riskiest thing Volt does, so one engine with strict,
//! reviewable rules does all of it. The sequence is always the same:
//!
//! 1. **Rules are data** ([`CleaningRules::from_toml`]): a named [`Location`]
//!    plus a relative path as the root, include and exclude patterns, a
//!    category, a risk level and constraints (minimum age, maximum depth,
//!    whether empty folders go too).
//! 2. **Scan** ([`CleanEngine::scan`]) is read-only. It resolves each root
//!    through the [`KnownFolders`] port, walks it without ever following a
//!    junction or symbolic link, never leaves the root, and returns a
//!    [`CleanPlan`]: per rule the files with their sizes and times, totals, and
//!    what was skipped and why.
//! 3. **Confirm** ([`CleanPlan::confirm`]): the caller shows the plan and the
//!    user agrees. Only a [`ConfirmedPlan`] can be cleaned.
//! 4. **Clean** ([`CleanEngine::clean`]) takes the confirmed plan, never a
//!    rule. It touches only paths listed in the plan, re-checks each one
//!    immediately before acting and moves it to the Recycle Bin. There is no
//!    permanent delete. Locked or in-use files are skipped and reported, never
//!    retried. The [`CleanReport`] says what was moved and what was skipped;
//!    it does not claim space was freed, because the files are still in the
//!    Recycle Bin.
//!
//! Cleaning is not an [`Operation`](crate::Operation): it cannot be reverted
//! through the journal (the Recycle Bin is the way back), and its ports are not
//! part of the [`Ports`](crate::Ports) bundle. The engine takes its own.

mod apply;
mod bin;
mod folders;
mod glob;
mod loader;
mod plan;
mod root;
mod rule;
mod scan;

#[cfg(any(test, feature = "fakes"))]
pub mod fakes;

use std::fmt;
use std::sync::Arc;

use thiserror::Error;

pub(crate) use apply::human_bytes;
pub use apply::{CleanReport, CleanSkip, CleanSkipReason, RuleClean};
pub use bin::{EmptyReport, RecycleBin, RecycleBinManager, RecycleBinStatus};
pub use folders::{KnownFolders, Location};
pub use glob::Pattern;
pub use loader::{CleaningRules, RuleIssue, RuleIssueKind, RulesError};
pub use plan::{
    CleanItem, CleanPlan, ConfirmedPlan, ItemKind, MAX_ITEMS_PER_RULE, MAX_SKIPPED_DETAILS,
    RuleScan, ScanCounts, ScanFailure, SkipReason, SkippedEntry,
};
pub use root::RootError;
pub(crate) use root::{same_path_ci, starts_with_ci};
pub use rule::{
    CleanCategory, CleaningRule, Constraints, MAX_DEPTH_LIMIT, MIN_AGE_SETTING_MAX,
    MIN_AGE_SETTING_MIN, RuleRoot,
};

use crate::activity::ActivityLog;
use crate::ports::{FileSystem, Processes};
use crate::settings::Settings;
use crate::time::Clock;

/// A scan was asked for a rule that does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown cleaning rule \"{0}\"")]
pub struct UnknownRule(pub String);

/// Why an observed scan produced no plan. A cancelled scan is not an error: it
/// returns the part it did, marked [`CleanPlan::cancelled`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ScanError {
    /// A rule id is not known.
    #[error(transparent)]
    UnknownRule(#[from] UnknownRule),
}

/// Watches a scan and may stop it, for scans long enough to show progress.
pub trait ScanObserver {
    /// Called before each rule is scanned: `done` rules are finished out of
    /// `total`.
    fn progress(&self, done: usize, total: usize);

    /// Asked before each rule and before each entry of the folder being
    /// walked; `true` stops the scan, which then returns the part it did as a
    /// [`CleanPlan`] with `cancelled` set. It must be cheap.
    fn is_cancelled(&self) -> bool;
}

/// Scans and cleans with a set of rules.
///
/// It owns the rules and its own ports; none of the cleaning ports is part of
/// the [`Ports`](crate::Ports) bundle used by operations.
pub struct CleanEngine {
    rules: CleaningRules,
    folders: Arc<dyn KnownFolders>,
    file_system: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
    settings: Option<Arc<dyn Settings>>,
    processes: Option<Arc<dyn Processes>>,
    item_cap: usize,
}

impl fmt::Debug for CleanEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CleanEngine")
            .field("rules", &self.rules.len())
            .field("activity_log", &self.activity.is_some())
            .field("settings", &self.settings.is_some())
            .finish_non_exhaustive()
    }
}

impl CleanEngine {
    /// An engine over `rules`. The clock decides how old a file is and stamps
    /// activity records.
    #[must_use]
    pub fn new(
        rules: CleaningRules,
        folders: Arc<dyn KnownFolders>,
        file_system: Arc<dyn FileSystem>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            rules,
            folders,
            file_system,
            clock,
            activity: None,
            settings: None,
            processes: None,
            item_cap: MAX_ITEMS_PER_RULE,
        }
    }

    /// Lowers the number of items one rule may plan from
    /// [`MAX_ITEMS_PER_RULE`] (never raises it; a cap of 0 plans nothing).
    #[must_use]
    pub fn with_item_cap(mut self, cap: usize) -> Self {
        self.item_cap = cap.min(MAX_ITEMS_PER_RULE);
        self
    }

    /// Lets the user's settings override the minimum age of the rules that
    /// name a settings key (`min_age_setting`). The key holds a whole number
    /// of days from [`MIN_AGE_SETTING_MIN`] to [`MIN_AGE_SETTING_MAX`]. Without
    /// settings, or with no value stored, a rule keeps its own age. A value
    /// that cannot be read or is out of range fails that rule's scan with a
    /// typed [`ScanFailure`]: it is never replaced by a smaller age.
    #[must_use]
    pub fn with_settings(mut self, settings: Arc<dyn Settings>) -> Self {
        self.settings = Some(settings);
        self
    }

    /// Lets a scan report which programs that hold a rule's files open are
    /// running ([`RuleScan::running_programs`]). Without it none is reported.
    #[must_use]
    pub fn with_processes(mut self, processes: Arc<dyn Processes>) -> Self {
        self.processes = Some(processes);
        self
    }

    /// Also writes an activity record for every clean (see
    /// [`CleanEngine::clean`]). Without a log nothing is recorded.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// The rules this engine knows.
    #[must_use]
    pub fn rules(&self) -> &CleaningRules {
        &self.rules
    }

    /// Scans the rules with these ids, in this order. Read-only: nothing is
    /// changed, so this is the dry run. A rule that cannot be scanned (its root
    /// is refused or unreadable) is reported in its [`RuleScan::failure`] and
    /// does not stop the others.
    pub fn scan<S: AsRef<str>>(&self, rule_ids: &[S]) -> Result<CleanPlan, UnknownRule> {
        let rules = rule_ids
            .iter()
            .map(|id| {
                self.rules
                    .get(id.as_ref())
                    .ok_or_else(|| UnknownRule(id.as_ref().to_owned()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let now = self.clock.now();
        Ok(CleanPlan {
            scanned_at: now,
            rules: rules
                .into_iter()
                .map(|rule| self.scan_rule(rule, now, None))
                .collect(),
            cancelled: false,
        })
    }

    /// [`scan`](Self::scan) that reports progress before each rule and can be
    /// cancelled through `observer`, between rules or inside the one being
    /// walked. A cancelled scan returns the rules it started with
    /// [`CleanPlan::cancelled`] set; the rule it stopped in is
    /// [`RuleScan::truncated`].
    pub fn scan_observed<S: AsRef<str>>(
        &self,
        rule_ids: &[S],
        observer: &dyn ScanObserver,
    ) -> Result<CleanPlan, ScanError> {
        let rules = rule_ids
            .iter()
            .map(|id| {
                self.rules
                    .get(id.as_ref())
                    .ok_or_else(|| UnknownRule(id.as_ref().to_owned()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let now = self.clock.now();
        let total = rules.len();
        let mut scanned = Vec::with_capacity(total);
        let mut cancelled = false;
        for (done, rule) in rules.into_iter().enumerate() {
            if observer.is_cancelled() {
                cancelled = true;
                break;
            }
            observer.progress(done, total);
            let scan = self.scan_rule(rule, now, Some(observer));
            // A rule cut short while a cancel is pending was stopped by it.
            cancelled = scan.truncated && observer.is_cancelled();
            scanned.push(scan);
            if cancelled {
                break;
            }
        }
        Ok(CleanPlan {
            scanned_at: now,
            rules: scanned,
            cancelled,
        })
    }

    /// Scans every rule, in file order.
    #[must_use]
    pub fn scan_all(&self) -> CleanPlan {
        let now = self.clock.now();
        CleanPlan {
            scanned_at: now,
            rules: self
                .rules
                .rules()
                .iter()
                .map(|rule| self.scan_rule(rule, now, None))
                .collect(),
            cancelled: false,
        }
    }
}

#[cfg(test)]
mod tests;
