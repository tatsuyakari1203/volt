//! Cleaning: moving the items of a confirmed plan to the Recycle Bin, with
//! every item re-checked immediately before it is touched.

use std::collections::HashSet;
use std::fmt;
use std::path::{Component, Path, PathBuf};

use super::CleanEngine;
use super::plan::{CleanItem, ConfirmedPlan, ItemKind, RuleScan};
use super::root::same_path_ci;
use super::rule::{CleaningRule, ItemUnit, MAX_DEPTH_LIMIT};
use super::scan::{Tally, component_text};
use crate::activity::{ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::phrase::{Phrase, class_phrase};
use crate::ports::{EntryKind, FileSystem};

/// Why an item of a confirmed plan was not moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CleanSkipReason {
    /// The engine has no rule with the plan's rule id.
    RuleUnknown,
    /// The rule's root is no longer usable (the text is the reason class) or
    /// does not resolve to the folder the plan was made for.
    RootChanged,
    /// The path is not inside the rule's root.
    OutsideRoot,
    /// The rule does not allow this path (it is excluded, does not match, or
    /// is deeper than the rule's limit).
    NotAllowedByRule,
    /// It no longer exists.
    Gone,
    /// It changed since the scan: another size, another modification time, or
    /// another kind of entry.
    Changed,
    /// It, or a folder above it, is a link or other reparse point now.
    Link,
    /// A folder planned for removal has content again.
    NotEmpty,
    /// Every copy of a duplicated file was chosen for removal; one must stay.
    NoCopyKept,
    /// The copy that was to stay is gone or no longer matches, so the others are left.
    KeptCopyLost,
    /// It is locked, in use by another program, or Windows denied access.
    LockedOrDenied,
    /// The file system adapter refused the request (path outside the root or
    /// behind a link).
    Refused,
    /// It could not be read or moved for another reason; the text is the error
    /// class.
    Failed(&'static str),
}

impl CleanSkipReason {
    /// The reason as a phrase, for the UI to write in its own language.
    #[must_use]
    pub fn phrase(&self) -> Phrase {
        match self {
            Self::RuleUnknown => Phrase::new("skip.ruleUnknown"),
            Self::RootChanged => Phrase::new("skip.rootChanged"),
            Self::OutsideRoot => Phrase::new("skip.outsideRoot"),
            Self::NotAllowedByRule => Phrase::new("skip.notAllowed"),
            Self::Gone => Phrase::new("skip.gone"),
            Self::Changed => Phrase::new("skip.changed"),
            Self::Link => Phrase::new("skip.link"),
            Self::NotEmpty => Phrase::new("skip.notEmpty"),
            Self::NoCopyKept => Phrase::new("skip.noCopyKept"),
            Self::KeptCopyLost => Phrase::new("skip.keptCopyLost"),
            Self::LockedOrDenied => Phrase::new("skip.locked"),
            Self::Refused => Phrase::new("skip.refused"),
            Self::Failed(class) => Phrase::new("skip.failed").phrase("class", class_phrase(class)),
        }
    }
}

impl fmt::Display for CleanSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.phrase().english())
    }
}

/// An item that was not moved, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanSkip {
    /// The item's path.
    pub path: PathBuf,
    /// Why it was left where it is.
    pub reason: CleanSkipReason,
}

/// What cleaning one rule did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleClean {
    /// The rule's id.
    pub rule_id: String,
    /// Items moved to the Recycle Bin, in the order they were moved.
    pub moved: Vec<CleanItem>,
    /// Items left in place, with the reason.
    pub skipped: Vec<CleanSkip>,
}

impl RuleClean {
    /// Number of files moved to the Recycle Bin, those inside whole folders included.
    #[must_use]
    pub fn moved_files(&self) -> usize {
        self.moved.iter().fold(0_usize, |sum, item| {
            sum.saturating_add(usize::try_from(item.file_count).unwrap_or(usize::MAX))
        })
    }

    /// Number of empty folders moved to the Recycle Bin.
    #[must_use]
    pub fn moved_directories(&self) -> usize {
        self.moved
            .iter()
            .filter(|item| item.kind == ItemKind::EmptyDirectory)
            .count()
    }

    /// Size of the files moved to the Recycle Bin, in bytes (logical size as
    /// seen by the scan).
    #[must_use]
    pub fn moved_bytes(&self) -> u64 {
        self.moved
            .iter()
            .fold(0, |sum, item| sum.saturating_add(item.size_bytes))
    }
}

/// What a clean did.
///
/// # Space is not freed yet
///
/// Everything Volt removes goes to the Recycle Bin, and the files keep using
/// their space there. Nothing is freed until the Recycle Bin is emptied, which
/// a clean never does: it is a separate, permanent action
/// ([`RecycleBinManager`]) that the user confirms. So the report offers
/// [`moved_bytes`] (what now sits in the Recycle Bin, and what emptying it
/// would free) and has no "freed" figure;
/// UI text must say "moved to the Recycle Bin", not "freed".
///
/// [`moved_bytes`]: CleanReport::moved_bytes
/// [`RecycleBinManager`]: super::RecycleBinManager
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CleanReport {
    /// One entry per confirmed rule, in plan order.
    pub rules: Vec<RuleClean>,
    /// How many activity records could not be written (the clean itself is
    /// never affected by that).
    pub activity_write_failures: usize,
}

impl CleanReport {
    /// Files moved to the Recycle Bin, over all rules.
    #[must_use]
    pub fn moved_files(&self) -> usize {
        self.rules.iter().map(RuleClean::moved_files).sum()
    }

    /// Empty folders moved to the Recycle Bin, over all rules.
    #[must_use]
    pub fn moved_directories(&self) -> usize {
        self.rules.iter().map(RuleClean::moved_directories).sum()
    }

    /// Size of what was moved to the Recycle Bin, in bytes. Not freed space:
    /// see the type documentation.
    #[must_use]
    pub fn moved_bytes(&self) -> u64 {
        self.rules
            .iter()
            .fold(0, |sum, rule| sum.saturating_add(rule.moved_bytes()))
    }

    /// Number of items left in place, over all rules.
    #[must_use]
    pub fn skipped_count(&self) -> usize {
        self.rules.iter().map(|rule| rule.skipped.len()).sum()
    }

    /// A plain sentence for the user that names the figure honestly.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut text = format!(
            "Moved {} to the Recycle Bin ({}).",
            counted(self.moved_files(), self.moved_directories()).english(),
            human_bytes(self.moved_bytes())
        );
        if self.skipped_count() > 0 {
            text = format!(
                "{text} {} left in place.",
                counted_items(self.skipped_count()).english()
            );
        }
        text.push_str(" No disk space is freed until the Recycle Bin is emptied.");
        text
    }
}

fn counted_items(count: usize) -> Phrase {
    Phrase::new("count.item").count("count", count)
}

/// `3 files` or `3 files and 2 empty folders`, as a phrase.
fn counted(files: usize, directories: usize) -> Phrase {
    let files = Phrase::new("count.file").count("count", files);
    if directories == 0 {
        files
    } else {
        Phrase::new("join.and").phrase("first", files).phrase(
            "second",
            Phrase::new("count.emptyFolder").count("count", directories),
        )
    }
}

/// `1.5 MB` style size (powers of 1024, one decimal).
pub(crate) fn human_bytes(bytes: u64) -> String {
    crate::phrase::size_text(bytes)
}

impl CleanEngine {
    /// Moves the items of a confirmed plan to the Recycle Bin.
    ///
    /// The engine takes no rule here, only what the user confirmed, and it
    /// touches nothing that is not listed in the plan. Each item is checked
    /// again immediately before it is moved, and is left in place (and
    /// reported) unless all of this holds:
    ///
    /// - the rule is known and its root still resolves, with the same safety
    ///   checks as a scan, to the folder the plan was made for,
    /// - the path is inside the root, the rule still allows it (include,
    ///   exclude, depth) and it is not the root itself,
    /// - every folder between the root and the item is still a plain folder
    ///   (no link or other reparse point),
    /// - the item still exists, is not a link, and is the same kind of entry
    ///   with the same size and modification time as in the scan; a folder
    ///   must still be empty.
    ///
    /// The move goes through the file system port's Recycle Bin call, which
    /// has its own containment checks. A locked or in-use item is reported and
    /// skipped: there is no retry loop. There is no permanent delete anywhere.
    ///
    /// Cleaning is not an [`Operation`](crate::Operation) and cannot be
    /// reverted through the journal; the Recycle Bin is the undo. When an
    /// activity log is attached, one record per rule is written, naming the
    /// rule id and counts only, never a path or file content. A failing log
    /// never affects the clean.
    #[must_use]
    pub fn clean(&self, plan: &ConfirmedPlan, source: Source) -> CleanReport {
        let rules: Vec<RuleClean> = plan
            .rules()
            .iter()
            .map(|scan| self.clean_rule(scan))
            .collect();
        let mut report = CleanReport {
            rules,
            activity_write_failures: 0,
        };
        report.activity_write_failures = self.record_activity(&report, source);
        log::info!(
            "cleaning finished: {} file(s) and {} empty folder(s) moved to the Recycle Bin, {} item(s) skipped",
            report.moved_files(),
            report.moved_directories(),
            report.skipped_count()
        );
        report
    }

    fn clean_rule(&self, scan: &RuleScan) -> RuleClean {
        let mut result = RuleClean {
            rule_id: scan.rule_id.clone(),
            moved: Vec::new(),
            skipped: Vec::new(),
        };
        let skip_all = |result: &mut RuleClean, reason| {
            result
                .skipped
                .extend(scan.items.iter().map(|item| CleanSkip {
                    path: item.path.clone(),
                    reason,
                }));
        };
        let Some(rule) = self.rules.get(&scan.rule_id) else {
            skip_all(&mut result, CleanSkipReason::RuleUnknown);
            return result;
        };
        // The root is resolved and judged again now; a plan is not trusted to
        // say where its rule's root is.
        let chosen = self.chosen_folder_of(rule).unwrap_or(None);
        let resolved = self.resolve_checked(rule, chosen.as_deref());
        let root = match (resolved, scan.root.clone()) {
            (Ok(resolved), Some(planned)) if same_path_ci(&resolved.path, &planned) => planned,
            _ => {
                skip_all(&mut result, CleanSkipReason::RootChanged);
                return result;
            }
        };
        let root_is_plain_dir = matches!(
            self.file_system.inspect(&root),
            Ok(Some(info)) if info.kind == EntryKind::Directory
        );
        if !root_is_plain_dir {
            skip_all(&mut result, CleanSkipReason::RootChanged);
            return result;
        }

        let mut verified_folders: HashSet<PathBuf> = HashSet::new();
        for item in &scan.items {
            match self.clean_item(rule, &root, item, &mut verified_folders) {
                Ok(()) => result.moved.push(item.clone()),
                Err(reason) => result.skipped.push(CleanSkip {
                    path: item.path.clone(),
                    reason,
                }),
            }
        }
        result
    }

    fn clean_item(
        &self,
        rule: &CleaningRule,
        root: &Path,
        item: &CleanItem,
        verified_folders: &mut HashSet<PathBuf>,
    ) -> Result<(), CleanSkipReason> {
        let fs: &dyn FileSystem = self.file_system.as_ref();
        let relative = relative_components(root, &item.path)?;

        // Would the rule have chosen this path? A path outside that (an
        // edited plan, a changed rule) is not touched.
        let depth_limit = rule.constraints.max_depth.min(MAX_DEPTH_LIMIT) as usize;
        let entries = rule.unit == ItemUnit::Entries;
        let allowed = match item.kind {
            ItemKind::File if entries => relative.len() == 1 && rule.includes(&relative),
            ItemKind::File => relative.len() <= depth_limit + 1 && rule.includes(&relative),
            ItemKind::Folder => entries && relative.len() == 1 && rule.includes(&relative),
            ItemKind::EmptyDirectory => {
                !entries && rule.constraints.remove_empty_dirs && relative.len() <= depth_limit
            }
        } && !rule.excludes(&relative);
        if !allowed {
            return Err(CleanSkipReason::NotAllowedByRule);
        }

        // Every folder on the way down is still a plain folder.
        let mut folder = root.to_path_buf();
        for part in &relative[..relative.len() - 1] {
            folder.push(part);
            if verified_folders.contains(&folder) {
                continue;
            }
            match fs.inspect(&folder) {
                Ok(Some(info)) if info.kind == EntryKind::Directory => {
                    verified_folders.insert(folder.clone());
                }
                Ok(Some(info)) if info.kind == EntryKind::ReparsePoint => {
                    return Err(CleanSkipReason::Link);
                }
                Ok(Some(_)) => return Err(CleanSkipReason::Changed),
                Ok(None) => return Err(CleanSkipReason::Gone),
                Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
            }
        }

        let info = match fs.inspect(&item.path) {
            Ok(Some(info)) => info,
            Ok(None) => return Err(CleanSkipReason::Gone),
            Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
        };
        match (item.kind, info.kind) {
            (_, EntryKind::ReparsePoint) => return Err(CleanSkipReason::Link),
            (ItemKind::File, EntryKind::File) => {
                if info.size_bytes != item.size_bytes || info.modified != item.modified {
                    return Err(CleanSkipReason::Changed);
                }
            }
            (ItemKind::Folder, EntryKind::Directory) => {
                // A whole folder is moved only when it still holds exactly what the scan saw.
                match tally_tree(fs, &item.path) {
                    Ok(now)
                        if now.files == item.file_count
                            && now.bytes == item.size_bytes
                            && now.newest == item.modified => {}
                    Ok(_) => return Err(CleanSkipReason::Changed),
                    Err(PortError::NotFound(_)) => return Err(CleanSkipReason::Gone),
                    Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
                }
            }
            (ItemKind::EmptyDirectory, EntryKind::Directory) => {
                // The folder's own modification time moves when its content is
                // moved away, so emptiness is what counts.
                match fs.read_dir(&item.path) {
                    Ok(entries) if entries.is_empty() => {}
                    Ok(_) => return Err(CleanSkipReason::NotEmpty),
                    Err(error) => return Err(CleanSkipReason::Failed(error_class(&error))),
                }
            }
            _ => return Err(CleanSkipReason::Changed),
        }

        fs.move_to_trash(root, &item.path)
            .map_err(|error| match error {
                PortError::NotFound(_) => CleanSkipReason::Gone,
                PortError::AccessDenied(_) => CleanSkipReason::LockedOrDenied,
                PortError::InvalidInput(_) => CleanSkipReason::Refused,
                other => CleanSkipReason::Failed(error_class(&other)),
            })
    }

    /// Writes one activity record per rule that had items. Returns how many
    /// writes failed.
    fn record_activity(&self, report: &CleanReport, source: Source) -> usize {
        let Some(log) = &self.activity else {
            return 0;
        };
        let mut failures = 0;
        for rule in &report.rules {
            let planned = rule.moved.len() + rule.skipped.len();
            if planned == 0 {
                continue;
            }
            let phrase = Phrase::new("summary.cleanRule")
                .text("rule", rule.rule_id.clone())
                .phrase(
                    "moved",
                    counted(rule.moved_files(), rule.moved_directories()),
                )
                .size("size", rule.moved_bytes());
            let phrase = phrase.maybe(
                "left",
                (!rule.skipped.is_empty()).then(|| {
                    Phrase::new("summary.leftItems")
                        .phrase("items", counted_items(rule.skipped.len()))
                }),
            );
            let outcome = if rule.moved.is_empty() {
                ActivityOutcome::Failed
            } else {
                ActivityOutcome::Applied
            };
            let activity = NewActivity::said(
                self.clock.now(),
                source,
                outcome,
                phrase,
                Some(rule.rule_id.clone()),
            );
            if let Err(error) = log.record(activity) {
                failures += 1;
                log::warn!(
                    "could not write the activity record for cleaning rule {} ({})",
                    rule.rule_id,
                    error_class(&error)
                );
            }
        }
        failures
    }
}

/// What the folder holds now, read-only: how many files, their size and the newest file time.
/// Links are not followed.
fn tally_tree(fs: &dyn FileSystem, folder: &Path) -> Result<Tally, PortError> {
    let mut tally = Tally::default();
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs.read_dir(&dir)? {
            let info = entry.info?;
            match info.kind {
                EntryKind::Directory => stack.push(dir.join(&entry.name)),
                EntryKind::File => tally.add_file(&info),
                _ => {}
            }
        }
    }
    Ok(tally)
}

/// The path below `root` as plain components, or why it is not one.
fn relative_components(root: &Path, path: &Path) -> Result<Vec<String>, CleanSkipReason> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| CleanSkipReason::OutsideRoot)?;
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => {
                parts.push(component_text(name).ok_or(CleanSkipReason::OutsideRoot)?);
            }
            _ => return Err(CleanSkipReason::OutsideRoot),
        }
    }
    if parts.is_empty() {
        return Err(CleanSkipReason::OutsideRoot);
    }
    Ok(parts)
}
