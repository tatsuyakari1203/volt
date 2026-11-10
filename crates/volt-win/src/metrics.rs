//! [`MetricsSource`] port on the Windows API.
//!
//! Everything here works without administrator rights and for any GPU vendor:
//!
//! - CPU: `GetSystemTimes` (cumulative idle, kernel and user time).
//! - Memory: `GlobalMemoryStatusEx`.
//! - Disks: `GetLogicalDrives`, `GetDriveTypeW` and `GetDiskFreeSpaceExW` for fixed volumes only,
//!   so a removable drive, an optical drive or a network share is never touched.
//! - GPU: `D3DKMTQueryStatistics` and DXGI, see [`crate::gpu`]. A machine without a readable GPU
//!   reports an empty list, never an error.
//!
//! Reading changes nothing on the machine.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use volt_core::{
    CpuTimes, DiskReading, GpuReading, MemoryStatus, MetricsSource, PortError, RawSample,
};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::PCWSTR;

use crate::gpu::{GpuReader, KmtGpuReader, NoGpu};
use crate::wide::to_wide_nul;

/// `GetDriveTypeW` result for a fixed (non-removable, non-network) drive.
const DRIVE_FIXED: u32 = 3;

/// The metrics source backed by the Windows API.
///
/// It keeps the previous GPU node times to compute utilisation, so create it when sampling starts and
/// drop it when sampling stops.
pub struct WinMetrics {
    gpu: Mutex<Box<dyn GpuReader>>,
    gpu_failed: AtomicBool,
}

impl std::fmt::Debug for WinMetrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WinMetrics").finish_non_exhaustive()
    }
}

impl Default for WinMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl WinMetrics {
    /// Opens the source. A machine with no readable GPU (a virtual machine, an old Windows, a
    /// driver without statistics) is a normal case: it is logged at debug level and every read
    /// then reports an empty GPU list.
    #[must_use]
    pub fn new() -> Self {
        match KmtGpuReader::open() {
            Ok(reader) => Self::with_gpu_reader(Box::new(reader)),
            Err(error) => {
                log::debug!("no GPU metrics on this machine: {error}");
                Self::with_gpu_reader(Box::new(NoGpu))
            }
        }
    }

    /// Opens a source that never reads the GPU: no GPU statistics are queried, so a
    /// one-off read of CPU time, memory and disks (the health check) stays cheap and leaves
    /// nothing behind. Every read reports an empty GPU list.
    #[must_use]
    pub fn without_gpu() -> Self {
        Self::with_gpu_reader(Box::new(NoGpu))
    }

    /// Builds the source over another GPU reader. Tests use it to inject a failing one.
    pub(crate) fn with_gpu_reader(reader: Box<dyn GpuReader>) -> Self {
        Self {
            gpu: Mutex::new(reader),
            gpu_failed: AtomicBool::new(false),
        }
    }

    /// Reads the GPU part. A failure is logged once (not every second) and reads as no GPU.
    fn read_gpu(&self) -> Vec<GpuReading> {
        let mut reader = self.gpu.lock().unwrap_or_else(PoisonError::into_inner);
        match reader.read() {
            Ok(readings) => {
                self.gpu_failed.store(false, Ordering::Relaxed);
                readings
            }
            Err(error) => {
                if !self.gpu_failed.swap(true, Ordering::Relaxed) {
                    log::warn!("GPU metrics could not be read, reporting none: {error}");
                }
                Vec::new()
            }
        }
    }
}

impl MetricsSource for WinMetrics {
    fn read(&self) -> Result<RawSample, PortError> {
        Ok(RawSample {
            cpu: read_cpu_times()?,
            memory: read_memory()?,
            disks: read_disks(),
            gpus: self.read_gpu(),
        })
    }
}

fn filetime_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Cumulative idle and busy time of all processors, in 100 ns ticks.
fn read_cpu_times() -> Result<CpuTimes, PortError> {
    let mut idle = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: the three out pointers are valid, writable `FILETIME`s that live for the call.
    unsafe {
        GetSystemTimes(
            Some(&raw mut idle),
            Some(&raw mut kernel),
            Some(&raw mut user),
        )
    }
    .map_err(|e| PortError::Other(format!("GetSystemTimes: {e}")))?;
    Ok(CpuTimes {
        idle: filetime_ticks(idle),
        // Kernel time already includes idle time, so kernel plus user is the whole.
        total: filetime_ticks(kernel).saturating_add(filetime_ticks(user)),
    })
}

pub(crate) fn read_memory() -> Result<MemoryStatus, PortError> {
    let mut status = MEMORYSTATUSEX {
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the struct is a few dozen bytes"
        )]
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: `status` is a valid, writable `MEMORYSTATUSEX` with `dwLength` set as the API
    // requires.
    unsafe { GlobalMemoryStatusEx(&raw mut status) }
        .map_err(|e| PortError::Other(format!("GlobalMemoryStatusEx: {e}")))?;
    Ok(MemoryStatus {
        total_physical_bytes: status.ullTotalPhys,
        available_physical_bytes: status.ullAvailPhys,
    })
}

/// The fixed volumes with a drive letter. A volume whose space cannot be read (a locked
/// `BitLocker` volume, for example) is left out: the other disks are still useful.
fn read_disks() -> Vec<DiskReading> {
    // SAFETY: no arguments and no preconditions.
    let letters = unsafe { GetLogicalDrives() };
    let mut disks = Vec::new();
    for bit in 0..26u8 {
        if letters & (1 << bit) == 0 {
            continue;
        }
        let mount = format!("{}:\\", char::from(b'A' + bit));
        if let Some(disk) = read_disk(&mount) {
            disks.push(disk);
        }
    }
    disks
}

fn read_disk(mount: &str) -> Option<DiskReading> {
    let wide = to_wide_nul(mount, "drive root").ok()?;
    let root = PCWSTR(wide.as_ptr());
    // SAFETY: `root` points at the NUL-terminated string in `wide`, alive for the call.
    if unsafe { GetDriveTypeW(root) } != DRIVE_FIXED {
        return None;
    }
    let mut total = 0u64;
    let mut free = 0u64;
    // SAFETY: `root` is as above; both out pointers are valid, writable `u64`s, and the
    // caller-quota value is not requested.
    unsafe { GetDiskFreeSpaceExW(root, None, Some(&raw mut total), Some(&raw mut free)) }.ok()?;
    Some(DiskReading {
        mount: mount.to_owned(),
        total_bytes: total,
        free_bytes: free,
    })
}

#[cfg(test)]
mod tests {
    use volt_core::MetricsTracker;
    use volt_core::Timestamp;

    use super::*;

    /// A GPU reader that always fails, to prove a broken GPU never breaks the other metrics.
    struct BrokenGpu;

    impl GpuReader for BrokenGpu {
        fn read(&mut self) -> Result<Vec<GpuReading>, PortError> {
            Err(PortError::Other("injected counter failure".into()))
        }
    }

    #[test]
    fn the_real_machine_reads_plausible_cpu_memory_and_disks() {
        let source = WinMetrics::new();
        let first = source.read().expect("read the machine");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let second = source.read().expect("read the machine again");

        // Counters only grow.
        assert!(second.cpu.total > first.cpu.total);
        assert!(second.cpu.total >= second.cpu.idle);
        assert!(second.memory.total_physical_bytes > 0);
        assert!(second.memory.available_physical_bytes <= second.memory.total_physical_bytes);
        // The system drive is fixed, so there is at least one disk with sane numbers.
        assert!(!second.disks.is_empty());
        for disk in &second.disks {
            assert!(disk.mount.ends_with(":\\"), "{}", disk.mount);
            assert!(disk.total_bytes > 0);
            assert!(disk.free_bytes <= disk.total_bytes);
        }

        let mut tracker = MetricsTracker::new();
        tracker.prime(&first);
        let snapshot = tracker.update(&second, Timestamp::from_unix_millis(0));
        let cpu = snapshot.cpu_percent.expect("the counters advanced");
        assert!((0.0..=100.0).contains(&cpu), "{cpu}");
        assert!((0.0..=100.0).contains(&snapshot.memory.used_percent));
    }

    #[test]
    fn a_failing_gpu_reader_leaves_every_other_metric_and_no_gpu() {
        let source = WinMetrics::with_gpu_reader(Box::new(BrokenGpu));
        let sample = source.read().expect("a broken GPU is not a read error");
        assert!(sample.gpus.is_empty());
        assert!(sample.memory.total_physical_bytes > 0);
        assert!(!sample.disks.is_empty());
        // Failing again keeps working (the failure is only logged once).
        assert!(source.read().expect("second read").gpus.is_empty());

        let mut tracker = MetricsTracker::new();
        let snapshot = tracker.update(&sample, Timestamp::from_unix_millis(0));
        assert_eq!(snapshot.gpu, None);
    }

    #[test]
    fn a_machine_without_gpu_counters_reads_as_no_gpu() {
        let source = WinMetrics::with_gpu_reader(Box::new(NoGpu));
        let sample = source.read().expect("read");
        assert!(sample.gpus.is_empty());
    }

    #[test]
    fn a_source_without_gpu_still_reads_memory_and_disks_and_reports_no_gpu() {
        let sample = WinMetrics::without_gpu().read().expect("read");
        assert!(sample.gpus.is_empty());
        assert!(sample.memory.total_physical_bytes > 0);
        assert!(!sample.disks.is_empty());
    }

    #[test]
    fn only_fixed_drives_are_listed() {
        for disk in read_disks() {
            let wide = to_wide_nul(&disk.mount, "drive root").expect("wide");
            // SAFETY: `wide` is NUL-terminated and alive for the call.
            let kind = unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) };
            assert_eq!(kind, DRIVE_FIXED, "{}", disk.mount);
        }
    }

    /// Measures what one read costs and shows what this machine reports. Not run by default (it
    /// takes seconds and prints): `cargo test -p volt-win --release read_cost -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "benchmark: prints what this machine reports and the cost of one read"]
    fn read_cost() {
        let source = WinMetrics::new();
        let first = source.read().expect("read");
        std::thread::sleep(std::time::Duration::from_secs(1));
        let sample = source.read().expect("read");
        let mut tracker = MetricsTracker::new();
        tracker.prime(&first);
        let snapshot = tracker.update(&sample, Timestamp::from_unix_millis(0));
        println!("{snapshot:#?}");

        let reads = 200u32;
        let started = std::time::Instant::now();
        for _ in 0..reads {
            source.read().expect("read");
        }
        let per_read = started.elapsed() / reads;
        println!("mean wall time of one read over {reads} reads: {per_read:?}");

        let cpu_started = std::time::Instant::now();
        for _ in 0..reads {
            read_cpu_times().expect("cpu");
            read_memory().expect("memory");
        }
        println!("cpu + memory only: {:?}", cpu_started.elapsed() / reads);
        let disk_started = std::time::Instant::now();
        for _ in 0..reads {
            read_disks();
        }
        println!("disks only: {:?}", disk_started.elapsed() / reads);
        let gpu_started = std::time::Instant::now();
        for _ in 0..reads {
            source.read_gpu();
        }
        println!("gpu only: {:?}", gpu_started.elapsed() / reads);
    }

    #[test]
    fn a_missing_drive_is_skipped_not_an_error() {
        // `Q:\` may or may not exist; a path that cannot be a fixed disk reads as nothing.
        assert!(read_disk("not a drive").is_none());
    }
}
