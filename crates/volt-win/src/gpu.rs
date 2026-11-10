//! GPU readings for any vendor, without administrator rights.
//!
//! Utilisation and dedicated video memory in use come from the video memory manager's own
//! statistics, `D3DKMTQueryStatistics` in `gdi32.dll`, the interface Task Manager and System
//! Informer read. It needs no vendor library and no elevation, and unlike the PDH performance
//! counters (which Volt used before) it loads no process-wide counter tables, so the memory of
//! an idle Volt does not stay higher after the live metrics ran once. The adapter's name and
//! installed dedicated memory come from DXGI. An NVIDIA, AMD, Intel or virtual adapter all work
//! the same way.
//!
//! Sources, checked against the `windows` crate 0.62 bindings (generated from the WDK header
//! `d3dkmthk.h`) and against System Informer's `plugins/ExtendedTools/gpumon.c`:
//! - `D3DKMT_QUERYSTATISTICS_ADAPTER` gives the segment count and the node count of an adapter.
//! - `D3DKMT_QUERYSTATISTICS_SEGMENT` gives, per segment, `BytesResident` and `Aperture`. A
//!   segment with `Aperture` set is system memory shared with the GPU; the others are dedicated
//!   video memory, which is what is added up.
//! - `D3DKMT_QUERYSTATISTICS_NODE` gives, per node (engine), `GlobalInformation.RunningTime`,
//!   the time the node has been busy, in 100 ns units. Utilisation is the delta of that over the
//!   delta of the wall clock; the busiest node represents the adapter, as Task Manager does.
//!
//! The first read has no utilisation (a rate needs two points). A machine without a hardware
//! adapter, or where the query fails, has no GPU readings: [`GpuReader::read`] then returns an
//! empty list or an error, which the caller treats as a normal case.
//!
//! The arithmetic is pure and tested through a fake of [`KmtStats`]. The FFI implementation
//! ([`D3dkmt`], [`enumerate_adapters`]) is tested on the real machine for plausibility only.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use volt_core::{GpuReading, PortError};
use windows::Wdk::Graphics::Direct3D::{
    D3DKMT_ADAPTERADDRESS, D3DKMT_CLOSEADAPTER, D3DKMT_OPENADAPTERFROMLUID,
    D3DKMT_QUERYADAPTERINFO, D3DKMT_QUERYSTATISTICS, D3DKMT_QUERYSTATISTICS_ADAPTER,
    D3DKMT_QUERYSTATISTICS_NODE, D3DKMT_QUERYSTATISTICS_QUERY_NODE,
    D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT, D3DKMT_QUERYSTATISTICS_SEGMENT,
    D3DKMT_QUERYSTATISTICS_TYPE, D3DKMTCloseAdapter, D3DKMTOpenAdapterFromLuid,
    D3DKMTQueryAdapterInfo, D3DKMTQueryStatistics, KMTQAITYPE_ADAPTERADDRESS,
};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};

/// Reads the GPU adapters of the machine. An empty list means "no (readable) GPU".
pub(crate) trait GpuReader: Send {
    /// One reading per hardware adapter, or an error when the statistics could not be read.
    fn read(&mut self) -> Result<Vec<GpuReading>, PortError>;
}

/// A reader for a machine that has no GPU statistics.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct NoGpu;

impl GpuReader for NoGpu {
    fn read(&mut self) -> Result<Vec<GpuReading>, PortError> {
        Ok(Vec::new())
    }
}

/// A hardware adapter known from DXGI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Adapter {
    /// The adapter's locally unique id, as `high << 32 | low`.
    pub luid: u64,
    /// Marketing name.
    pub name: String,
    /// Installed dedicated video memory in bytes.
    pub dedicated_total_bytes: u64,
    /// Where the card sits on the PCI bus, `None` when the driver does not say.
    pub location: Option<PciLocation>,
    /// Whether the video memory manager gives statistics for this LUID.
    pub queryable: bool,
}

/// The PCI bus, device and function of a card: unique per physical card, shared by every
/// adapter entry Windows lists for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PciLocation {
    pub bus: u32,
    pub device: u32,
    pub function: u32,
}

/// Keeps one adapter per physical card, in the order the cards first appear.
///
/// Windows can list one card as several DXGI adapters (different LUIDs, same name and memory).
/// The key is the PCI location, not name plus memory: two real identical cards share name and
/// memory but never a slot, while every listing of one card shares its slot. An adapter whose
/// location is unknown is never merged (a wrong merge would hide a real card). Of the listings
/// of one card the one with statistics and then the most dedicated memory is kept.
fn one_per_card(adapters: Vec<Adapter>) -> Vec<Adapter> {
    let mut kept: Vec<Adapter> = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        let twin = adapter
            .location
            .and_then(|at| kept.iter().position(|k| k.location == Some(at)));
        match twin {
            Some(index) => {
                let better = (adapter.queryable, adapter.dedicated_total_bytes)
                    > (kept[index].queryable, kept[index].dedicated_total_bytes);
                if better {
                    kept[index] = adapter;
                }
            }
            None => kept.push(adapter),
        }
    }
    kept
}

/// The PCI location of an adapter, `None` when it cannot be read.
fn pci_location(luid: u64) -> Option<PciLocation> {
    let mut open = D3DKMT_OPENADAPTERFROMLUID {
        AdapterLuid: LUID {
            LowPart: u32::try_from(luid & 0xFFFF_FFFF).unwrap_or(0),
            HighPart: u32::try_from(luid >> 32).unwrap_or(0).cast_signed(),
        },
        ..Default::default()
    };
    // SAFETY: `open` is a fully initialised input that lives for the call; the call fills
    // `hAdapter` on success.
    if unsafe { D3DKMTOpenAdapterFromLuid(&raw mut open) }.0 < 0 {
        return None;
    }
    let mut address = D3DKMT_ADAPTERADDRESS::default();
    let mut query = D3DKMT_QUERYADAPTERINFO {
        hAdapter: open.hAdapter,
        Type: KMTQAITYPE_ADAPTERADDRESS,
        pPrivateDriverData: (&raw mut address).cast(),
        PrivateDriverDataSize: u32::try_from(size_of::<D3DKMT_ADAPTERADDRESS>()).unwrap_or(0),
    };
    // SAFETY: `query` names the adapter just opened, and its data pointer and size describe
    // `address`, which is alive for the call and is the type this query type writes.
    let status = unsafe { D3DKMTQueryAdapterInfo(&raw mut query) };
    let close = D3DKMT_CLOSEADAPTER {
        hAdapter: open.hAdapter,
    };
    // SAFETY: `close` holds the handle opened above, closed exactly once.
    let _ = unsafe { D3DKMTCloseAdapter(&raw const close) };
    (status.0 >= 0).then_some(PciLocation {
        bus: address.BusNumber,
        device: address.DeviceNumber,
        function: address.FunctionNumber,
    })
}

/// Combines the two halves of a LUID into one key.
fn luid_key(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// How many segments and nodes an adapter has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AdapterShape {
    pub segments: u32,
    pub nodes: u32,
}

/// One memory segment of an adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SegmentUsage {
    /// System memory shared with the GPU (not dedicated video memory).
    pub shared: bool,
    /// Bytes resident in the segment.
    pub resident_bytes: u64,
}

/// The video memory manager statistics Volt needs. The real one is [`D3dkmt`]; tests use a fake.
pub(crate) trait KmtStats: Send {
    /// Segment and node counts, `None` when the adapter cannot be queried.
    fn shape(&self, luid: u64) -> Option<AdapterShape>;
    /// One segment, `None` when it cannot be queried.
    fn segment(&self, luid: u64, id: u32) -> Option<SegmentUsage>;
    /// The total busy time of one node in 100 ns units, `None` when it cannot be queried.
    fn node_running_time(&self, luid: u64, id: u32) -> Option<i64>;
}

/// Dedicated bytes in use: the resident bytes of every non-shared segment.
fn dedicated_used(stats: &dyn KmtStats, luid: u64, segments: u32) -> Option<u64> {
    let mut total: Option<u64> = None;
    for id in 0..segments {
        if let Some(segment) = stats.segment(luid, id)
            && !segment.shared
        {
            total = Some(total.unwrap_or(0).saturating_add(segment.resident_bytes));
        }
    }
    total
}

/// Busy share of one node between two readings of its running time, 0 to 100. `None` when the
/// clock did not advance or the counter went backwards (a driver reset).
fn node_busy_percent(before: i64, after: i64, elapsed: Duration) -> Option<f64> {
    let elapsed_100ns = elapsed.as_secs_f64() * 1e7;
    if after < before || elapsed_100ns <= 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_precision_loss,
        reason = "a busy-time delta far below 2^53 units; a percentage needs no more precision"
    )]
    let busy = (after - before) as f64;
    Some((busy / elapsed_100ns * 100.0).clamp(0.0, 100.0))
}

/// The last reading of an adapter's nodes.
struct Previous {
    at: Instant,
    running: Vec<Option<i64>>,
}

/// The real reader: DXGI for the adapters, `D3DKMTQueryStatistics` for utilisation and memory.
pub(crate) struct KmtGpuReader<S: KmtStats> {
    adapters: Vec<Adapter>,
    stats: S,
    previous: HashMap<u64, Previous>,
}

impl<S: KmtStats> std::fmt::Debug for KmtGpuReader<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KmtGpuReader")
            .field("adapters", &self.adapters.len())
            .finish_non_exhaustive()
    }
}

impl<S: KmtStats> KmtGpuReader<S> {
    /// Builds a reader over some adapters and a statistics source.
    pub(crate) fn new(adapters: Vec<Adapter>, stats: S) -> Self {
        Self {
            adapters,
            stats,
            previous: HashMap::new(),
        }
    }

    /// Reads every adapter at the moment `now`.
    fn read_at(&mut self, now: Instant) -> Vec<GpuReading> {
        let mut readings = Vec::with_capacity(self.adapters.len());
        for adapter in &self.adapters {
            let shape = self.stats.shape(adapter.luid);
            let used = shape.and_then(|s| dedicated_used(&self.stats, adapter.luid, s.segments));
            let mut utilisation = None;
            if let Some(shape) = shape {
                let running: Vec<Option<i64>> = (0..shape.nodes)
                    .map(|id| self.stats.node_running_time(adapter.luid, id))
                    .collect();
                if let Some(before) = self.previous.get(&adapter.luid) {
                    let elapsed = now.saturating_duration_since(before.at);
                    utilisation = running
                        .iter()
                        .zip(&before.running)
                        .filter_map(|(after, before)| {
                            node_busy_percent((*before)?, (*after)?, elapsed)
                        })
                        .reduce(f64::max);
                }
                self.previous
                    .insert(adapter.luid, Previous { at: now, running });
            }
            readings.push(GpuReading {
                name: adapter.name.clone(),
                utilisation_percent: utilisation,
                dedicated_used_bytes: used,
                dedicated_total_bytes: Some(adapter.dedicated_total_bytes),
            });
        }
        readings
    }
}

impl KmtGpuReader<D3dkmt> {
    /// Opens the reader. Fails when there is no hardware adapter or the statistics cannot be
    /// queried, which the caller turns into "this machine has no readable GPU".
    pub(crate) fn open() -> Result<Self, PortError> {
        let adapters = enumerate_adapters()?;
        if adapters.is_empty() {
            return Err(PortError::NotFound("no hardware GPU adapter".into()));
        }
        if !adapters.iter().any(|a| D3dkmt.shape(a.luid).is_some()) {
            return Err(PortError::Unsupported(
                "the video memory manager gives no statistics".into(),
            ));
        }
        Ok(Self::new(adapters, D3dkmt))
    }
}

impl<S: KmtStats> GpuReader for KmtGpuReader<S> {
    fn read(&mut self) -> Result<Vec<GpuReading>, PortError> {
        Ok(self.read_at(Instant::now()))
    }
}

/// `D3DKMTQueryStatistics`, a stateless call into `gdi32.dll`.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct D3dkmt;

impl D3dkmt {
    /// Runs one query. `fill` sets the query-specific input; the filled query is returned when
    /// the call succeeds (NTSTATUS >= 0).
    fn query(
        luid: u64,
        kind: D3DKMT_QUERYSTATISTICS_TYPE,
        fill: impl FnOnce(&mut D3DKMT_QUERYSTATISTICS),
    ) -> Option<D3DKMT_QUERYSTATISTICS> {
        let mut query = D3DKMT_QUERYSTATISTICS {
            Type: kind,
            AdapterLuid: LUID {
                LowPart: u32::try_from(luid & 0xFFFF_FFFF).unwrap_or(0),
                HighPart: u32::try_from(luid >> 32).unwrap_or(0).cast_signed(),
            },
            ..Default::default()
        };
        fill(&mut query);
        // SAFETY: `query` is a fully initialised, correctly typed D3DKMT_QUERYSTATISTICS that
        // lives for the whole call. The API declares the pointer `const` but writes the result
        // into `QueryResult` (as System Informer relies on), so the pointer is derived from a
        // mutable borrow, which makes that write sound.
        let status = unsafe { D3DKMTQueryStatistics((&raw mut query).cast_const()) };
        (status.0 >= 0).then_some(query)
    }
}

impl KmtStats for D3dkmt {
    fn shape(&self, luid: u64) -> Option<AdapterShape> {
        let result = Self::query(luid, D3DKMT_QUERYSTATISTICS_ADAPTER, |_| {})?;
        // SAFETY: the query type was ADAPTER and it succeeded, so the driver wrote the
        // `AdapterInformation` member of the result union.
        let info = unsafe { result.QueryResult.AdapterInformation };
        Some(AdapterShape {
            segments: info.NbSegments,
            nodes: info.NodeCount,
        })
    }

    fn segment(&self, luid: u64, id: u32) -> Option<SegmentUsage> {
        let result = Self::query(luid, D3DKMT_QUERYSTATISTICS_SEGMENT, |q| {
            q.Anonymous.QuerySegment = D3DKMT_QUERYSTATISTICS_QUERY_SEGMENT { SegmentId: id };
        })?;
        // SAFETY: the query type was SEGMENT and it succeeded, so the driver wrote the
        // `SegmentInformation` member of the result union.
        let info = unsafe { result.QueryResult.SegmentInformation };
        Some(SegmentUsage {
            shared: info.Aperture != 0,
            resident_bytes: info.BytesResident,
        })
    }

    fn node_running_time(&self, luid: u64, id: u32) -> Option<i64> {
        let result = Self::query(luid, D3DKMT_QUERYSTATISTICS_NODE, |q| {
            q.Anonymous.QueryNode = D3DKMT_QUERYSTATISTICS_QUERY_NODE { NodeId: id };
        })?;
        // SAFETY: the query type was NODE and it succeeded, so the driver wrote the
        // `NodeInformation` member of the result union.
        let info = unsafe { result.QueryResult.NodeInformation };
        Some(info.GlobalInformation.RunningTime)
    }
}

/// Lists the hardware adapters through DXGI. Software adapters (the Basic Render Driver) are
/// skipped: they are not a GPU the user could be asked about.
pub(crate) fn enumerate_adapters() -> Result<Vec<Adapter>, PortError> {
    // SAFETY: `CreateDXGIFactory1` has no preconditions; the returned COM object is owned and
    // released by the `windows` crate wrapper when it is dropped.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|e| PortError::Other(format!("DXGI factory: {e}")))?;
    let mut adapters = Vec::new();
    for index in 0.. {
        // SAFETY: `factory` is a live COM object; `EnumAdapters1` only reads the index. It
        // fails with DXGI_ERROR_NOT_FOUND after the last adapter, which ends the loop.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: `adapter` is a live COM object and `GetDesc1` takes no input.
        let Ok(desc) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        #[allow(
            clippy::cast_sign_loss,
            reason = "DXGI_ADAPTER_FLAG is a bit set that Windows declares as a signed int"
        )]
        let software = desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0;
        if software {
            continue;
        }
        let name_len = desc
            .Description
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(desc.Description.len());
        let luid = luid_key(
            desc.AdapterLuid.HighPart.cast_unsigned(),
            desc.AdapterLuid.LowPart,
        );
        adapters.push(Adapter {
            luid,
            location: pci_location(luid),
            queryable: D3dkmt.shape(luid).is_some(),
            name: String::from_utf16_lossy(&desc.Description[..name_len]),
            dedicated_total_bytes: u64::try_from(desc.DedicatedVideoMemory).unwrap_or(u64::MAX),
        });
    }
    Ok(one_per_card(adapters))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake of the statistics: one adapter, given segments and node running times that the
    /// test advances.
    struct Fake {
        segments: Vec<SegmentUsage>,
        running: std::sync::Arc<std::sync::Mutex<Vec<i64>>>,
    }

    impl KmtStats for Fake {
        fn shape(&self, _: u64) -> Option<AdapterShape> {
            Some(AdapterShape {
                segments: u32::try_from(self.segments.len()).ok()?,
                nodes: u32::try_from(self.running.lock().ok()?.len()).ok()?,
            })
        }
        fn segment(&self, _: u64, id: u32) -> Option<SegmentUsage> {
            self.segments.get(id as usize).copied()
        }
        fn node_running_time(&self, _: u64, id: u32) -> Option<i64> {
            self.running.lock().ok()?.get(id as usize).copied()
        }
    }

    fn adapter() -> Adapter {
        Adapter {
            luid: 7,
            name: "Test GPU".into(),
            dedicated_total_bytes: 4_000,
            location: None,
            queryable: true,
        }
    }

    fn at(luid: u64, slot: Option<u32>, queryable: bool, memory: u64) -> Adapter {
        Adapter {
            luid,
            name: "RTX".into(),
            dedicated_total_bytes: memory,
            location: slot.map(|bus| PciLocation {
                bus,
                device: 0,
                function: 0,
            }),
            queryable,
        }
    }

    #[test]
    fn a_card_listed_twice_is_kept_once_preferring_the_one_with_statistics() {
        let listed = vec![
            at(1, Some(1), false, 12_000),
            at(2, Some(1), true, 12_000),
            at(3, Some(1), true, 0),
        ];
        let kept = one_per_card(listed);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].luid, 2);
    }

    #[test]
    fn two_identical_cards_in_different_slots_stay_two_and_unknown_slots_are_never_merged() {
        let kept = one_per_card(vec![
            at(1, Some(1), true, 12_000),
            at(2, Some(2), true, 12_000),
            at(3, None, true, 12_000),
            at(4, None, true, 12_000),
        ]);
        let luids: Vec<u64> = kept.iter().map(|a| a.luid).collect();
        assert_eq!(luids, [1, 2, 3, 4]);
    }

    #[test]
    fn luid_halves_are_combined() {
        assert_eq!(luid_key(1, 2), (1 << 32) | 2);
    }

    #[test]
    fn busy_percent_is_the_busy_delta_over_the_clock_delta() {
        // 5 000 000 units of 100 ns = 0.5 s busy over 1 s.
        let busy = node_busy_percent(10, 5_000_010, Duration::from_secs(1));
        assert!((busy.unwrap_or(-1.0) - 50.0).abs() < 1e-9);
        // More busy time than wall time (clock skew) is capped.
        assert_eq!(
            node_busy_percent(0, 99_000_000, Duration::from_secs(1)),
            Some(100.0)
        );
        // A counter that went backwards or a clock that did not move gives nothing.
        assert_eq!(node_busy_percent(10, 5, Duration::from_secs(1)), None);
        assert_eq!(node_busy_percent(0, 5, Duration::ZERO), None);
    }

    #[test]
    fn dedicated_memory_adds_up_the_non_shared_segments_only() {
        let running = std::sync::Arc::default();
        let fake = Fake {
            segments: vec![
                SegmentUsage {
                    shared: false,
                    resident_bytes: 300,
                },
                SegmentUsage {
                    shared: true,
                    resident_bytes: 9_000,
                },
                SegmentUsage {
                    shared: false,
                    resident_bytes: 50,
                },
            ],
            running,
        };
        assert_eq!(dedicated_used(&fake, 7, 3), Some(350));
        // No segment at all is "unknown", not zero.
        assert_eq!(dedicated_used(&fake, 7, 0), None);
    }

    #[test]
    fn the_first_read_has_no_utilisation_and_the_second_uses_the_busiest_node() {
        let running = std::sync::Arc::new(std::sync::Mutex::new(vec![0, 0]));
        let fake = Fake {
            segments: vec![SegmentUsage {
                shared: false,
                resident_bytes: 1_000,
            }],
            running: running.clone(),
        };
        let mut reader = KmtGpuReader::new(vec![adapter()], fake);
        let start = Instant::now();
        let first = reader.read_at(start);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].utilisation_percent, None);
        assert_eq!(first[0].dedicated_used_bytes, Some(1_000));
        assert_eq!(first[0].dedicated_total_bytes, Some(4_000));

        // Over 2 s node 0 was busy 0.5 s (25 %) and node 1 1.5 s (75 %).
        *running.lock().expect("lock") = vec![5_000_000, 15_000_000];
        let second = reader.read_at(start + Duration::from_secs(2));
        let percent = second[0].utilisation_percent.expect("utilisation");
        assert!((percent - 75.0).abs() < 1e-9, "{percent}");
    }

    #[test]
    fn an_adapter_that_cannot_be_queried_is_reported_with_name_and_total_only() {
        struct Dead;
        impl KmtStats for Dead {
            fn shape(&self, _: u64) -> Option<AdapterShape> {
                None
            }
            fn segment(&self, _: u64, _: u32) -> Option<SegmentUsage> {
                None
            }
            fn node_running_time(&self, _: u64, _: u32) -> Option<i64> {
                None
            }
        }
        let mut reader = KmtGpuReader::new(vec![adapter()], Dead);
        let readings = reader.read().expect("never an error");
        assert_eq!(readings[0].name, "Test GPU");
        assert_eq!(readings[0].utilisation_percent, None);
        assert_eq!(readings[0].dedicated_used_bytes, None);
        assert_eq!(readings[0].dedicated_total_bytes, Some(4_000));
        assert_eq!(NoGpu.read(), Ok(Vec::new()));
    }

    /// Reads the real adapters. Changes nothing. A machine without a GPU takes the `Err`
    /// branch, which is as valid as an NVIDIA one: only plausibility is asserted.
    #[test]
    fn the_real_reader_gives_plausible_numbers_or_cleanly_no_gpu() {
        let Ok(mut reader) = KmtGpuReader::open() else {
            return;
        };
        let _ = reader.read();
        std::thread::sleep(Duration::from_millis(300));
        let readings = reader.read().unwrap_or_default();
        for reading in &readings {
            assert!(!reading.name.is_empty());
            if let Some(percent) = reading.utilisation_percent {
                assert!((0.0..=100.0).contains(&percent), "{percent}");
            }
            if let (Some(used), Some(total)) =
                (reading.dedicated_used_bytes, reading.dedicated_total_bytes)
            {
                assert!(used <= total.saturating_mul(2), "{used} of {total}");
            }
        }
    }
}
