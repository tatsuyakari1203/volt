//! The live-metrics sampler: a thread that exists only while the main window exists.
//!
//! Volt keeps almost nothing running while it sits in the tray (see `docs/architecture.md`), so
//! the sampler must not either. [`Sampler::set_window_open`] starts it when the window is
//! created and stops it when the window is destroyed; the thread builds its metrics source when
//! it starts and drops it when it ends, so a closed window holds no performance-counter query.
//!
//! Timing. [`run_loop`] sleeps until a deadline, reads, publishes and repeats. The deadlines are
//! [`next_deadline`]: a fixed grid of one interval, so the period does not drift by the time a
//! read takes, and when a read is slow the missed ticks are skipped instead of run back to back,
//! so a slow sample never causes a burst. There is one thread and no queue, so samples cannot
//! pile up. The wait is a channel receive with a timeout, so a stop request ends it at once.
//!
//! The decisions ([`decide`], [`next_deadline`]) and the loop run against fakes in tests; no
//! Tauri runtime is needed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager as _, Runtime};
use tauri_specta::Event as _;
use volt_core::{Clock, MetricsSnapshot, MetricsSource, MetricsTracker, SystemClock};
use volt_win::WinMetrics;

use crate::metrics::{MetricsUpdated, MetricsView};
use crate::window::MAIN_WINDOW;

/// Time between two samples.
pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Builds the metrics source. Called on the sampler thread each time sampling starts.
pub(crate) type SourceFactory = Arc<dyn Fn() -> Box<dyn MetricsSource> + Send + Sync>;

/// Receives each snapshot the sampler produces.
pub(crate) type Publish = Arc<dyn Fn(&MetricsSnapshot) + Send + Sync>;

/// Tells whether the window the sampler serves still exists.
pub(crate) type WindowAlive = Arc<dyn Fn() -> bool + Send + Sync>;

/// The time of the sample after the one scheduled at `scheduled`, given that it is `now`.
///
/// The ticks lie on a grid of `interval` starting at `scheduled`. The next deadline is the first
/// grid point after `now`: normally that is `scheduled + interval` (no drift), and when the loop
/// is late (a slow read) the grid points already in the past are skipped (no burst of catch-up
/// samples). A zero interval is treated as "now".
#[must_use]
pub(crate) fn next_deadline(scheduled: Instant, now: Instant, interval: Duration) -> Instant {
    if interval.is_zero() {
        return now;
    }
    let elapsed = now.saturating_duration_since(scheduled);
    let ticks_passed = elapsed.as_nanos() / interval.as_nanos();
    let next_tick = u32::try_from(ticks_passed.saturating_add(1)).unwrap_or(u32::MAX);
    // `checked_*` keep an absurd interval from panicking; falling back to now + interval is
    // always a valid deadline.
    interval
        .checked_mul(next_tick)
        .and_then(|offset| scheduled.checked_add(offset))
        .unwrap_or_else(|| now + interval)
}

/// What to do with the sampler when the existence of the window is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Transition {
    /// Start sampling: there is a window and no sampler.
    Start,
    /// Stop sampling: there is a sampler and no window.
    Stop,
    /// Nothing to do.
    Keep,
}

/// Decides the transition from whether sampling is running and whether the window exists.
#[must_use]
pub(crate) const fn decide(running: bool, window_open: bool) -> Transition {
    match (running, window_open) {
        (false, true) => Transition::Start,
        (true, false) => Transition::Stop,
        _ => Transition::Keep,
    }
}

/// What a run of the loop did, for the log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LoopStats {
    /// Snapshots published.
    pub samples: u64,
    /// Reads that failed.
    pub failures: u64,
}

/// Runs the sampling loop on the calling thread until `stop` is signalled (a message, or the
/// sender dropped) or `alive` reports that the window is gone.
///
/// One read is taken first as a baseline and not published, so the first snapshot already has a
/// CPU figure. A failing read is counted and logged once per streak, and sampling goes on.
pub(crate) fn run_loop(
    source: &dyn MetricsSource,
    clock: &dyn Clock,
    interval: Duration,
    stop: &Receiver<()>,
    alive: &dyn Fn() -> bool,
    publish: &mut dyn FnMut(&MetricsSnapshot),
) -> LoopStats {
    let mut stats = LoopStats::default();
    let mut tracker = MetricsTracker::new();
    match source.read() {
        Ok(raw) => tracker.prime(&raw),
        Err(error) => log::warn!("live metrics: the baseline read failed: {error}"),
    }
    let mut scheduled = Instant::now();
    let mut failing = false;
    loop {
        let deadline = next_deadline(scheduled, Instant::now(), interval);
        scheduled = deadline;
        match stop.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Err(RecvTimeoutError::Timeout) => {}
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
        }
        if !alive() {
            break;
        }
        let read = source.read();
        // A stop that arrived during a slow read must not publish one more snapshot.
        if matches!(stop.try_recv(), Ok(()) | Err(TryRecvError::Disconnected)) {
            break;
        }
        match read {
            Ok(raw) => {
                failing = false;
                let snapshot = tracker.update(&raw, clock.now());
                stats.samples += 1;
                publish(&snapshot);
            }
            Err(error) => {
                stats.failures += 1;
                if !failing {
                    failing = true;
                    log::warn!("live metrics: a read failed, will keep trying: {error}");
                }
            }
        }
    }
    stats
}

/// State shared between the controller and the sampler thread.
#[derive(Debug, Default)]
struct Shared {
    /// Bumped on every start and stop. A thread publishes only while its own generation is the
    /// current one, so a thread that was told to stop cannot overwrite a newer run's data.
    generation: AtomicU64,
    /// The latest snapshot of the current run.
    latest: Mutex<Option<MetricsSnapshot>>,
    /// Snapshots published since the app started, over all runs. For logs and tests.
    samples: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded data is replaced as a whole, so a poisoned lock holds nothing half-written.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A running sampler thread.
struct Running {
    stop: Sender<()>,
    thread: JoinHandle<()>,
}

impl Running {
    /// The thread ended by itself (the window vanished) or panicked.
    fn finished(&self) -> bool {
        self.thread.is_finished()
    }
}

/// Starts and stops the sampler thread. Lives in Tauri's managed state.
pub(crate) struct Sampler {
    source: SourceFactory,
    publish: Publish,
    alive: WindowAlive,
    clock: Arc<dyn Clock>,
    interval: Duration,
    shared: Arc<Shared>,
    running: Mutex<Option<Running>>,
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sampler")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl Sampler {
    /// A stopped sampler. `source` builds the metrics source each time sampling starts,
    /// `publish` receives every snapshot, and `alive` tells whether the window still exists.
    pub(crate) fn new(
        source: SourceFactory,
        publish: Publish,
        alive: WindowAlive,
        clock: Arc<dyn Clock>,
        interval: Duration,
    ) -> Self {
        Self {
            source,
            publish,
            alive,
            clock,
            interval,
            shared: Arc::new(Shared::default()),
            running: Mutex::new(None),
        }
    }

    /// Brings the sampler in line with the window: starts it when the window exists and it is
    /// not running, stops it when the window is gone and it is. Idempotent.
    ///
    /// `window_open` is called *inside* the sampler's lock. Two callers (a window being created
    /// and a window being destroyed) cannot both act on an outdated answer: the last one to take
    /// the lock sees the final state of the window.
    pub(crate) fn set_window_open(&self, window_open: impl FnOnce() -> bool) {
        let mut running = lock(&self.running);
        let is_running = running.as_ref().is_some_and(|run| !run.finished());
        match decide(is_running, window_open()) {
            Transition::Start => *running = self.spawn(),
            Transition::Stop => {
                // Detached on purpose: this can be called from the main thread, and the thread
                // ends by itself as soon as it sees the signal.
                drop(self.halt(&mut running));
            }
            Transition::Keep => {}
        }
    }

    /// Stops the sampler and waits for its thread to end. For tests and shutdown.
    #[cfg(test)]
    pub(crate) fn stop_and_wait(&self) {
        let handle = self.halt(&mut lock(&self.running));
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// Whether a sampler thread is running.
    #[cfg(test)]
    pub(crate) fn is_running(&self) -> bool {
        lock(&self.running)
            .as_ref()
            .is_some_and(|run| !run.finished())
    }

    /// The latest snapshot of the current run; `None` when stopped or before the first sample.
    pub(crate) fn latest(&self) -> Option<MetricsSnapshot> {
        lock(&self.shared.latest).clone()
    }

    /// Snapshots published since the app started, over all runs.
    #[cfg(test)]
    pub(crate) fn total_samples(&self) -> u64 {
        self.shared.samples.load(Ordering::SeqCst)
    }

    fn spawn(&self) -> Option<Running> {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (stop, signal) = mpsc::channel();
        let shared = Arc::clone(&self.shared);
        let source = Arc::clone(&self.source);
        let publish = Arc::clone(&self.publish);
        let alive = Arc::clone(&self.alive);
        let clock = Arc::clone(&self.clock);
        let interval = self.interval;
        let spawned = thread::Builder::new()
            .name("volt-metrics".into())
            .spawn(move || {
                // Built here and dropped when the loop ends: a closed window holds no counters.
                let metrics = source();
                log::info!("live metrics sampling started");
                let stats = run_loop(
                    metrics.as_ref(),
                    clock.as_ref(),
                    interval,
                    &signal,
                    alive.as_ref(),
                    &mut |snapshot| {
                        {
                            let mut latest = lock(&shared.latest);
                            if shared.generation.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            *latest = Some(snapshot.clone());
                        }
                        shared.samples.fetch_add(1, Ordering::SeqCst);
                        publish(snapshot);
                    },
                );
                drop(metrics);
                {
                    // A run that ended by itself (the window vanished) leaves no stale numbers.
                    let mut latest = lock(&shared.latest);
                    if shared.generation.load(Ordering::SeqCst) == generation {
                        *latest = None;
                    }
                }
                log::info!(
                    "live metrics sampling stopped after {} samples ({} failed reads)",
                    stats.samples,
                    stats.failures
                );
            });
        match spawned {
            Ok(thread) => Some(Running { stop, thread }),
            Err(error) => {
                log::error!("live metrics: the sampler thread could not start: {error}");
                None
            }
        }
    }

    /// Signals the running thread, invalidates its data and returns its handle.
    fn halt(&self, running: &mut Option<Running>) -> Option<JoinHandle<()>> {
        let run = running.take()?;
        {
            let mut latest = lock(&self.shared.latest);
            self.shared.generation.fetch_add(1, Ordering::SeqCst);
            *latest = None;
        }
        // The thread may be gone already (the window vanished first): nothing to signal then.
        let _ = run.stop.send(());
        Some(run.thread)
    }
}

/// Builds the real sampler: Windows metrics, a one second interval, snapshots emitted as the
/// typed `MetricsUpdated` event, and the main window as the thing it serves.
pub(crate) fn real<R: Runtime>(app: &AppHandle<R>) -> Sampler {
    let emitter = app.clone();
    let probe = app.clone();
    Sampler::new(
        Arc::new(|| Box::new(WinMetrics::new())),
        Arc::new(move |snapshot| {
            // A failed emit only means the UI misses one reading; the next one follows.
            let _ = MetricsUpdated(MetricsView::from(snapshot)).emit(&emitter);
        }),
        Arc::new(move || probe.get_webview_window(MAIN_WINDOW).is_some()),
        Arc::new(SystemClock),
        SAMPLE_INTERVAL,
    )
}

/// Starts or stops sampling to match whether the main window exists right now. Called after a
/// window was created and when one was destroyed. Does nothing before the sampler is managed.
pub(crate) fn sync_with_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(sampler) = app.try_state::<Sampler>() {
        sampler.set_window_open(|| app.get_webview_window(MAIN_WINDOW).is_some());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use volt_core::metrics::fake::{FakeMetrics, typical_sample};
    use volt_core::{PortError, Timestamp};

    use super::*;

    const TICK: Duration = Duration::from_millis(5);

    /// A clock that always says the same time.
    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> Timestamp {
            Timestamp::from_unix_millis(42)
        }
    }

    /// Waits (briefly) until `condition` holds.
    fn wait_until(condition: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// A sampler over a fake source that counts what it publishes. `window` plays the window.
    fn fake_sampler(
        fake: &Arc<FakeMetrics>,
        window: &Arc<AtomicBool>,
        published: &Arc<AtomicU64>,
    ) -> Sampler {
        let fake = Arc::clone(fake);
        let window = Arc::clone(window);
        let published = Arc::clone(published);
        Sampler::new(
            Arc::new(move || Box::new(Shared2(Arc::clone(&fake)))),
            Arc::new(move |_| {
                published.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(move || window.load(Ordering::SeqCst)),
            Arc::new(FixedClock),
            TICK,
        )
    }

    /// Lets the sampler own a boxed source while the test keeps the fake to inspect it.
    struct Shared2(Arc<FakeMetrics>);

    impl MetricsSource for Shared2 {
        fn read(&self) -> Result<volt_core::RawSample, PortError> {
            self.0.read()
        }
    }

    #[test]
    fn the_next_deadline_stays_on_the_grid_without_drift() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        // On time (a little after the previous deadline): the next grid point.
        assert_eq!(
            next_deadline(start, start + Duration::from_millis(30), second),
            start + second
        );
        // A later tick keeps the phase of the first one, not "now + interval".
        let scheduled = start + second;
        assert_eq!(
            next_deadline(scheduled, scheduled + Duration::from_millis(80), second),
            scheduled + second
        );
    }

    #[test]
    fn a_slow_sample_skips_the_missed_ticks_instead_of_bursting() {
        let start = Instant::now();
        let second = Duration::from_secs(1);
        // The read took 3.4 s: ticks 1, 2 and 3 are missed; the next one is tick 4.
        assert_eq!(
            next_deadline(start, start + Duration::from_millis(3400), second),
            start + second * 4
        );
        // Exactly on a grid point counts as passed.
        assert_eq!(
            next_deadline(start, start + second, second),
            start + second * 2
        );
    }

    #[test]
    fn a_zero_interval_does_not_panic_or_spin_on_the_grid() {
        let now = Instant::now();
        assert_eq!(next_deadline(now, now, Duration::ZERO), now);
    }

    #[test]
    fn sampling_follows_the_window() {
        assert_eq!(decide(false, true), Transition::Start);
        assert_eq!(decide(true, false), Transition::Stop);
        assert_eq!(decide(true, true), Transition::Keep);
        assert_eq!(decide(false, false), Transition::Keep);
    }

    #[test]
    fn nothing_is_read_while_no_window_has_been_opened() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let window = Arc::new(AtomicBool::new(false));
        let published = Arc::new(AtomicU64::new(0));
        let sampler = fake_sampler(&fake, &window, &published);
        // The window never exists, so the app asking to sync changes nothing.
        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        thread::sleep(TICK * 10);
        assert!(!sampler.is_running());
        assert_eq!(fake.reads(), 0, "no window, so no counter is read");
        assert_eq!(published.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn sampling_starts_with_the_window_and_stops_when_it_is_destroyed() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let window = Arc::new(AtomicBool::new(true));
        let published = Arc::new(AtomicU64::new(0));
        let sampler = fake_sampler(&fake, &window, &published);

        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(wait_until(|| published.load(Ordering::SeqCst) >= 3));
        assert!(sampler.is_running());
        assert!(
            sampler.latest().is_some(),
            "a snapshot is kept for the pull command"
        );

        // The window is destroyed.
        window.store(false, Ordering::SeqCst);
        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(
            sampler.latest().is_none(),
            "no stale numbers after the window closed"
        );
        assert!(wait_until(|| !sampler.is_running()));

        let reads = fake.reads();
        let published_before = published.load(Ordering::SeqCst);
        thread::sleep(TICK * 12);
        assert_eq!(
            fake.reads(),
            reads,
            "no read after the window was destroyed"
        );
        assert_eq!(
            published.load(Ordering::SeqCst),
            published_before,
            "nothing published either"
        );
    }

    #[test]
    fn asking_to_start_twice_runs_one_thread() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let window = Arc::new(AtomicBool::new(true));
        let published = Arc::new(AtomicU64::new(0));
        let sampler = fake_sampler(&fake, &window, &published);
        for _ in 0..3 {
            sampler.set_window_open(|| window.load(Ordering::SeqCst));
        }
        thread::sleep(TICK * 20);
        sampler.stop_and_wait();
        // One thread reads at most once per tick (plus the baseline); three would triple that.
        let reads = fake.reads();
        assert!(
            reads <= 20 + 2,
            "{reads} reads in 20 ticks means more than one thread"
        );
    }

    #[test]
    fn the_sampler_can_be_started_again_for_a_new_window() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let window = Arc::new(AtomicBool::new(true));
        let published = Arc::new(AtomicU64::new(0));
        let sampler = fake_sampler(&fake, &window, &published);

        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(wait_until(|| published.load(Ordering::SeqCst) >= 2));
        sampler.stop_and_wait();
        let after_first = sampler.total_samples();

        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(wait_until(|| sampler.total_samples() >= after_first + 2));
        assert!(sampler.is_running());
        sampler.stop_and_wait();
    }

    #[test]
    fn the_loop_ends_by_itself_when_the_window_is_gone() {
        let fake = Arc::new(FakeMetrics::constant(typical_sample()));
        let window = Arc::new(AtomicBool::new(true));
        let published = Arc::new(AtomicU64::new(0));
        let sampler = fake_sampler(&fake, &window, &published);
        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(wait_until(|| published.load(Ordering::SeqCst) >= 2));

        // The window vanishes and the destroy event is missed: the tick probe still stops it.
        window.store(false, Ordering::SeqCst);
        assert!(wait_until(|| !sampler.is_running()));
        let reads = fake.reads();
        thread::sleep(TICK * 10);
        assert_eq!(fake.reads(), reads);
        // And the controller recovers: a new window starts a new run.
        window.store(true, Ordering::SeqCst);
        sampler.set_window_open(|| window.load(Ordering::SeqCst));
        assert!(wait_until(|| sampler.is_running()));
        sampler.stop_and_wait();
    }

    /// Runs the loop on its own thread over an owned source, with a never-closing window.
    fn spawn_loop(
        source: impl MetricsSource + 'static,
        interval: Duration,
        mut on_snapshot: impl FnMut(&MetricsSnapshot) + Send + 'static,
    ) -> (Sender<()>, JoinHandle<LoopStats>) {
        let (stop, signal) = mpsc::channel();
        let worker = thread::spawn(move || {
            run_loop(
                &source,
                &FixedClock,
                interval,
                &signal,
                &|| true,
                &mut on_snapshot,
            )
        });
        (stop, worker)
    }

    #[test]
    fn a_failing_read_is_counted_and_sampling_goes_on() {
        let fake = FakeMetrics::scripted(vec![
            Ok(typical_sample()),
            Err(PortError::Other("counter failed".into())),
            Err(PortError::Other("counter failed".into())),
            Ok(typical_sample()),
        ]);
        let seen = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&seen);
        let (stop, worker) = spawn_loop(fake, TICK, move |snapshot| {
            assert_eq!(snapshot.taken_at, Timestamp::from_unix_millis(42));
            counter.fetch_add(1, Ordering::SeqCst);
        });
        assert!(wait_until(|| seen.load(Ordering::SeqCst) >= 3));
        let _ = stop.send(());
        let stats = worker.join().unwrap_or_default();
        assert_eq!(stats.failures, 2);
        assert!(stats.samples >= 3);
    }

    /// A source that takes four ticks to answer and counts its reads.
    struct SlowSource(Arc<AtomicU64>);

    impl MetricsSource for SlowSource {
        fn read(&self) -> Result<volt_core::RawSample, PortError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            thread::sleep(TICK * 4);
            Ok(typical_sample())
        }
    }

    #[test]
    fn a_slow_source_does_not_pile_up_reads() {
        let reads = Arc::new(AtomicU64::new(0));
        let started = Instant::now();
        let (stop, worker) = spawn_loop(SlowSource(Arc::clone(&reads)), TICK, |_| {});
        thread::sleep(TICK * 40);
        let _ = stop.send(());
        let stats = worker.join().unwrap_or_default();
        let elapsed = started.elapsed();

        // One thread, one read at a time: the number of reads cannot exceed the time that
        // passed divided by the cost of a read (plus the baseline and one in flight). A queue
        // of missed ticks would have run one read per tick, four times as many.
        let ticks_of_work = elapsed.as_millis() / (TICK * 4).as_millis();
        let reads = u128::from(reads.load(Ordering::SeqCst));
        assert!(
            reads <= ticks_of_work + 3,
            "{reads} reads of a 4-tick source in {elapsed:?}"
        );
        assert!(stats.samples >= 2, "{stats:?}");
    }

    #[test]
    fn a_stop_ends_the_wait_at_once() {
        let fake = FakeMetrics::constant(typical_sample());
        let started = Instant::now();
        // A one minute interval: only the stop signal can end this in time.
        let (stop, worker) = spawn_loop(fake, Duration::from_secs(60), |_| {});
        thread::sleep(Duration::from_millis(30));
        let _ = stop.send(());
        let stats = worker.join().unwrap_or_default();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(stats.samples, 0);
    }
}
