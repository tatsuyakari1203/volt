//! Live system metrics: the [`MetricsSource`] port, the snapshot types and the pure logic that
//! turns raw counters into percentages.
//!
//! A source reads *one raw sample*: cumulative CPU time counters, physical memory totals, the
//! space of every fixed volume and one reading per GPU adapter. A raw sample means nothing on
//! its own (a CPU counter only grows), so [`MetricsTracker`] keeps the previous sample and turns
//! each new one into a [`MetricsSnapshot`]: CPU load from the difference of two samples, memory
//! in use, per-disk used and free space, and the primary GPU.
//!
//! This port is **not** part of [`Ports`](crate::Ports): metrics are read-only and independent
//! of the executor, so the sampler is built with its own source.
//!
//! GPU and video memory are optional by type. A machine without a GPU, in a virtual machine, or
//! with counters switched off has an empty GPU list and its snapshot has `gpu: None`. That is a
//! normal case, not an error: every other metric is still reported.

use serde::{Deserialize, Serialize};

use crate::error::PortError;
use crate::ports::MemoryStatus;
use crate::time::Timestamp;

/// Cumulative CPU time counters since boot, summed over all logical processors.
///
/// The unit does not matter as long as all three fields use the same one (the Windows adapter
/// reports 100 ns ticks). Only differences between two samples are meaningful.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuTimes {
    /// Time spent idle.
    pub idle: u64,
    /// Time spent doing anything, idle included (kernel plus user time).
    pub total: u64,
}

/// Space of one fixed volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskReading {
    /// Where the volume is mounted, such as `C:\`.
    pub mount: String,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Free space in bytes.
    pub free_bytes: u64,
}

/// What one GPU adapter reported. Every number may be missing: counters differ per driver and
/// per Windows version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuReading {
    /// Adapter name, such as the marketing name of the card.
    pub name: String,
    /// Busy share of the busiest engine, 0 to 100. `None` when the counters gave no value yet
    /// (the first read after the counters were opened) or do not exist.
    pub utilisation_percent: Option<f64>,
    /// Dedicated video memory in use, in bytes.
    pub dedicated_used_bytes: Option<u64>,
    /// Dedicated video memory installed, in bytes.
    pub dedicated_total_bytes: Option<u64>,
}

/// One raw reading of the machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawSample {
    /// CPU time counters.
    pub cpu: CpuTimes,
    /// Physical memory totals.
    pub memory: MemoryStatus,
    /// Fixed volumes. May be empty when they cannot be listed.
    pub disks: Vec<DiskReading>,
    /// GPU adapters. Empty on a machine without a (readable) GPU, which is not an error.
    pub gpus: Vec<GpuReading>,
}

/// Port: reads one raw sample of the machine. Read-only: it changes nothing.
///
/// A failure to read the CPU or memory fails the sample. A part that is allowed to be missing
/// (disks, GPUs) is reported as empty instead.
pub trait MetricsSource: Send + Sync {
    /// Reads the counters now.
    fn read(&self) -> Result<RawSample, PortError>;
}

/// Physical memory, in use and available.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Installed physical memory in bytes.
    pub total_bytes: u64,
    /// Memory in use in bytes (total minus available).
    pub used_bytes: u64,
    /// Memory available for use in bytes.
    pub available_bytes: u64,
    /// `used_bytes` as a share of `total_bytes`, 0 to 100.
    pub used_percent: f32,
}

/// One fixed volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiskSnapshot {
    /// Where the volume is mounted, such as `C:\`.
    pub mount: String,
    /// Capacity in bytes.
    pub total_bytes: u64,
    /// Used space in bytes.
    pub used_bytes: u64,
    /// Free space in bytes.
    pub free_bytes: u64,
    /// `used_bytes` as a share of `total_bytes`, 0 to 100.
    pub used_percent: f32,
}

/// The GPU the dashboard shows: the adapter with the most dedicated video memory.
///
/// An integrated GPU reports the small dedicated segment it has, so its video memory figures are
/// real but small.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GpuSnapshot {
    /// Adapter name.
    pub name: String,
    /// Busy share of the busiest engine, 0 to 100, when known.
    pub utilisation_percent: Option<f32>,
    /// Dedicated video memory in use in bytes, when known.
    pub vram_used_bytes: Option<u64>,
    /// Dedicated video memory installed in bytes, when known.
    pub vram_total_bytes: Option<u64>,
    /// Video memory in use as a share of the installed amount, 0 to 100, when both are known.
    pub vram_used_percent: Option<f32>,
}

/// Everything the UI shows at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// When the sample was taken.
    pub taken_at: Timestamp,
    /// CPU load over the time since the previous sample, 0 to 100. `None` for the first sample
    /// of a tracker, and when the counters did not advance or went backwards.
    pub cpu_percent: Option<f32>,
    /// Physical memory.
    pub memory: MemorySnapshot,
    /// Fixed volumes, in the order the source listed them.
    pub disks: Vec<DiskSnapshot>,
    /// The primary GPU, or `None` when the machine has none (or none could be read).
    pub gpu: Option<GpuSnapshot>,
}

/// Turns raw samples into snapshots. Holds the previous CPU counters, so one tracker follows
/// one stream of samples.
#[derive(Debug, Clone, Default)]
pub struct MetricsTracker {
    previous_cpu: Option<CpuTimes>,
}

impl MetricsTracker {
    /// A tracker that has seen no sample yet.
    #[must_use]
    pub const fn new() -> Self {
        Self { previous_cpu: None }
    }

    /// Reads the CPU counters of `raw` into the tracker without producing a snapshot. Used to
    /// take a baseline, so the first real snapshot already has a CPU figure.
    pub fn prime(&mut self, raw: &RawSample) {
        self.previous_cpu = Some(raw.cpu);
    }

    /// Builds the snapshot for `raw`, taken at `taken_at`, and remembers its CPU counters.
    pub fn update(&mut self, raw: &RawSample, taken_at: Timestamp) -> MetricsSnapshot {
        let cpu_percent = self
            .previous_cpu
            .and_then(|previous| cpu_percent(previous, raw.cpu));
        self.previous_cpu = Some(raw.cpu);
        MetricsSnapshot {
            taken_at,
            cpu_percent,
            memory: memory_snapshot(raw.memory),
            disks: raw.disks.iter().filter_map(disk_snapshot).collect(),
            gpu: primary_gpu(&raw.gpus),
        }
    }
}

/// `part` as a percentage of `whole`, clamped to 0 to 100. Zero `whole` gives 0.
#[allow(
    clippy::cast_possible_truncation,
    reason = "the value is clamped to 0..=100 first, so it fits an f32 without loss that matters"
)]
fn percent_of(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    // Compute in f64 so 64-bit byte counts keep their precision until the final narrowing.
    #[allow(
        clippy::cast_precision_loss,
        reason = "byte counts above 2^53 are far beyond any real machine; a share needs no more precision"
    )]
    let share = part as f64 / whole as f64 * 100.0;
    share.clamp(0.0, 100.0) as f32
}

/// CPU load between two samples: the busy share of the time that passed. `None` when no time
/// passed or a counter went backwards (a counter reset), because neither gives a rate.
#[must_use]
pub fn cpu_percent(previous: CpuTimes, current: CpuTimes) -> Option<f32> {
    let total = current.total.checked_sub(previous.total)?;
    let idle = current.idle.checked_sub(previous.idle)?;
    if total == 0 {
        return None;
    }
    // Idle can exceed total by a rounding of the clock; that is no busy time, not a negative.
    let busy = total.saturating_sub(idle);
    Some(percent_of(busy, total))
}

/// Memory in use and available. A source that reports more available than installed is clamped:
/// used is never negative.
#[must_use]
pub fn memory_snapshot(status: MemoryStatus) -> MemorySnapshot {
    let available = status
        .available_physical_bytes
        .min(status.total_physical_bytes);
    let used = status.total_physical_bytes - available;
    MemorySnapshot {
        total_bytes: status.total_physical_bytes,
        used_bytes: used,
        available_bytes: available,
        used_percent: percent_of(used, status.total_physical_bytes),
    }
}

/// One volume's used and free space. A volume with no capacity (an empty card reader) is not a
/// disk to show and gives `None`; free space above the capacity is clamped to it.
#[must_use]
pub fn disk_snapshot(reading: &DiskReading) -> Option<DiskSnapshot> {
    if reading.total_bytes == 0 {
        return None;
    }
    let free = reading.free_bytes.min(reading.total_bytes);
    let used = reading.total_bytes - free;
    Some(DiskSnapshot {
        mount: reading.mount.clone(),
        total_bytes: reading.total_bytes,
        used_bytes: used,
        free_bytes: free,
        used_percent: percent_of(used, reading.total_bytes),
    })
}

/// Picks the GPU to show: the adapter with the most dedicated video memory; among equals the
/// first listed. No adapters gives `None`.
#[must_use]
pub fn primary_gpu(readings: &[GpuReading]) -> Option<GpuSnapshot> {
    let mut best: Option<&GpuReading> = None;
    for reading in readings {
        let better = best.is_none_or(|current| {
            reading.dedicated_total_bytes.unwrap_or(0) > current.dedicated_total_bytes.unwrap_or(0)
        });
        if better {
            best = Some(reading);
        }
    }
    best.map(|reading| {
        let vram_used_percent = match (reading.dedicated_used_bytes, reading.dedicated_total_bytes)
        {
            (Some(used), Some(total)) if total > 0 => Some(percent_of(used, total)),
            _ => None,
        };
        #[allow(clippy::cast_possible_truncation, reason = "clamped to 0..=100 first")]
        let utilisation_percent = reading
            .utilisation_percent
            .filter(|value| value.is_finite())
            .map(|value| value.clamp(0.0, 100.0) as f32);
        GpuSnapshot {
            name: reading.name.clone(),
            utilisation_percent,
            vram_used_bytes: reading.dedicated_used_bytes,
            vram_total_bytes: reading.dedicated_total_bytes,
            vram_used_percent,
        }
    })
}

/// A scripted [`MetricsSource`] for tests: it answers with the queued results in order, then
/// keeps repeating the last one. It reads nothing from the machine.
#[cfg(any(test, feature = "fakes"))]
pub mod fake {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, PoisonError};

    use super::{CpuTimes, MetricsSource, RawSample};
    use crate::error::PortError;
    use crate::ports::MemoryStatus;

    /// See the module documentation.
    #[derive(Debug)]
    pub struct FakeMetrics {
        script: Mutex<VecDeque<Result<RawSample, PortError>>>,
        last: Mutex<Result<RawSample, PortError>>,
        reads: AtomicUsize,
    }

    impl FakeMetrics {
        /// A fake that answers `results` in order and then repeats the last one. An empty
        /// script answers with a [`typical_sample`].
        #[must_use]
        pub fn scripted(results: Vec<Result<RawSample, PortError>>) -> Self {
            let last = results
                .last()
                .cloned()
                .unwrap_or_else(|| Ok(typical_sample()));
            Self {
                script: Mutex::new(results.into()),
                last: Mutex::new(last),
                reads: AtomicUsize::new(0),
            }
        }

        /// A fake that always answers `sample`.
        #[must_use]
        pub fn constant(sample: RawSample) -> Self {
            Self::scripted(vec![Ok(sample)])
        }

        /// How many times [`MetricsSource::read`] was called.
        #[must_use]
        pub fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
    }

    impl MetricsSource for FakeMetrics {
        fn read(&self) -> Result<RawSample, PortError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let next = self
                .script
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            match next {
                Some(result) => {
                    self.last
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .clone_from(&result);
                    result
                }
                None => self
                    .last
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            }
        }
    }

    /// A plausible sample of a machine with no GPU and one disk.
    #[must_use]
    pub fn typical_sample() -> RawSample {
        RawSample {
            cpu: CpuTimes {
                idle: 900,
                total: 1000,
            },
            memory: MemoryStatus {
                total_physical_bytes: 16 * 1024 * 1024 * 1024,
                available_physical_bytes: 4 * 1024 * 1024 * 1024,
            },
            disks: vec![super::DiskReading {
                mount: "C:\\".into(),
                total_bytes: 500_000_000_000,
                free_bytes: 200_000_000_000,
            }],
            gpus: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{FakeMetrics, typical_sample};
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn cpu(idle: u64, total: u64) -> CpuTimes {
        CpuTimes { idle, total }
    }

    fn gpu(name: &str, total: Option<u64>, used: Option<u64>, util: Option<f64>) -> GpuReading {
        GpuReading {
            name: name.into(),
            utilisation_percent: util,
            dedicated_used_bytes: used,
            dedicated_total_bytes: total,
        }
    }

    #[test]
    fn cpu_is_the_busy_share_of_the_time_between_two_samples() {
        assert_eq!(cpu_percent(cpu(100, 200), cpu(150, 300)), Some(50.0));
        assert_eq!(cpu_percent(cpu(0, 0), cpu(100, 100)), Some(0.0));
        assert_eq!(cpu_percent(cpu(0, 0), cpu(0, 100)), Some(100.0));
    }

    #[test]
    fn cpu_has_no_rate_when_no_time_passed_or_a_counter_went_backwards() {
        assert_eq!(cpu_percent(cpu(10, 100), cpu(10, 100)), None);
        assert_eq!(cpu_percent(cpu(10, 100), cpu(5, 50)), None);
        // Only the total went back, or only idle did.
        assert_eq!(cpu_percent(cpu(10, 100), cpu(20, 90)), None);
        assert_eq!(cpu_percent(cpu(10, 100), cpu(5, 200)), None);
    }

    #[test]
    fn cpu_idle_ahead_of_total_is_zero_busy_not_negative() {
        assert_eq!(cpu_percent(cpu(0, 0), cpu(120, 100)), Some(0.0));
    }

    #[test]
    fn memory_reports_in_use_and_available() {
        let snapshot = memory_snapshot(MemoryStatus {
            total_physical_bytes: 16 * GIB,
            available_physical_bytes: 4 * GIB,
        });
        assert_eq!(snapshot.total_bytes, 16 * GIB);
        assert_eq!(snapshot.available_bytes, 4 * GIB);
        assert_eq!(snapshot.used_bytes, 12 * GIB);
        assert!((snapshot.used_percent - 75.0).abs() < 1e-4);
    }

    #[test]
    fn memory_available_above_total_is_clamped() {
        let snapshot = memory_snapshot(MemoryStatus {
            total_physical_bytes: 8 * GIB,
            available_physical_bytes: 9 * GIB,
        });
        assert_eq!(snapshot.used_bytes, 0);
        assert_eq!(snapshot.available_bytes, 8 * GIB);
        assert!(snapshot.used_percent.abs() < 1e-6);
    }

    #[test]
    fn memory_with_nothing_installed_does_not_divide_by_zero() {
        let snapshot = memory_snapshot(MemoryStatus {
            total_physical_bytes: 0,
            available_physical_bytes: 0,
        });
        assert!(snapshot.used_percent.abs() < 1e-6);
    }

    #[test]
    fn a_disk_reports_used_and_free_space() {
        let snapshot = disk_snapshot(&DiskReading {
            mount: "D:\\".into(),
            total_bytes: 1000,
            free_bytes: 250,
        });
        let Some(snapshot) = snapshot else {
            panic!("a disk with capacity must be reported");
        };
        assert_eq!(snapshot.mount, "D:\\");
        assert_eq!((snapshot.used_bytes, snapshot.free_bytes), (750, 250));
        assert!((snapshot.used_percent - 75.0).abs() < 1e-4);
    }

    #[test]
    fn a_disk_without_capacity_is_skipped_and_odd_free_space_is_clamped() {
        let empty = DiskReading {
            mount: "E:\\".into(),
            total_bytes: 0,
            free_bytes: 0,
        };
        assert_eq!(disk_snapshot(&empty), None);
        let odd = DiskReading {
            mount: "F:\\".into(),
            total_bytes: 100,
            free_bytes: 400,
        };
        let Some(snapshot) = disk_snapshot(&odd) else {
            panic!("a disk with capacity must be reported");
        };
        assert_eq!((snapshot.used_bytes, snapshot.free_bytes), (0, 100));
    }

    #[test]
    fn no_gpu_adapter_means_no_gpu_snapshot() {
        assert_eq!(primary_gpu(&[]), None);
    }

    #[test]
    fn the_primary_gpu_is_the_adapter_with_the_most_dedicated_memory() {
        let picked = primary_gpu(&[
            gpu("Integrated", Some(GIB / 2), Some(GIB / 4), Some(3.0)),
            gpu("Discrete", Some(16 * GIB), Some(4 * GIB), Some(42.5)),
        ]);
        let Some(picked) = picked else {
            panic!("a GPU must be picked");
        };
        assert_eq!(picked.name, "Discrete");
        assert_eq!(picked.utilisation_percent, Some(42.5));
        assert_eq!(picked.vram_used_bytes, Some(4 * GIB));
        assert_eq!(picked.vram_total_bytes, Some(16 * GIB));
        assert_eq!(picked.vram_used_percent, Some(25.0));
    }

    #[test]
    fn a_gpu_with_missing_counters_keeps_the_values_it_has() {
        let only_total = primary_gpu(&[gpu("Card", Some(8 * GIB), None, None)]);
        let Some(only_total) = only_total else {
            panic!("a GPU must be picked");
        };
        assert_eq!(only_total.utilisation_percent, None);
        assert_eq!(only_total.vram_used_bytes, None);
        assert_eq!(only_total.vram_used_percent, None);
        assert_eq!(only_total.vram_total_bytes, Some(8 * GIB));

        let nothing = primary_gpu(&[gpu("Card", None, None, None)]);
        assert!(nothing.is_some_and(|g| g.vram_total_bytes.is_none()));
    }

    #[test]
    fn gpu_utilisation_is_clamped_and_never_not_a_number() {
        let over = primary_gpu(&[gpu("A", None, None, Some(180.0))]);
        assert_eq!(over.and_then(|g| g.utilisation_percent), Some(100.0));
        let nan = primary_gpu(&[gpu("A", None, None, Some(f64::NAN))]);
        assert_eq!(nan.and_then(|g| g.utilisation_percent), None);
    }

    #[test]
    fn the_first_snapshot_has_no_cpu_rate_and_later_ones_do() {
        let mut tracker = MetricsTracker::new();
        let mut raw = typical_sample();
        let first = tracker.update(&raw, Timestamp::from_unix_millis(1000));
        assert_eq!(first.cpu_percent, None);
        assert_eq!(first.taken_at, Timestamp::from_unix_millis(1000));

        raw.cpu = cpu(950, 1100);
        let second = tracker.update(&raw, Timestamp::from_unix_millis(2000));
        assert_eq!(second.cpu_percent, Some(50.0));
    }

    #[test]
    fn a_primed_tracker_gives_a_cpu_rate_at_once() {
        let mut tracker = MetricsTracker::new();
        let mut raw = typical_sample();
        tracker.prime(&raw);
        raw.cpu = cpu(900, 1100);
        let snapshot = tracker.update(&raw, Timestamp::from_unix_millis(0));
        assert_eq!(snapshot.cpu_percent, Some(100.0));
    }

    #[test]
    fn a_machine_without_a_gpu_still_gets_every_other_metric() {
        let mut tracker = MetricsTracker::new();
        let snapshot = tracker.update(&typical_sample(), Timestamp::from_unix_millis(0));
        assert_eq!(snapshot.gpu, None);
        assert_eq!(snapshot.memory.used_bytes, 12 * GIB);
        assert_eq!(snapshot.disks.len(), 1);
        assert_eq!(snapshot.disks[0].free_bytes, 200_000_000_000);
    }

    #[test]
    fn the_fake_answers_in_order_then_repeats_the_last_and_counts_reads() {
        let mut second = typical_sample();
        second.cpu = cpu(1, 2);
        let fake = FakeMetrics::scripted(vec![
            Ok(typical_sample()),
            Err(PortError::Other("counter failed".into())),
            Ok(second.clone()),
        ]);
        assert_eq!(fake.read(), Ok(typical_sample()));
        assert_eq!(fake.read(), Err(PortError::Other("counter failed".into())));
        assert_eq!(fake.read(), Ok(second.clone()));
        assert_eq!(fake.read(), Ok(second));
        assert_eq!(fake.reads(), 4);
    }
}
