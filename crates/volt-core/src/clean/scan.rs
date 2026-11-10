//! The read-only scan: walks one rule's root and decides what a clean would
//! take.

use std::path::Path;

use super::CleanEngine;
use super::plan::{
    CleanItem, ItemKind, MAX_SKIPPED_DETAILS, RuleScan, ScanCounts, ScanFailure, SkipReason,
    SkippedEntry,
};
use super::root::resolve_root;
use super::rule::{CleaningRule, MAX_DEPTH_LIMIT};
use crate::activity::error_class;
use crate::ports::{EntryInfo, EntryKind};
use crate::time::Timestamp;

const DAY_MS: i64 = 86_400_000;

/// Whether the entry is old enough for the rule, as of `now`.
pub(super) fn age_allows(
    min_age_days: u32,
    modified: Option<Timestamp>,
    now: Timestamp,
) -> Result<(), SkipReason> {
    if min_age_days == 0 {
        return Ok(());
    }
    let Some(modified) = modified else {
        return Err(SkipReason::AgeUnknown);
    };
    let min_age_ms = i64::from(min_age_days).saturating_mul(DAY_MS);
    let age_ms = now
        .as_unix_millis()
        .saturating_sub(modified.as_unix_millis());
    if age_ms >= min_age_ms {
        Ok(())
    } else {
        Err(SkipReason::TooNew)
    }
}

/// A single name component as text, or `None` when it is not one.
pub(super) fn component_text(name: &std::ffi::OsStr) -> Option<String> {
    let text = name.to_string_lossy();
    let invalid = text.is_empty()
        || text == "."
        || text == ".."
        || text.contains(['/', '\\'])
        || text.chars().any(|c| c == '\0');
    (!invalid).then(|| text.into_owned())
}

impl CleanEngine {
    pub(super) fn scan_rule(&self, rule: &CleaningRule, now: Timestamp) -> RuleScan {
        let mut scan = RuleScan {
            rule_id: rule.id.clone(),
            title: rule.title.clone(),
            category: rule.category,
            risk: rule.risk,
            root: None,
            root_exists: false,
            failure: None,
            items: Vec::new(),
            skipped: Vec::new(),
            counts: ScanCounts::default(),
        };
        let root = match resolve_root(rule, self.folders.as_ref()) {
            Ok(root) => root,
            Err(error) => {
                scan.failure = Some(ScanFailure::Root(error));
                return scan;
            }
        };
        scan.root = Some(root.clone());
        match self.file_system.inspect(&root) {
            Ok(None) => return scan,
            Ok(Some(EntryInfo {
                kind: EntryKind::Directory,
                ..
            })) => scan.root_exists = true,
            Ok(Some(EntryInfo {
                kind: EntryKind::ReparsePoint,
                ..
            })) => {
                scan.failure = Some(ScanFailure::RootIsLink);
                return scan;
            }
            Ok(Some(_)) => {
                scan.failure = Some(ScanFailure::RootNotDirectory);
                return scan;
            }
            Err(error) => {
                scan.failure = Some(ScanFailure::RootUnreadable(error_class(&error)));
                return scan;
            }
        }

        let mut walker = Walker {
            rule,
            engine: self,
            now,
            max_depth: rule.constraints.max_depth.min(MAX_DEPTH_LIMIT),
            scan: &mut scan,
        };
        let mut relative = Vec::new();
        walker.walk(&root, &mut relative);
        scan
    }
}

struct Walker<'a> {
    rule: &'a CleaningRule,
    engine: &'a CleanEngine,
    now: Timestamp,
    max_depth: u32,
    scan: &'a mut RuleScan,
}

impl Walker<'_> {
    fn skip(&mut self, path: &Path, reason: SkipReason) {
        let counts = &mut self.scan.counts;
        match reason {
            SkipReason::Link => counts.links += 1,
            SkipReason::TooNew => counts.too_new += 1,
            SkipReason::AgeUnknown => counts.age_unknown += 1,
            SkipReason::BeyondMaxDepth => counts.beyond_max_depth += 1,
            SkipReason::Unreadable(_) => counts.unreadable += 1,
            SkipReason::InvalidName => counts.invalid_name += 1,
        }
        if self.scan.skipped.len() < MAX_SKIPPED_DETAILS {
            self.scan.skipped.push(SkippedEntry {
                path: path.to_path_buf(),
                reason,
            });
        }
    }

    fn age_ok(&self, modified: Option<Timestamp>) -> Result<(), SkipReason> {
        age_allows(self.rule.constraints.min_age_days, modified, self.now)
    }

    /// Scans the directory `dir`, whose path below the root is `relative`.
    /// Returns whether everything in it is planned for removal (so it will be
    /// empty after a clean), which is what makes a folder removable.
    fn walk(&mut self, dir: &Path, relative: &mut Vec<String>) -> bool {
        let depth = u32::try_from(relative.len()).unwrap_or(u32::MAX);
        let entries = match self.engine.file_system.read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.skip(dir, SkipReason::Unreadable(error_class(&error)));
                return false;
            }
        };
        let mut all_planned = true;
        for entry in entries {
            let path = dir.join(&entry.name);
            let Some(name) = component_text(&entry.name) else {
                self.skip(&path, SkipReason::InvalidName);
                all_planned = false;
                continue;
            };
            let info = match entry.info {
                Ok(info) => info,
                Err(error) => {
                    self.skip(&path, SkipReason::Unreadable(error_class(&error)));
                    all_planned = false;
                    continue;
                }
            };
            relative.push(name);
            let planned = match info.kind {
                EntryKind::Directory => self.directory(&path, info, depth, relative),
                EntryKind::File => self.file(&path, info, relative),
                // A reparse point is described as itself and left alone; so is
                // any kind this code does not know.
                _ => {
                    self.skip(&path, SkipReason::Link);
                    false
                }
            };
            relative.pop();
            all_planned &= planned;
        }
        all_planned
    }

    /// Returns whether the file is planned.
    fn file(&mut self, path: &Path, info: EntryInfo, relative: &[String]) -> bool {
        self.scan.counts.files_seen += 1;
        if self.rule.excludes(relative) {
            self.scan.counts.excluded += 1;
            return false;
        }
        if !self.rule.includes(relative) {
            self.scan.counts.not_matching += 1;
            return false;
        }
        if let Err(reason) = self.age_ok(info.modified) {
            self.skip(path, reason);
            return false;
        }
        self.scan.items.push(CleanItem {
            path: path.to_path_buf(),
            kind: ItemKind::File,
            size_bytes: info.size_bytes,
            modified: info.modified,
        });
        true
    }

    /// Returns whether the directory is planned for removal.
    fn directory(
        &mut self,
        path: &Path,
        info: EntryInfo,
        depth: u32,
        relative: &mut Vec<String>,
    ) -> bool {
        if self.rule.excludes(relative) {
            self.scan.counts.excluded += 1;
            return false;
        }
        if depth >= self.max_depth {
            self.skip(path, SkipReason::BeyondMaxDepth);
            return false;
        }
        self.scan.counts.folders_entered += 1;
        let emptied = self.walk(path, relative);
        if !emptied || !self.rule.constraints.remove_empty_dirs {
            return false;
        }
        if let Err(reason) = self.age_ok(info.modified) {
            self.skip(path, reason);
            return false;
        }
        self.scan.items.push(CleanItem {
            path: path.to_path_buf(),
            kind: ItemKind::EmptyDirectory,
            size_bytes: 0,
            modified: info.modified,
        });
        true
    }
}
