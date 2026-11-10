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
use volt_core::clean::ScanCounts;
use volt_core::{
    CleanCategory, CleanPlan, CleanReport, CleaningRule, RecycleBinManager, RuleScan, ScanError,
    ScanFailure, ScanObserver, Source,
};

use crate::IpcError;
use crate::api::{RiskLevel, risk_view};
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
    /// Why the rule could not be scanned at all, or `null`. The counts are then zero.
    pub problem: Option<String>,
    /// A few of the files, relative to the rule's location (for example `user_temp/sub/a.tmp`).
    /// Never a full path.
    pub samples: Vec<String>,
    /// Set when a program that keeps this rule's files open is running (for example "chrome is
    /// running, so the files it has open will be skipped"), or `null`.
    pub locked_note: Option<String>,
}

/// The result of a scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct CleanScanResult {
    /// Names this scan when cleaning it; empty when the scan was cancelled.
    pub plan_id: String,
    /// Whether the scan was cancelled. Nothing was found and the earlier scan, if any, still
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

    /// Asks the running scan to stop before its next rule. Does nothing when none runs.
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
fn problem_text(failure: &ScanFailure) -> String {
    match failure {
        ScanFailure::Root(_) => "Volt could not use this location, so it was not scanned.",
        ScanFailure::RootIsLink => "This location is a link, so it was not scanned.",
        ScanFailure::RootNotDirectory => "This location is not a folder, so it was not scanned.",
        ScanFailure::RootAccessDenied => "Windows denied access to this location.",
        ScanFailure::RootUnreadable(class) => {
            return format!("This location could not be read ({class}).");
        }
        ScanFailure::MinAgeSettingUnreadable(_) | ScanFailure::MinAgeSettingInvalid => {
            "The minimum age setting could not be used, so this rule was not scanned."
        }
        // `ScanFailure` is non-exhaustive: anything new degrades to a generic line.
        _ => "This rule could not be scanned.",
    }
    .to_owned()
}

/// What a scan left out, in a few words, or `None` when it left out nothing worth saying.
fn skipped_text(counts: &ScanCounts) -> Option<String> {
    let parts: Vec<String> = [
        (counts.too_new, "too new"),
        (counts.age_unknown, "with an unknown age"),
        (counts.access_denied, "denied by Windows"),
        (counts.unreadable, "unreadable"),
        (counts.links, "links left alone"),
        (counts.beyond_max_depth, "deep folders not entered"),
        (counts.invalid_name, "with unusual names"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, what)| format!("{count} {what}"))
    .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
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
fn locked_text(running: &[String]) -> Option<String> {
    if running.is_empty() {
        return None;
    }
    let names: Vec<&str> = running
        .iter()
        .map(|name| name.strip_suffix(".exe").unwrap_or(name))
        .collect();
    let verb = if names.len() == 1 { "is" } else { "are" };
    Some(format!(
        "{} {verb} running: files it keeps open are skipped. Close it first to clean everything.",
        names.join(", ")
    ))
}

fn scan_view(rule: &CleaningRule, scan: &RuleScan) -> RuleScanView {
    RuleScanView {
        rule: CleaningRuleInfo::from(rule),
        files: count32(scan.file_count()),
        folders: count32(scan.directory_count()),
        bytes: count64(scan.total_bytes()),
        truncated: scan.truncated,
        skipped: skipped_text(&scan.counts),
        problem: scan.failure.as_ref().map(problem_text),
        samples: samples(rule, scan),
        locked_note: locked_text(&scan.running_programs),
    }
}

/// Scans every rule (read-only), keeps the plan under a new id and reports what was found.
/// `on_progress` is called before each rule. A scan that is cancelled between rules keeps the
/// earlier plan and returns `cancelled`.
///
/// # Errors
/// `invalidInput` when a scan already runs; `unsupported` without a clean engine.
pub fn scan_cleaning(
    core: &AppCore,
    on_progress: &dyn Fn(CleanScanProgress),
) -> Result<CleanScanResult, IpcError> {
    let engine = clean_engine(core)?;
    let session = core.clean_session();
    let _running = session.begin_scan()?;
    let ids: Vec<String> = engine
        .rules()
        .rules()
        .iter()
        .map(|rule| rule.id.clone())
        .collect();
    let observer = Observer {
        session,
        on_progress,
    };
    let plan = match engine.scan_observed(&ids, &observer) {
        Ok(plan) => plan,
        Err(ScanError::Cancelled) => {
            return Ok(CleanScanResult {
                plan_id: String::new(),
                cancelled: true,
                rules: Vec::new(),
            });
        }
        Err(error) => {
            return Err(IpcError::Other {
                message: error.to_string(),
            });
        }
    };
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
        cancelled: false,
        rules,
    })
}

/// Asks the running scan to stop before its next rule.
pub fn cancel_cleaning_scan(core: &AppCore) {
    core.clean_session().cancel_scan();
}

fn run_view(core: &AppCore, report: &CleanReport) -> CleanRunResult {
    if report.activity_write_failures > 0 {
        core.note_history_incomplete();
    }
    let engine = core.clean_engine();
    let mut groups: Vec<SkipGroup> = Vec::new();
    for skip in report.rules.iter().flat_map(|rule| &rule.skipped) {
        let reason = skip.reason.to_string();
        match groups.iter_mut().find(|group| group.reason == reason) {
            Some(group) => group.count += 1,
            None => groups.push(SkipGroup { reason, count: 1 }),
        }
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
