//! Live metrics at the IPC boundary: the transfer types, the typed event the sampler emits and
//! the command that reads the latest snapshot.
//!
//! The types are defined here, in the shell, like the others (see [`crate::api`]): the UI gets
//! what it shows, and a change in the core cannot change the UI contract by accident.
//!
//! Byte counts cross as `f64` (a JavaScript number), not as 64-bit integers, which
//! `specta-typescript` refuses. A byte count is exact up to 2^53 (9 PB), far beyond any disk or
//! memory, and the UI needs to divide and format them, which strings would make awkward.

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::State;
use volt_core::{DiskSnapshot, GpuSnapshot, MemorySnapshot, MetricsSnapshot};

use crate::IpcError;
use crate::sampler::Sampler;

/// A byte count as a JavaScript number.
#[allow(
    clippy::cast_precision_loss,
    reason = "exact up to 2^53 bytes, which no real disk or memory reaches"
)]
fn bytes(value: u64) -> f64 {
    value as f64
}

/// Physical memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MemoryView {
    /// Installed physical memory in bytes.
    pub total_bytes: f64,
    /// Memory in use in bytes.
    pub used_bytes: f64,
    /// Memory available for use in bytes.
    pub available_bytes: f64,
    /// Memory in use as a share of the installed amount, 0 to 100.
    pub used_percent: f32,
}

impl From<&MemorySnapshot> for MemoryView {
    fn from(memory: &MemorySnapshot) -> Self {
        Self {
            total_bytes: bytes(memory.total_bytes),
            used_bytes: bytes(memory.used_bytes),
            available_bytes: bytes(memory.available_bytes),
            used_percent: memory.used_percent,
        }
    }
}

/// One fixed volume.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct DiskView {
    /// Where the volume is mounted, such as `C:\`.
    pub mount: String,
    /// Capacity in bytes.
    pub total_bytes: f64,
    /// Used space in bytes.
    pub used_bytes: f64,
    /// Free space in bytes.
    pub free_bytes: f64,
    /// Used space as a share of the capacity, 0 to 100.
    pub used_percent: f32,
}

impl From<&DiskSnapshot> for DiskView {
    fn from(disk: &DiskSnapshot) -> Self {
        Self {
            mount: disk.mount.clone(),
            total_bytes: bytes(disk.total_bytes),
            used_bytes: bytes(disk.used_bytes),
            free_bytes: bytes(disk.free_bytes),
            used_percent: disk.used_percent,
        }
    }
}

/// The primary GPU. Every number may be missing (`null`): a driver without counters still
/// gives a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GpuView {
    /// Adapter name.
    pub name: String,
    /// Busy share of the busiest engine, 0 to 100.
    pub utilisation_percent: Option<f32>,
    /// Dedicated video memory in use in bytes.
    pub vram_used_bytes: Option<f64>,
    /// Dedicated video memory installed in bytes.
    pub vram_total_bytes: Option<f64>,
    /// Video memory in use as a share of the installed amount, 0 to 100.
    pub vram_used_percent: Option<f32>,
}

impl From<&GpuSnapshot> for GpuView {
    fn from(gpu: &GpuSnapshot) -> Self {
        Self {
            name: gpu.name.clone(),
            utilisation_percent: gpu.utilisation_percent,
            vram_used_bytes: gpu.vram_used_bytes.map(bytes),
            vram_total_bytes: gpu.vram_total_bytes.map(bytes),
            vram_used_percent: gpu.vram_used_percent,
        }
    }
}

/// One reading of the machine, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MetricsView {
    /// When it was taken, ISO 8601 in UTC.
    pub taken_at: String,
    /// CPU load since the previous reading, 0 to 100. `null` when it could not be worked out.
    pub cpu_percent: Option<f32>,
    /// Physical memory.
    pub memory: MemoryView,
    /// Fixed volumes.
    pub disks: Vec<DiskView>,
    /// The primary GPU, or `null` on a machine without a (readable) GPU, which is normal.
    pub gpu: Option<GpuView>,
}

impl From<&MetricsSnapshot> for MetricsView {
    fn from(snapshot: &MetricsSnapshot) -> Self {
        Self {
            taken_at: snapshot.taken_at.to_iso8601(),
            cpu_percent: snapshot.cpu_percent,
            memory: MemoryView::from(&snapshot.memory),
            disks: snapshot.disks.iter().map(DiskView::from).collect(),
            gpu: snapshot.gpu.as_ref().map(GpuView::from),
        }
    }
}

/// Event emitted about once a second while the window is open, with the latest reading.
///
/// Generated as `events.metricsUpdated`. The sampler runs only while the main window exists, so
/// nothing is emitted (and nothing is read) while the app sits in the tray.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct MetricsUpdated(pub MetricsView);

/// Tauri command: the latest reading, or `null` when none has been taken yet (the window was
/// just opened and the first interval has not passed). Lets a screen show numbers at once
/// instead of waiting for the next event.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "every command returns Result<_, IpcError> so the UI handles one error shape, and               Tauri hands `State` over by value"
)]
pub(crate) fn latest_metrics(sampler: State<'_, Sampler>) -> Result<Option<MetricsView>, IpcError> {
    Ok(sampler.latest().as_ref().map(MetricsView::from))
}

#[cfg(test)]
mod tests {
    use volt_core::metrics::fake::typical_sample;
    use volt_core::{MetricsTracker, Timestamp};

    use super::*;

    #[test]
    fn the_view_carries_the_snapshot_and_a_missing_gpu_stays_null() {
        let mut tracker = MetricsTracker::new();
        let snapshot = tracker.update(&typical_sample(), Timestamp::from_unix_millis(0));
        let view = MetricsView::from(&snapshot);
        assert_eq!(view.taken_at, "1970-01-01T00:00:00.000Z");
        assert_eq!(view.cpu_percent, None);
        assert!(view.gpu.is_none());
        assert!((view.memory.used_bytes - 12.0 * 1024.0 * 1024.0 * 1024.0).abs() < 1.0);
        assert_eq!(view.disks.len(), 1);
        assert!((view.disks[0].free_bytes - 200_000_000_000.0).abs() < 1.0);
    }

    #[test]
    fn the_event_serialises_in_camel_case_with_nulls_for_missing_values() {
        let mut tracker = MetricsTracker::new();
        let snapshot = tracker.update(&typical_sample(), Timestamp::from_unix_millis(0));
        let json =
            serde_json::to_value(MetricsUpdated(MetricsView::from(&snapshot))).unwrap_or_default();
        assert_eq!(json["cpuPercent"], serde_json::Value::Null);
        assert_eq!(json["gpu"], serde_json::Value::Null);
        assert!(json["memory"]["usedPercent"].is_number());
        assert!(json["disks"][0]["totalBytes"].is_number());
    }
}
