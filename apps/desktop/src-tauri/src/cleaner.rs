//! The cleaner at the IPC boundary: listing the rules, scanning, cleaning a confirmed plan and the
//! Recycle Bin card.
//!
//! The transfer types are defined here, in the shell, like the others (see [`crate::api`]). Three
//! things are decided here and nowhere else:
//!
//! - **The plan stays in Rust.** A scan keeps its plan in [`CleanSession`] under an id and sends
//!   the UI only counts, sizes and rule-relative sample paths (`user_temp/sub/file.tmp`), never a
//!   full path: those contain the profile name. A new scan replaces the stored plan, and a clean
//!   consumes it, so one scan can be cleaned once and only as the user confirmed it.
//! - **Rules that need administrator rights cannot be cleaned.** They are scanned and shown (the
//!   scan is read-only), but the clean command refuses them: the elevation model is undecided.
//! - **Nothing says space was freed.** The clean result carries the engine's own sentence, which
//!   says the files were moved to the Recycle Bin. Only [`empty_recycle_bin`] deletes for good.
//!
//! Byte and item counts cross as `f64` (see [`crate::metrics`]).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri_specta::Event;
use volt_core::Phrase;
use volt_core::clean::ScanCounts;
use volt_core::{
    CleanCategory, CleanPlan, CleanReport, CleaningRule, RecycleBinManager, RuleScan, ScanError,
    ScanFailure, ScanObserver, Source,
};

use crate::IpcError;
use crate::api::{RiskLevel, risk_view};
use crate::phrase::PhraseView;
use crate::wiring::AppCore;

/// Sample paths listed per rule.
const SAMPLES_PER_RULE: usize = 5;

/// A byte or item count as a JavaScript number.
#[allow(
    clippy::cast_precision_loss,
    reason = "exact up to 2^53, which no real disk or file count reaches"
)]
pub(crate) fn count64(value: u64) -> f64 {
    value as f64
}

pub(crate) fn count32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// The group a rule is listed under. Mirrors [`volt_core::CleanCategory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum CleanCategoryView {
    /// Temporary files programs left behind.
    TemporaryFiles,
    /// Caches of applications.
    ApplicationCache,
    /// Caches of web browsers.
    BrowserCache,
    /// Shader caches of Windows and graphics drivers.
    ShaderCache,
    /// Caches of chat applications.
    ChatAppCache,
    /// Package manager and build tool caches.
    DeveloperCache,
    /// Log files.
    Logs,
    /// Crash dumps and error reports.
    CrashReports,
    /// Leftovers of Windows Update.
    WindowsUpdate,
    /// A group this build does not know a name for.
    Other,
}

fn category_view(category: CleanCategory) -> CleanCategoryView {
    match category {
        CleanCategory::TemporaryFiles => CleanCategoryView::TemporaryFiles,
        CleanCategory::ApplicationCache => CleanCategoryView::ApplicationCache,
        CleanCategory::BrowserCache => CleanCategoryView::BrowserCache,
        CleanCategory::ShaderCache => CleanCategoryView::ShaderCache,
        CleanCategory::ChatAppCache => CleanCategoryView::ChatAppCache,
        CleanCategory::DeveloperCache => CleanCategoryView::DeveloperCache,
        CleanCategory::Logs => CleanCategoryView::Logs,
        CleanCategory::CrashReports => CleanCategoryView::CrashReports,
        CleanCategory::WindowsUpdate => CleanCategoryView::WindowsUpdate,
        // `CleanCategory` is non-exhaustive: a group added later degrades to `Other`.
        _ => CleanCategoryView::Other,
    }
}

/// One cleaning rule, as the UI lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleaningRuleInfo {
    /// Stable id, sent back to clean.
    pub id: String,
    /// Short name.
    pub title: String,
    /// What the rule takes and why that is safe.
    pub description: String,
    /// The group it is listed under.
    pub category: CleanCategoryView,
    /// The group's name.
    pub category_label: String,
    /// How risky cleaning what it finds is.
    pub risk: RiskLevel,
    /// Whether this rule needs administrator rights, so it cannot be cleaned yet.
    pub needs_administrator: bool,
    /// Files younger than this many days are never taken (0: no minimum).
    pub min_age_days: u32,
}

impl From<&CleaningRule> for CleaningRuleInfo {
    fn from(rule: &CleaningRule) -> Self {
        Self {
            id: rule.id.clone(),
            title: rule.title.clone(),
            description: rule.description.clone(),
            category: category_view(rule.category),
            category_label: rule.category.to_string(),
            risk: risk_view(rule.risk),
            needs_administrator: rule.requires_admin,
            min_age_days: rule.constraints.min_age_days,
        }
    }
}

/// What a scan found for one rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleScanView {
    /// The rule.
    pub rule: CleaningRuleInfo,
    /// Files a clean would move to the Recycle Bin.
    pub files: u32,
    /// Empty folders a clean would move to the Recycle Bin.
    pub folders: u32,
    /// Size of those files in bytes.
    pub bytes: f64,
    /// Whether the scan stopped at the per-rule limit: there is more than what is listed.
    pub truncated: bool,
    /// What was left out and why, in a few words (for example "12 too new"), or `null`.
    pub skipped: Option<String>,
    /// `skipped` as a code and parameters, for the UI to write in its own language.
    pub skipped_phrase: Option<PhraseView>,
    /// Why the rule could not be scanned at all, or `null`. The counts are then zero.
    pub problem: Option<String>,
    /// `problem` as a code and parameters.
    pub problem_phrase: Option<PhraseView>,
    /// A few of the files, relative to the rule's location (for example `user_temp/sub/a.tmp`).
    /// Never a full path.
    pub samples: Vec<String>,
    /// Set when a program that keeps this rule's files open is running (for example "chrome is
    /// running, so the files it has open will be skipped"), or `null`.
    pub locked_note: Option<String>,
    /// `locked_note` as a code and parameters.
    pub locked_phrase: Option<PhraseView>,
}

/// The result of a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleanScanResult {
    /// Names this scan when cleaning it; empty when the scan was cancelled before it looked at
    /// any rule.
    pub plan_id: String,
    /// Whether the scan was cancelled and is partial: `rules` lists only the rules it got to, the
    /// last of them possibly `truncated`. With no rule at all the earlier scan, if any, still
    /// stands.
    pub cancelled: bool,
    /// One entry per rule, in file order.
    pub rules: Vec<RuleScanView>,
}

/// How far a scan is: rule `done + 1` of `total` is about to be looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct CleanScanProgress {
    /// Rules finished.
    pub done: u32,
    /// Rules in the scan.
    pub total: u32,
}

/// Skipped items with the same reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkipGroup {
    /// Why they were left where they were.
    pub reason: String,
    /// `reason` as a code and parameters, for the UI to write in its own language.
    pub reason_phrase: PhraseView,
    /// How many.
    pub count: u32,
}

/// What cleaning one rule did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RuleCleanView {
    /// The rule's id.
    pub id: String,
    /// The rule's title.
    pub title: String,
    /// Files moved to the Recycle Bin.
    pub moved_files: u32,
    /// Size of those files in bytes.
    pub moved_bytes: f64,
    /// Items left in place.
    pub skipped: u32,
}

/// What a clean did. The files are in the Recycle Bin and still use their space: there is no
/// "freed" figure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleanRunResult {
    /// Files moved to the Recycle Bin.
    pub moved_files: u32,
    /// Empty folders moved to the Recycle Bin.
    pub moved_folders: u32,
    /// Size of the files moved, in bytes.
    pub moved_bytes: f64,
    /// Items left in place, in all.
    pub skipped_total: u32,
    /// Items left in place, by reason.
    pub skipped: Vec<SkipGroup>,
    /// Per rule.
    pub rules: Vec<RuleCleanView>,
    /// The engine's own sentence about what happened.
    pub message: String,
    /// Whether the activity record could not be written (the clean itself is unaffected).
    pub history_incomplete: bool,
}

/// What the Recycle Bin holds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RecycleBinView {
    /// Number of items.
    pub item_count: f64,
    /// Size in bytes.
    pub size_bytes: f64,
}

/// What emptying the Recycle Bin did.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct EmptyBinResult {
    /// Whether the bin was emptied: `false` when it was already empty.
    pub emptied: bool,
    /// Items it held just before.
    pub item_count: f64,
    /// Their size in bytes.
    pub size_bytes: f64,
    /// Whether the activity record could not be written.
    pub history_incomplete: bool,
}

/// A plan the shell keeps between a scan and a clean.
#[derive(Debug)]
struct StoredPlan {
    id: String,
    plan: CleanPlan,
}

/// The scan state of the shell: the plan of the last scan, whether a scan runs and whether it
/// was asked to stop.
#[derive(Debug, Default)]
pub struct CleanSession {
    plan: Mutex<Option<StoredPlan>>,
    scanning: AtomicBool,
    cancel: AtomicBool,
    last_id: AtomicU64,
}

/// Marks a scan as running for as long as it lives.
struct ScanGuard<'a>(&'a CleanSession);

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.scanning.store(false, Ordering::SeqCst);
    }
}

impl CleanSession {
    fn plan(&self) -> MutexGuard<'_, Option<StoredPlan>> {
        // The guard protects a plain value; a poisoned lock only means another call panicked.
        self.plan.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin_scan(&self) -> Result<ScanGuard<'_>, IpcError> {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return Err(IpcError::InvalidInput {
                message: "a scan is already running".to_owned(),
            });
        }
        self.cancel.store(false, Ordering::SeqCst);
        Ok(ScanGuard(self))
    }

    /// Asks the running scan to stop, inside the rule being walked or before the next one. Does
    /// nothing when none runs.
    pub fn cancel_scan(&self) {
        if self.scanning.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn next_id(&self) -> String {
        format!("scan-{}", self.last_id.fetch_add(1, Ordering::SeqCst) + 1)
    }
}

struct Observer<'a> {
    session: &'a CleanSession,
    on_progress: &'a dyn Fn(CleanScanProgress),
}

impl ScanObserver for Observer<'_> {
    fn progress(&self, done: usize, total: usize) {
        (self.on_progress)(CleanScanProgress {
            done: count32(done),
            total: count32(total),
        });
    }

    fn is_cancelled(&self) -> bool {
        self.session.cancel.load(Ordering::SeqCst)
    }
}

fn clean_engine(core: &AppCore) -> Result<&volt_core::CleanEngine, IpcError> {
    core.clean_engine().ok_or_else(|| IpcError::Unsupported {
        message: "the cleaner is not available in this build".to_owned(),
    })
}

fn recycle_bin(core: &AppCore) -> Result<&RecycleBinManager, IpcError> {
    core.recycle_bin().ok_or_else(|| IpcError::Unsupported {
        message: "the Recycle Bin is not available in this build".to_owned(),
    })
}

/// Lists the cleaning rules. Reads nothing from the file system.
pub fn list_cleaning_rules(core: &AppCore) -> Result<Vec<CleaningRuleInfo>, IpcError> {
    Ok(clean_engine(core)?
        .rules()
        .rules()
        .iter()
        .map(CleaningRuleInfo::from)
        .collect())
}

/// Why a rule was not scanned, in words that name no folder.
fn problem_phrase(failure: &ScanFailure) -> Phrase {
    match failure {
        ScanFailure::Root(_) => Phrase::new("scan.problem.root"),
        ScanFailure::RootIsLink => Phrase::new("scan.problem.link"),
        ScanFailure::RootNotDirectory => Phrase::new("scan.problem.notFolder"),
        ScanFailure::RootAccessDenied => Phrase::new("scan.problem.denied"),
        ScanFailure::RootUnreadable(class) => {
            Phrase::new("scan.problem.unreadable").phrase("class", volt_core::class_phrase(class))
        }
        ScanFailure::MinAgeSettingUnreadable(_) | ScanFailure::MinAgeSettingInvalid => {
            Phrase::new("scan.problem.minAge")
        }
        // `ScanFailure` is non-exhaustive: anything new degrades to a generic line.
        _ => Phrase::new("scan.problem.generic"),
    }
}

/// What a scan left out, in a few words, or `None` when it left out nothing worth saying.
fn skipped_phrase(counts: &ScanCounts) -> Option<Phrase> {
    let parts: Vec<Phrase> = [
        (counts.too_new, "scan.tooNew"),
        (counts.age_unknown, "scan.ageUnknown"),
        (counts.access_denied, "scan.accessDenied"),
        (counts.unreadable, "scan.unreadable"),
        (counts.links, "scan.links"),
        (counts.beyond_max_depth, "scan.deep"),
        (counts.invalid_name, "scan.oddNames"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, code)| Phrase::new(code).count("count", count))
    .collect();
    (!parts.is_empty()).then(|| Phrase::new("scan.skipped").list("parts", ", ", parts))
}

/// A few files of the scan as `location/relative/path`: relative to the rule's location, so the
/// profile name never appears.
fn samples(rule: &CleaningRule, scan: &RuleScan) -> Vec<String> {
    let Some(root) = &scan.root else {
        return Vec::new();
    };
    scan.items
        .iter()
        .filter(|item| item.kind == volt_core::ItemKind::File)
        .filter_map(|item| {
            let relative = item.path.strip_prefix(root).ok()?;
            let parts: Vec<String> = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect();
            Some(format!("{}/{}", rule.root, parts.join("/")))
        })
        .take(SAMPLES_PER_RULE)
        .collect()
}

/// Names the running programs that hold a rule's files open, or `None` when none runs.
fn locked_phrase(running: &[String]) -> Option<Phrase> {
    if running.is_empty() {
        return None;
    }
    let names: Vec<&str> = running
        .iter()
        .map(|name| name.strip_suffix(".exe").unwrap_or(name))
        .collect();
    Some(
        Phrase::new("scan.locked")
            .text("names", names.join(", "))
            .count("count", names.len()),
    )
}

fn scan_view(rule: &CleaningRule, scan: &RuleScan) -> RuleScanView {
    let skipped = skipped_phrase(&scan.counts);
    let problem = scan.failure.as_ref().map(problem_phrase);
    let locked = locked_phrase(&scan.running_programs);
    RuleScanView {
        rule: CleaningRuleInfo::from(rule),
        files: count32(scan.file_count()),
        folders: count32(scan.directory_count()),
        bytes: count64(scan.total_bytes()),
        truncated: scan.truncated,
        skipped: skipped.as_ref().map(Phrase::english),
        skipped_phrase: skipped.as_ref().map(PhraseView::from),
        problem: problem.as_ref().map(Phrase::english),
        problem_phrase: problem.as_ref().map(PhraseView::from),
        samples: samples(rule, scan),
        locked_note: locked.as_ref().map(Phrase::english),
        locked_phrase: locked.as_ref().map(PhraseView::from),
    }
}

/// Scans every rule (read-only), keeps the plan under a new id and reports what was found.
/// `on_progress` is called before each rule. A scan cancelled before its first rule keeps the
/// earlier plan and returns `cancelled` with no rules; one cancelled later (between rules or
/// inside one) returns, and keeps as the new plan, the rules it got to, with `cancelled` set.
///
/// # Errors
/// `invalidInput` when a scan already runs; `unsupported` without a clean engine.
pub fn scan_cleaning(
    core: &AppCore,
    on_progress: &dyn Fn(CleanScanProgress),
) -> Result<CleanScanResult, IpcError> {
    let engine = clean_engine(core)?;
    let ids: Vec<String> = engine
        .rules()
        .rules()
        .iter()
        .map(|rule| rule.id.clone())
        .collect();
    scan_rules(core, &ids, on_progress)
}

/// Like [`scan_cleaning`] for the chosen rules only: how the page continues with the rest of a
/// rule whose scan was truncated. Read-only; the usual confirmation still comes before anything
/// moves.
///
/// # Errors
/// `invalidInput` when a scan already runs, no rule is named or a rule id is not known;
/// `unsupported` without a clean engine.
pub fn scan_cleaning_rules(
    core: &AppCore,
    rule_ids: &[String],
    on_progress: &dyn Fn(CleanScanProgress),
) -> Result<CleanScanResult, IpcError> {
    clean_engine(core)?;
    if rule_ids.is_empty() {
        return Err(IpcError::InvalidInput {
            message: "no rule is chosen".to_owned(),
        });
    }
    scan_rules(core, rule_ids, on_progress)
}

fn scan_rules(
    core: &AppCore,
    ids: &[String],
    on_progress: &dyn Fn(CleanScanProgress),
) -> Result<CleanScanResult, IpcError> {
    let engine = clean_engine(core)?;
    let session = core.clean_session();
    let _running = session.begin_scan()?;
    let observer = Observer {
        session,
        on_progress,
    };
    let plan = match engine.scan_observed(ids, &observer) {
        Ok(plan) => plan,
        Err(ScanError::UnknownRule(unknown)) => {
            return Err(IpcError::InvalidInput {
                message: unknown.to_string(),
            });
        }
        Err(error) => {
            return Err(IpcError::Other {
                message: error.to_string(),
            });
        }
    };
    if plan.cancelled && plan.rules.is_empty() {
        return Ok(CleanScanResult {
            plan_id: String::new(),
            cancelled: true,
            rules: Vec::new(),
        });
    }
    let cancelled = plan.cancelled;
    let rules = plan
        .rules
        .iter()
        .filter_map(|scan| {
            engine
                .rules()
                .get(&scan.rule_id)
                .map(|rule| scan_view(rule, scan))
        })
        .collect();
    let id = session.next_id();
    *session.plan() = Some(StoredPlan {
        id: id.clone(),
        plan,
    });
    Ok(CleanScanResult {
        plan_id: id,
        cancelled,
        rules,
    })
}

/// Asks the running scan to stop: inside the rule it is walking, or before its next rule.
pub fn cancel_cleaning_scan(core: &AppCore) {
    core.clean_session().cancel_scan();
}

/// Counts `reason` in the group of items left in place for the same reason, starting one.
pub(crate) fn add_skip(groups: &mut Vec<SkipGroup>, reason: &volt_core::CleanSkipReason) {
    let phrase = reason.phrase();
    let reason = phrase.english();
    match groups.iter_mut().find(|group| group.reason == reason) {
        Some(group) => group.count += 1,
        None => groups.push(SkipGroup {
            reason,
            reason_phrase: PhraseView::from(&phrase),
            count: 1,
        }),
    }
}

fn run_view(core: &AppCore, report: &CleanReport) -> CleanRunResult {
    if report.activity_write_failures > 0 {
        core.note_history_incomplete();
    }
    let engine = core.clean_engine();
    let mut groups: Vec<SkipGroup> = Vec::new();
    for skip in report.rules.iter().flat_map(|rule| &rule.skipped) {
        add_skip(&mut groups, &skip.reason);
    }
    CleanRunResult {
        moved_files: count32(report.moved_files()),
        moved_folders: count32(report.moved_directories()),
        moved_bytes: count64(report.moved_bytes()),
        skipped_total: count32(report.skipped_count()),
        skipped: groups,
        rules: report
            .rules
            .iter()
            .map(|rule| RuleCleanView {
                id: rule.rule_id.clone(),
                title: engine
                    .and_then(|engine| engine.rules().get(&rule.rule_id))
                    .map_or_else(|| rule.rule_id.clone(), |rule| rule.title.clone()),
                moved_files: count32(rule.moved_files()),
                moved_bytes: count64(rule.moved_bytes()),
                skipped: count32(rule.skipped.len()),
            })
            .collect(),
        message: report.describe(),
        history_incomplete: report.activity_write_failures > 0,
    }
}

/// Cleans the rules the user chose from the scan named `plan_id`: moves their files to the Recycle
/// Bin and reports what was moved and what was left, with reasons. The stored plan is consumed:
/// to clean again, scan again.
///
/// Refused, with nothing touched: a scan is running, the plan is not the latest one, no rule is
/// chosen, a chosen rule is not in the plan, or a chosen rule needs administrator rights.
///
/// # Errors
/// `invalidInput`, `notFound` or `accessDenied` as above; `unsupported` without a clean engine.
pub fn clean_plan(
    core: &AppCore,
    plan_id: &str,
    rule_ids: &[String],
) -> Result<CleanRunResult, IpcError> {
    let engine = clean_engine(core)?;
    let session = core.clean_session();
    if session.scanning.load(Ordering::SeqCst) {
        return Err(IpcError::InvalidInput {
            message: "a scan is running; wait for it to finish or cancel it".to_owned(),
        });
    }
    let confirmed = {
        let mut stored = session.plan();
        let plan = match stored.as_ref() {
            Some(stored) if stored.id == plan_id => &stored.plan,
            _ => {
                return Err(IpcError::NotFound {
                    message: "that scan is out of date; scan again before cleaning".to_owned(),
                });
            }
        };
        if rule_ids.is_empty() {
            return Err(IpcError::InvalidInput {
                message: "no rule is chosen".to_owned(),
            });
        }
        for id in rule_ids {
            let Some(scan) = plan.rules.iter().find(|scan| &scan.rule_id == id) else {
                return Err(IpcError::InvalidInput {
                    message: format!("\"{id}\" is not part of this scan"),
                });
            };
            if scan.requires_admin {
                return Err(IpcError::AccessDenied {
                    message: format!("\"{}\" needs administrator rights.", scan.title),
                });
            }
        }
        let confirmed = plan.confirm_rules(rule_ids);
        *stored = None;
        confirmed
    };
    let report = engine.clean(&confirmed, Source::Manual);
    Ok(run_view(core, &report))
}

/// What the Recycle Bin holds. Read-only.
///
/// # Errors
/// The port's error when the bin cannot be read.
pub fn recycle_bin_status(core: &AppCore) -> Result<RecycleBinView, IpcError> {
    let status = recycle_bin(core)?.status()?;
    Ok(RecycleBinView {
        item_count: count64(status.item_count),
        size_bytes: count64(status.size_bytes),
    })
}

/// Permanently empties the Recycle Bin. The UI asks first; this cannot be undone.
///
/// # Errors
/// The port's error when the bin cannot be read or emptied.
pub fn empty_recycle_bin(core: &AppCore) -> Result<EmptyBinResult, IpcError> {
    let report = recycle_bin(core)?.empty(Source::Manual)?;
    if report.activity_write_failed {
        core.note_history_incomplete();
    }
    Ok(EmptyBinResult {
        emptied: report.emptied,
        item_count: count64(report.before.item_count),
        size_bytes: count64(report.before.size_bytes),
        history_incomplete: report.activity_write_failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_scan_left_out_is_a_phrase_with_counts_and_its_english_is_unchanged() {
        let counts = ScanCounts {
            too_new: 12,
            links: 1,
            ..ScanCounts::default()
        };
        let phrase = skipped_phrase(&counts).expect("something was left out");
        assert_eq!(phrase.english(), "12 too new, 1 links left alone");
        assert_eq!(skipped_phrase(&ScanCounts::default()), None);
    }

    #[test]
    fn a_running_program_is_named_in_a_phrase_that_agrees_with_the_count() {
        let one = locked_phrase(&["chrome.exe".to_owned()]).expect("one");
        assert_eq!(
            one.english(),
            "chrome is running: files it keeps open are skipped. Close it first to clean everything."
        );
        let two = locked_phrase(&["a.exe".to_owned(), "b.exe".to_owned()]).expect("two");
        assert!(two.english().starts_with("a, b are running:"));
        assert_eq!(locked_phrase(&[]), None);
    }
}
