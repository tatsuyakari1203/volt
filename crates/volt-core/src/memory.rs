//! Memory cleaning: run the chosen purges, report memory by list before and after, and write one
//! activity record that says the run cannot be undone.
//!
//! A purge only releases pages Windows can reload or recompute, so there is nothing to restore
//! and nothing goes through the undo journal. Each area is a separate operation: the caller picks
//! which ones run, and one failing (a missing privilege, an unsupported build) does not stop the
//! others, because each failure is a value in the [`MemoryCleanReport`].

use std::sync::Arc;

use crate::activity::{ActivityLog, ActivityOutcome, NewActivity, error_class};
use crate::error::PortError;
use crate::journal::Source;
use crate::ports::{Availability, Memory, MemoryArea, MemoryLists, PurgeOutcome};
use crate::time::Clock;

/// The activity label of a memory cleaning run.
pub const MEMORY_LABEL: &str = "memory.clean";

/// How one selected area ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AreaReport {
    /// The area.
    pub area: MemoryArea,
    /// What the purge did, or why it did not run.
    pub result: Result<PurgeOutcome, PortError>,
}

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryCleanReport {
    /// Memory by list before the first purge; `None` when it could not be read (or nothing ran).
    pub before: Option<MemoryLists>,
    /// Memory by list after the last purge; `None` when it could not be read (or nothing ran).
    pub after: Option<MemoryLists>,
    /// One entry per selected area, in the order they ran ([`MemoryArea::RUN_ORDER`]).
    pub areas: Vec<AreaReport>,
    /// Whether the activity record could not be written. The purges are not affected.
    pub activity_write_failed: bool,
}

impl MemoryCleanReport {
    /// Whether at least one area was purged.
    #[must_use]
    pub fn any_purged(&self) -> bool {
        self.areas.iter().any(|area| area.result.is_ok())
    }
}

/// Runs memory cleaning over the [`Memory`] port.
pub struct MemoryCleaner {
    memory: Arc<dyn Memory>,
    clock: Arc<dyn Clock>,
    activity: Option<Arc<dyn ActivityLog>>,
}

impl std::fmt::Debug for MemoryCleaner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCleaner").finish_non_exhaustive()
    }
}

impl MemoryCleaner {
    /// A cleaner over `memory` that stamps records with `clock`.
    #[must_use]
    pub fn new(memory: Arc<dyn Memory>, clock: Arc<dyn Clock>) -> Self {
        Self {
            memory,
            clock,
            activity: None,
        }
    }

    /// Writes one activity record per run to `log`.
    #[must_use]
    pub fn with_activity_log(mut self, log: Arc<dyn ActivityLog>) -> Self {
        self.activity = Some(log);
        self
    }

    /// Physical memory by list now. Reads only.
    pub fn lists(&self) -> Result<MemoryLists, PortError> {
        self.memory.lists()
    }

    /// Whether each area can run now, in [`MemoryArea::RUN_ORDER`]. Reads only.
    #[must_use]
    pub fn availability(&self) -> Vec<(MemoryArea, Availability)> {
        MemoryArea::RUN_ORDER
            .iter()
            .map(|&area| (area, self.memory.availability(area)))
            .collect()
    }

    /// Runs the selected areas (each at most once, in [`MemoryArea::RUN_ORDER`]), reads memory by
    /// list before and after, and records the run. An empty selection does nothing and writes no
    /// record.
    #[must_use]
    pub fn run(&self, areas: &[MemoryArea], source: Source) -> MemoryCleanReport {
        let selected: Vec<MemoryArea> = MemoryArea::RUN_ORDER
            .into_iter()
            .filter(|area| areas.contains(area))
            .collect();
        if selected.is_empty() {
            return MemoryCleanReport {
                before: None,
                after: None,
                areas: Vec::new(),
                activity_write_failed: false,
            };
        }
        let before = self.read_lists("before");
        let areas: Vec<AreaReport> = selected
            .into_iter()
            .map(|area| AreaReport {
                area,
                result: self.memory.purge(area),
            })
            .collect();
        let after = self.read_lists("after");
        let mut report = MemoryCleanReport {
            before,
            after,
            areas,
            activity_write_failed: false,
        };
        report.activity_write_failed = !self.record(&report, source);
        report
    }

    fn read_lists(&self, when: &str) -> Option<MemoryLists> {
        match self.memory.lists() {
            Ok(lists) => Some(lists),
            Err(error) => {
                log::warn!(
                    "memory by list could not be read {when} cleaning ({})",
                    error_class(&error)
                );
                None
            }
        }
    }

    /// Writes the activity record. Returns whether it was written (or there is no log).
    fn record(&self, report: &MemoryCleanReport, source: Source) -> bool {
        let Some(log) = &self.activity else {
            return true;
        };
        let activity = NewActivity {
            timestamp: self.clock.now(),
            source,
            outcome: if report.any_purged() {
                ActivityOutcome::Applied
            } else {
                ActivityOutcome::Failed
            },
            summary: summary(report),
            batch: None,
            entry: None,
            label: Some(MEMORY_LABEL.to_owned()),
        };
        match log.record(activity) {
            Ok(_) => true,
            Err(error) => {
                log::warn!(
                    "could not write the activity record for memory cleaning ({})",
                    error_class(&error)
                );
                false
            }
        }
    }
}

/// One line for the activity log: what ran, what failed (the error class only, never a port
/// message) and the change in free memory. It says the run cannot be undone.
fn summary(report: &MemoryCleanReport) -> String {
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for entry in &report.areas {
        match &entry.result {
            Ok(outcome) if entry.area == MemoryArea::WorkingSets => done.push(format!(
                "{} ({} processes)",
                entry.area.name(),
                outcome.processes_trimmed
            )),
            Ok(_) => done.push(entry.area.name().to_owned()),
            Err(error) => failed.push(format!("{} ({})", entry.area.name(), error_class(error))),
        }
    }
    let head = "Cleaned memory (cannot be undone)";
    // "Cleaned memory (cannot be undone): a, b; could not clean c; free memory ..."
    let mut parts = vec![if done.is_empty() {
        head.to_owned()
    } else {
        format!("{head}: {}", done.join(", "))
    }];
    if !failed.is_empty() {
        parts.push(format!("could not clean {}", failed.join(", ")));
    }
    if let (Some(before), Some(after)) = (report.before, report.after) {
        parts.push(format!(
            "free memory {} to {}, standby {} to {}",
            human_bytes(before.free_bytes),
            human_bytes(after.free_bytes),
            human_bytes(before.standby_bytes),
            human_bytes(after.standby_bytes),
        ));
    }
    parts.join("; ")
}

/// `1.5 GB` style size (powers of 1024, one decimal).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)] // display only
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fakes::{FakeActivityLog, FakeMemory, ManualClock};

    const GIB: u64 = 1 << 30;

    fn setup() -> (Arc<FakeMemory>, Arc<FakeActivityLog>, MemoryCleaner) {
        let memory = Arc::new(FakeMemory::new());
        let log = Arc::new(FakeActivityLog::new());
        let cleaner = MemoryCleaner::new(memory.clone(), Arc::new(ManualClock::new(5)))
            .with_activity_log(log.clone());
        (memory, log, cleaner)
    }

    #[test]
    fn only_the_selected_areas_run_in_run_order_and_each_once() {
        let (memory, _, cleaner) = setup();
        let report = cleaner.run(
            &[
                MemoryArea::StandbyList,
                MemoryArea::WorkingSets,
                MemoryArea::StandbyList,
            ],
            Source::Manual,
        );
        assert_eq!(
            memory.purged(),
            [MemoryArea::WorkingSets, MemoryArea::StandbyList]
        );
        assert_eq!(report.areas.len(), 2);
        assert!(report.any_purged());
    }

    #[test]
    fn the_report_has_memory_by_list_before_and_after() {
        let (_, _, cleaner) = setup();
        let report = cleaner.run(&[MemoryArea::StandbyList], Source::Manual);
        let (before, after) = (report.before.expect("before"), report.after.expect("after"));
        assert_eq!(before.standby_bytes, 4 * GIB);
        assert_eq!(after.standby_bytes, 0);
        assert_eq!(after.free_bytes, before.free_bytes + 4 * GIB);
        assert_eq!(before.total_bytes, after.total_bytes);
    }

    #[test]
    fn a_failing_area_is_a_typed_value_and_the_others_still_run() {
        let (memory, _, cleaner) = setup();
        memory.make_unavailable(MemoryArea::ModifiedList, Availability::NeedsAdministrator);
        memory.make_unavailable(MemoryArea::FileCache, Availability::Unsupported);
        let report = cleaner.run(&MemoryArea::RUN_ORDER, Source::Manual);
        let result = |area| {
            report
                .areas
                .iter()
                .find(|entry| entry.area == area)
                .map(|entry| entry.result.clone())
        };
        assert!(matches!(
            result(MemoryArea::ModifiedList),
            Some(Err(PortError::AccessDenied(_)))
        ));
        assert!(matches!(
            result(MemoryArea::FileCache),
            Some(Err(PortError::Unsupported(_)))
        ));
        assert!(matches!(result(MemoryArea::StandbyList), Some(Ok(_))));
        assert_eq!(
            memory.purged(),
            [
                MemoryArea::WorkingSets,
                MemoryArea::StandbyListLowPriority,
                MemoryArea::StandbyList
            ]
        );
    }

    #[test]
    fn the_activity_record_says_it_cannot_be_undone_and_has_no_journal_link() {
        let (memory, log, cleaner) = setup();
        memory.make_unavailable(MemoryArea::ModifiedList, Availability::NeedsAdministrator);
        memory.fail_purge(
            MemoryArea::FileCache,
            PortError::Other("secret detail".into()),
        );
        let report = cleaner.run(
            &[
                MemoryArea::WorkingSets,
                MemoryArea::ModifiedList,
                MemoryArea::FileCache,
            ],
            Source::Manual,
        );
        assert!(!report.activity_write_failed);
        let records = log.records();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.outcome, ActivityOutcome::Applied);
        assert_eq!(record.label.as_deref(), Some(MEMORY_LABEL));
        assert_eq!((record.batch, record.entry), (None, None));
        assert!(
            record
                .summary
                .starts_with("Cleaned memory (cannot be undone)")
        );
        assert!(record.summary.contains("working sets (2 processes)"));
        assert!(record.summary.contains("modified list (access denied)"));
        assert!(record.summary.contains("system file cache (error)"));
        assert!(!record.summary.contains("secret"));
        assert!(record.summary.contains("standby 4.0 GB to 5.0 GB"));
    }

    #[test]
    fn a_run_where_nothing_worked_is_recorded_as_failed() {
        let (memory, log, cleaner) = setup();
        memory.make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
        let report = cleaner.run(&[MemoryArea::StandbyList], Source::Manual);
        assert!(!report.any_purged());
        assert_eq!(log.records()[0].outcome, ActivityOutcome::Failed);
    }

    #[test]
    fn an_empty_selection_changes_nothing_and_writes_no_record() {
        let (memory, log, cleaner) = setup();
        let report = cleaner.run(&[], Source::Manual);
        assert!(report.areas.is_empty() && report.before.is_none());
        assert!(memory.purged().is_empty() && log.records().is_empty());
    }

    #[test]
    fn unreadable_lists_do_not_stop_the_purge_and_a_failing_log_is_reported() {
        let (memory, log, cleaner) = setup();
        memory.fail_lists(PortError::Other("nope".into()));
        log.fail_records_with(Some(PortError::Other("disk".into())));
        let report = cleaner.run(&[MemoryArea::StandbyList], Source::Manual);
        assert!(report.before.is_none() && report.after.is_none());
        assert!(report.any_purged());
        assert!(report.activity_write_failed);
    }

    #[test]
    fn availability_lists_every_area_in_run_order() {
        let (memory, _, cleaner) = setup();
        memory.make_unavailable(MemoryArea::StandbyList, Availability::NeedsAdministrator);
        let listed = cleaner.availability();
        assert_eq!(listed.len(), 5);
        assert_eq!(
            listed[0],
            (MemoryArea::WorkingSets, Availability::Available)
        );
        assert_eq!(
            listed[4],
            (MemoryArea::StandbyList, Availability::NeedsAdministrator)
        );
    }

    #[test]
    fn in_use_is_what_the_three_lists_leave() {
        let lists = MemoryLists::from_lists(16 * GIB, 4 * GIB, GIB, 3 * GIB);
        assert_eq!(lists.in_use_bytes, 8 * GIB);
        // A source that reports more than the total never underflows.
        assert_eq!(MemoryLists::from_lists(10, 8, 8, 8).in_use_bytes, 0);
    }
}
