//! Memory by list over the last minutes, for the chart on the Memory page.
//!
//! The readings ride on the live-metrics sampler instead of a timer of their own: the sampler's
//! metrics source is wrapped in a [`TrendSource`], which reads memory by list right after every
//! metrics read. So the cost is what the sampler already costs, plus one read of memory by list
//! (one `NtQuerySystemInformation` call and one `GlobalMemoryStatusEx`, both read-only and
//! without administrator rights) per second, one small event per second and at most
//! [`MemoryHistory::CAPACITY`] readings in memory (about 20 KB).
//!
//! The sampler exists only while the main window does, builds its source when it starts and
//! drops it when it ends, so nothing is read while the app sits in the tray, and the history is
//! cleared when the source is dropped: a closed window keeps no readings.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use volt_core::{Clock, Memory, MemoryHistory, MemoryPoint, MetricsSource, PortError, RawSample};

use crate::IpcError;
use crate::memory::MemoryListsView;

/// The readings of the current run of the sampler. Lives in Tauri's managed state.
#[derive(Debug, Default)]
pub(crate) struct MemoryTrend {
    history: Mutex<MemoryHistory>,
}

impl MemoryTrend {
    fn history(&self) -> MutexGuard<'_, MemoryHistory> {
        // Readings are added and dropped whole, so a poisoned lock holds nothing half-written.
        self.history.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, point: MemoryPoint) {
        self.history().push(point);
    }

    fn clear(&self) {
        self.history().clear();
    }

    pub(crate) fn points(&self) -> Vec<MemoryPoint> {
        self.history().points()
    }
}

/// One reading of memory by list, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPointView {
    /// When it was taken, ISO 8601 in UTC.
    pub taken_at: String,
    /// Memory by list at that time.
    pub lists: MemoryListsView,
}

impl From<&MemoryPoint> for MemoryPointView {
    fn from(point: &MemoryPoint) -> Self {
        Self {
            taken_at: point.taken_at.to_iso8601(),
            lists: MemoryListsView::from(point.lists),
        }
    }
}

/// Event emitted about once a second while the window is open, with the latest reading of memory
/// by list. Generated as `events.memoryTrendUpdated`. Nothing is emitted while the app sits in
/// the tray.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MemoryTrendUpdated(pub MemoryPointView);

/// Receives each reading as it is taken.
pub(crate) type Announce = Arc<dyn Fn(&MemoryPoint) + Send + Sync>;

/// A metrics source that also records memory by list on every read.
///
/// A failed read of the lists leaves a gap in the chart and never fails the metrics read.
pub(crate) struct TrendSource {
    inner: Box<dyn MetricsSource>,
    memory: Arc<dyn Memory>,
    clock: Arc<dyn Clock>,
    trend: Arc<MemoryTrend>,
    announce: Announce,
}

impl TrendSource {
    /// Wraps `inner`. The history starts empty: a new run of the sampler is a new chart.
    pub(crate) fn new(
        inner: Box<dyn MetricsSource>,
        memory: Arc<dyn Memory>,
        clock: Arc<dyn Clock>,
        trend: Arc<MemoryTrend>,
        announce: Announce,
    ) -> Self {
        trend.clear();
        Self {
            inner,
            memory,
            clock,
            trend,
            announce,
        }
    }
}

impl MetricsSource for TrendSource {
    fn read(&self) -> Result<RawSample, PortError> {
        let raw = self.inner.read();
        if let Ok(lists) = self.memory.lists() {
            let point = MemoryPoint {
                taken_at: self.clock.now(),
                lists,
            };
            self.trend.push(point);
            (self.announce)(&point);
        }
        raw
    }
}

impl Drop for TrendSource {
    fn drop(&mut self) {
        // The sampler ended (the window closed): keep no readings.
        self.trend.clear();
    }
}

/// Tauri command: the readings of the current run, oldest first (empty when the window was just
/// opened). The chart loads these, then follows `memoryTrendUpdated`.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "every command returns Result<_, IpcError> so the UI handles one error shape, and \
              Tauri hands `State` over by value"
)]
pub(crate) fn memory_history(
    trend: State<'_, Arc<MemoryTrend>>,
) -> Result<Vec<MemoryPointView>, IpcError> {
    Ok(trend.points().iter().map(MemoryPointView::from).collect())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use volt_core::Timestamp;
    use volt_core::fakes::FakeMemory;
    use volt_core::metrics::fake::{FakeMetrics, typical_sample};

    use super::*;

    /// A clock that moves one second on every call.
    struct StepClock(AtomicU64);

    impl Clock for StepClock {
        fn now(&self) -> Timestamp {
            let step = self.0.fetch_add(1000, Ordering::SeqCst);
            Timestamp::from_unix_millis(i64::try_from(step).unwrap_or(0))
        }
    }

    struct Fixture {
        memory: Arc<FakeMemory>,
        trend: Arc<MemoryTrend>,
        announced: Arc<AtomicU64>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                memory: Arc::new(FakeMemory::new()),
                trend: Arc::new(MemoryTrend::default()),
                announced: Arc::new(AtomicU64::new(0)),
            }
        }

        fn source(&self, metrics: FakeMetrics) -> TrendSource {
            let announced = Arc::clone(&self.announced);
            TrendSource::new(
                Box::new(metrics),
                Arc::clone(&self.memory) as Arc<dyn Memory>,
                Arc::new(StepClock(AtomicU64::new(1000))),
                Arc::clone(&self.trend),
                Arc::new(move |_| {
                    announced.fetch_add(1, Ordering::SeqCst);
                }),
            )
        }
    }

    #[test]
    fn every_read_records_and_announces_memory_by_list() {
        let fixture = Fixture::new();
        let source = fixture.source(FakeMetrics::constant(typical_sample()));
        source.read().expect("first read");
        source.read().expect("second read");
        let points = fixture.trend.points();
        assert_eq!(points.len(), 2);
        assert!(points[0].taken_at < points[1].taken_at);
        assert_eq!(points[0].lists, fixture.memory.lists().expect("lists"));
        assert_eq!(fixture.announced.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn unreadable_lists_leave_a_gap_and_do_not_fail_the_metrics_read() {
        let fixture = Fixture::new();
        fixture.memory.fail_lists(PortError::Other("nope".into()));
        let source = fixture.source(FakeMetrics::constant(typical_sample()));
        assert!(source.read().is_ok());
        assert!(fixture.trend.points().is_empty());
        assert_eq!(fixture.announced.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_failing_metrics_read_is_passed_on_and_the_history_is_dropped_with_the_source() {
        let fixture = Fixture::new();
        let source = fixture.source(FakeMetrics::scripted(vec![Err(PortError::Other(
            "counters".into(),
        ))]));
        assert!(source.read().is_err());
        assert_eq!(fixture.trend.points().len(), 1);
        drop(source);
        assert!(fixture.trend.points().is_empty());
    }

    #[test]
    fn a_new_source_starts_with_an_empty_history() {
        let fixture = Fixture::new();
        fixture.trend.push(MemoryPoint {
            taken_at: Timestamp::from_unix_millis(1),
            lists: fixture.memory.lists().expect("lists"),
        });
        let _fresh = fixture.source(FakeMetrics::constant(typical_sample()));
        assert!(fixture.trend.points().is_empty());
    }
}
