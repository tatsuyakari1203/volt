//! The read-only scan: walks one rule's root and decides what a clean would
//! take.

use std::path::Path;

use super::plan::{
    CleanItem, ItemKind, MAX_SKIPPED_DETAILS, RuleScan, ScanCounts, ScanFailure, SkipReason,
    SkippedEntry,
};
use super::root::RootError;
use super::rule::{
    CleaningRule, ItemUnit, MAX_DEPTH_LIMIT, MIN_AGE_SETTING_MAX, MIN_AGE_SETTING_MIN,
};
use super::{CleanEngine, ScanObserver};
use crate::activity::error_class;
use crate::error::PortError;
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

/// How a port failure while reading is reported: access denied is its own
/// typed reason, everything else carries its class.
fn read_failure(error: &PortError) -> SkipReason {
    match error {
        PortError::AccessDenied(_) => SkipReason::AccessDenied,
        other => SkipReason::Unreadable(error_class(other)),
    }
}

/// A root that could not be examined or listed, as a typed failure.
fn root_failure(error: &PortError) -> ScanFailure {
    match error {
        PortError::AccessDenied(_) => ScanFailure::RootAccessDenied,
        other => ScanFailure::RootUnreadable(error_class(other)),
    }
}

impl CleanEngine {
    /// The minimum age to apply to `rule`: its own, or the whole number of days
    /// stored under the rule's settings key. A missing value keeps the rule's
    /// own age; a value that cannot be read or is out of range is a failure,
    /// never a quiet fallback to a smaller age.
    fn effective_min_age(&self, rule: &CleaningRule) -> Result<u32, ScanFailure> {
        let own = rule.constraints.min_age_days;
        let (Some(key), Some(settings)) = (&rule.min_age_setting, &self.settings) else {
            return Ok(own);
        };
        match settings.get(key) {
            Ok(None) => Ok(own),
            Ok(Some(value)) => value
                .as_u64()
                .and_then(|days| u32::try_from(days).ok())
                .filter(|days| (MIN_AGE_SETTING_MIN..=MIN_AGE_SETTING_MAX).contains(days))
                .ok_or(ScanFailure::MinAgeSettingInvalid),
            Err(error) => Err(ScanFailure::MinAgeSettingUnreadable(error_class(&error))),
        }
    }

    /// The programs of the rule's `locked_by` list that are running now. A
    /// process list that cannot be read says nothing: it is not a claim that
    /// no program runs, and the clean skips locked files either way.
    fn running_programs(&self, rule: &CleaningRule) -> Vec<String> {
        let Some(processes) = self
            .processes
            .as_ref()
            .filter(|_| !rule.locked_by.is_empty())
        else {
            return Vec::new();
        };
        let Ok(running) = processes.list() else {
            return Vec::new();
        };
        rule.locked_by
            .iter()
            .filter(|wanted| {
                running
                    .iter()
                    .any(|process| process.name.to_lowercase() == **wanted)
            })
            .cloned()
            .collect()
    }

    pub(super) fn scan_rule(
        &self,
        rule: &CleaningRule,
        now: Timestamp,
        observer: Option<&dyn ScanObserver>,
    ) -> RuleScan {
        let mut scan = RuleScan {
            rule_id: rule.id.clone(),
            title: rule.title.clone(),
            category: rule.category,
            risk: rule.risk,
            requires_admin: rule.requires_admin,
            min_age_days: rule.constraints.min_age_days,
            root: None,
            root_exists: false,
            failure: None,
            truncated: false,
            items: Vec::new(),
            skipped: Vec::new(),
            counts: ScanCounts::default(),
            running_programs: self.running_programs(rule),
            pick: rule.pick,
            followed_by: None,
        };
        match self.effective_min_age(rule) {
            Ok(days) => scan.min_age_days = days,
            Err(failure) => {
                scan.failure = Some(failure);
                return scan;
            }
        }
        let chosen = match self.chosen_folder_of(rule) {
            Ok(chosen) => chosen,
            Err(class) => {
                scan.failure = Some(ScanFailure::Root(RootError::Unavailable(class)));
                return scan;
            }
        };
        let root = match self.resolve_checked(rule, chosen.as_deref()) {
            Ok(resolved) => {
                scan.followed_by = resolved.followed;
                resolved.path
            }
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
                scan.failure = Some(root_failure(&error));
                return scan;
            }
        }

        let mut walker = Walker {
            rule,
            engine: self,
            now,
            min_age_days: scan.min_age_days,
            max_depth: rule.constraints.max_depth.min(MAX_DEPTH_LIMIT),
            observer,
            scan: &mut scan,
        };
        if rule.unit == ItemUnit::Entries {
            walker.entries(&root);
        } else {
            let mut relative = Vec::new();
            walker.walk(&root, &mut relative);
        }
        scan
    }
}

/// What a folder holds: the number of files, their total size and the newest modification time.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct Tally {
    pub(super) files: u64,
    pub(super) bytes: u64,
    pub(super) newest: Option<Timestamp>,
}

impl Tally {
    pub(super) fn add_file(&mut self, info: &EntryInfo) {
        self.files += 1;
        self.bytes = self.bytes.saturating_add(info.size_bytes);
        self.newest = match (self.newest, info.modified) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (a, b) => a.or(b),
        };
    }
}

struct Walker<'a> {
    rule: &'a CleaningRule,
    engine: &'a CleanEngine,
    now: Timestamp,
    min_age_days: u32,
    max_depth: u32,
    observer: Option<&'a dyn ScanObserver>,
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
            SkipReason::AccessDenied => counts.access_denied += 1,
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

    /// Whether the plan of this rule is full. Reaching the cap marks the scan as
    /// truncated, which also stops the walk.
    fn at_cap(&mut self) -> bool {
        if self.scan.items.len() >= self.engine.item_cap {
            self.scan.truncated = true;
        }
        self.scan.truncated
    }

    fn age_ok(&self, modified: Option<Timestamp>) -> Result<(), SkipReason> {
        age_allows(self.min_age_days, modified, self.now)
    }

    /// Scans the directory `dir`, whose path below the root is `relative`.
    /// Returns whether everything in it is planned for removal (so it will be
    /// empty after a clean), which is what makes a folder removable.
    fn walk(&mut self, dir: &Path, relative: &mut Vec<String>) -> bool {
        let depth = u32::try_from(relative.len()).unwrap_or(u32::MAX);
        let entries = match self.engine.file_system.read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                if relative.is_empty() {
                    // The root itself cannot be listed: the whole rule is
                    // held back, which is a failure, not one skipped entry.
                    self.scan.failure = Some(root_failure(&error));
                } else {
                    self.skip(dir, read_failure(&error));
                }
                return false;
            }
        };
        let mut all_planned = true;
        for entry in entries {
            if !self.scan.truncated && self.observer.is_some_and(ScanObserver::is_cancelled) {
                // Cancelled in the middle of the walk: what is listed so far
                // stays, and the rule says it is partial.
                self.scan.truncated = true;
            }
            if self.scan.truncated {
                // The item cap was reached or the scan was cancelled: nothing
                // more is looked at, and a folder that was not fully looked at
                // is never planned.
                return false;
            }
            let path = dir.join(&entry.name);
            let Some(name) = component_text(&entry.name) else {
                self.skip(&path, SkipReason::InvalidName);
                all_planned = false;
                continue;
            };
            let info = match entry.info {
                Ok(info) => info,
                Err(error) => {
                    self.skip(&path, read_failure(&error));
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

    /// Scans a rule whose items are the entries directly inside the root: a file is an item, and
    /// so is a folder, whole, whatever it holds.
    fn entries(&mut self, root: &Path) {
        let entries = match self.engine.file_system.read_dir(root) {
            Ok(entries) => entries,
            Err(error) => {
                self.scan.failure = Some(root_failure(&error));
                return;
            }
        };
        for entry in entries {
            if self.cancelled() {
                return;
            }
            let path = root.join(&entry.name);
            let Some(name) = component_text(&entry.name) else {
                self.skip(&path, SkipReason::InvalidName);
                continue;
            };
            let info = match entry.info {
                Ok(info) => info,
                Err(error) => {
                    self.skip(&path, read_failure(&error));
                    continue;
                }
            };
            let relative = [name];
            if self.rule.excludes(&relative) {
                self.scan.counts.excluded += 1;
                continue;
            }
            match info.kind {
                EntryKind::File => {
                    self.file(&path, info, &relative);
                }
                EntryKind::Directory => {
                    if !self.rule.includes(&relative) {
                        self.scan.counts.not_matching += 1;
                        continue;
                    }
                    self.whole_folder(&path, &info);
                }
                _ => self.skip(&path, SkipReason::Link),
            }
        }
    }

    /// Plans the folder at `path` as one item, with the size and newest file time found by
    /// walking it (read-only). Links inside are never followed and go with the folder; the
    /// folder is left out when something in it cannot be read or is too new.
    fn whole_folder(&mut self, path: &Path, info: &EntryInfo) {
        let Some(tally) = self.tally(path) else {
            return;
        };
        if let Err(reason) = self.age_ok(tally.newest.or(info.modified)) {
            self.skip(path, reason);
            return;
        }
        if self.at_cap() {
            return;
        }
        self.scan.items.push(CleanItem {
            path: path.to_path_buf(),
            kind: ItemKind::Folder,
            size_bytes: tally.bytes,
            file_count: tally.files,
            modified: tally.newest,
        });
    }

    /// What a folder holds, or `None` when it cannot be planned: something in it could not be
    /// read (it is then reported as skipped) or the scan was cancelled.
    fn tally(&mut self, folder: &Path) -> Option<Tally> {
        let mut tally = Tally::default();
        let mut stack = vec![folder.to_path_buf()];
        while let Some(dir) = stack.pop() {
            if self.cancelled() {
                return None;
            }
            let entries = match self.engine.file_system.read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) => {
                    self.skip(&dir, read_failure(&error));
                    return None;
                }
            };
            self.scan.counts.folders_entered += 1;
            for entry in entries {
                let path = dir.join(&entry.name);
                let info = match entry.info {
                    Ok(info) => info,
                    Err(error) => {
                        self.skip(&path, read_failure(&error));
                        return None;
                    }
                };
                match info.kind {
                    EntryKind::Directory => stack.push(path),
                    EntryKind::File => {
                        self.scan.counts.files_seen += 1;
                        tally.add_file(&info);
                    }
                    // A link is not followed and its target is not counted.
                    _ => self.scan.counts.links += 1,
                }
            }
        }
        Some(tally)
    }

    /// Marks the scan as stopped when it was cancelled; returns whether it is stopped.
    fn cancelled(&mut self) -> bool {
        if !self.scan.truncated && self.observer.is_some_and(ScanObserver::is_cancelled) {
            self.scan.truncated = true;
        }
        self.scan.truncated
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
        if self.at_cap() {
            return false;
        }
        self.scan.items.push(CleanItem {
            path: path.to_path_buf(),
            kind: ItemKind::File,
            size_bytes: info.size_bytes,
            file_count: 1,
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
        if self.at_cap() {
            return false;
        }
        self.scan.items.push(CleanItem {
            path: path.to_path_buf(),
            kind: ItemKind::EmptyDirectory,
            size_bytes: 0,
            file_count: 0,
            modified: info.modified,
        });
        true
    }
}
