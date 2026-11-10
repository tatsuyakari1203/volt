//! GPU readings for any vendor, without administrator rights.
//!
//! Utilisation and dedicated video memory in use come from the Windows performance counters
//! (`GPU Engine` and `GPU Adapter Memory`, the same source Task Manager uses). The adapter's name
//! and installed dedicated memory come from DXGI. Neither needs a vendor library, so an NVIDIA,
//! AMD, Intel or virtual adapter all work the same way.
//!
//! The counters exist since Windows 10 version 1709 and only with a WDDM 2 driver. A machine
//! without them, or without a hardware adapter, has no GPU readings: [`GpuReader::read`] then
//! returns an empty list, which the caller treats as a normal case.
//!
//! The parsing and aggregation of counter instance names is pure and tested on strings. The two
//! FFI readers ([`PdhGpuCounters`], [`enumerate_adapters`]) are tested on the real machine for
//! plausibility only.

use std::collections::HashMap;

use volt_core::{GpuReading, PortError};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIFactory1,
};
use windows::Win32::System::Performance::{
    PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery,
    PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::core::{PCWSTR, w};

/// Counter path of the busy share of every GPU engine of every process.
const ENGINE_COUNTER: PCWSTR = w!("\\GPU Engine(*)\\Utilization Percentage");

/// Counter path of the dedicated video memory in use per adapter, in bytes.
const MEMORY_COUNTER: PCWSTR = w!("\\GPU Adapter Memory(*)\\Dedicated Usage");

/// Windows reports success of the PDH functions as zero.
const ERROR_SUCCESS: u32 = 0;

/// Reads the GPU adapters of the machine. An empty list means "no (readable) GPU".
pub(crate) trait GpuReader: Send {
    /// One reading per hardware adapter, or an error when the counters could not be read.
    fn read(&mut self) -> Result<Vec<GpuReading>, PortError>;
}

/// A reader for a machine that has no GPU counters.
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
}

/// Combines the two halves of a LUID into one key.
fn luid_key(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// Reads the LUID out of a counter instance name such as
/// `pid_4242_luid_0x00000000_0x0000A1B2_phys_0_eng_3_engtype_3D` or
/// `luid_0x00000000_0x0000A1B2_phys_0`.
fn parse_luid(instance: &str) -> Option<u64> {
    let rest = instance.split("luid_0x").nth(1)?;
    let mut parts = rest.split('_');
    let high = u32::from_str_radix(parts.next()?, 16).ok()?;
    let low_text = parts.next()?.strip_prefix("0x")?;
    let low = u32::from_str_radix(low_text, 16).ok()?;
    Some(luid_key(high, low))
}

/// Identity of one physical engine inside an engine counter instance name: the adapter, the
/// physical adapter index and the engine index. Several processes share an engine, so their
/// instances carry the same key.
fn parse_engine_key(instance: &str) -> Option<(u64, u32, u32)> {
    let luid = parse_luid(instance)?;
    let number_after = |tag: &str| -> Option<u32> {
        let rest = instance.split(tag).nth(1)?;
        rest.split('_').next()?.parse().ok()
    };
    Some((luid, number_after("_phys_")?, number_after("_eng_")?))
}

/// Turns the `Utilization Percentage` instances into one figure per adapter: the processes'
/// shares are added up per engine (they share it), and the busiest engine represents the
/// adapter, which is how Task Manager reports a GPU. Capped at 100.
fn aggregate_engines(instances: &[(String, f64)]) -> HashMap<u64, f64> {
    let mut per_engine: HashMap<(u64, u32, u32), f64> = HashMap::new();
    for (name, value) in instances {
        if !value.is_finite() || *value < 0.0 {
            continue;
        }
        if let Some(key) = parse_engine_key(name) {
            *per_engine.entry(key).or_insert(0.0) += value;
        }
    }
    let mut per_adapter: HashMap<u64, f64> = HashMap::new();
    for ((luid, _, _), busy) in per_engine {
        let entry = per_adapter.entry(luid).or_insert(0.0);
        if busy > *entry {
            *entry = busy;
        }
    }
    for busy in per_adapter.values_mut() {
        *busy = busy.min(100.0);
    }
    per_adapter
}

/// Turns the `Dedicated Usage` instances into bytes per adapter (an adapter with several
/// physical parts reports one instance each; they add up).
fn aggregate_memory(instances: &[(String, f64)]) -> HashMap<u64, u64> {
    let mut per_adapter: HashMap<u64, u64> = HashMap::new();
    for (name, value) in instances {
        if !value.is_finite() || *value < 0.0 {
            continue;
        }
        if let Some(luid) = parse_luid(name) {
            // A byte count of a counter that is a double: round, never wrap.
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "checked finite and non-negative above; `as` saturates at the top"
            )]
            let bytes = value.round() as u64;
            let entry = per_adapter.entry(luid).or_insert(0);
            *entry = entry.saturating_add(bytes);
        }
    }
    per_adapter
}

/// Joins the adapters with their counter figures. Counters of an adapter DXGI does not list
/// (the software renderer, a removed device) are ignored; an adapter without counters is still
/// reported with its name and memory.
fn build_readings(
    adapters: &[Adapter],
    utilisation: &HashMap<u64, f64>,
    memory_used: &HashMap<u64, u64>,
) -> Vec<GpuReading> {
    adapters
        .iter()
        .map(|adapter| GpuReading {
            name: adapter.name.clone(),
            utilisation_percent: utilisation.get(&adapter.luid).copied(),
            dedicated_used_bytes: memory_used.get(&adapter.luid).copied(),
            dedicated_total_bytes: Some(adapter.dedicated_total_bytes),
        })
        .collect()
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
        adapters.push(Adapter {
            luid: luid_key(
                desc.AdapterLuid.HighPart.cast_unsigned(),
                desc.AdapterLuid.LowPart,
            ),
            name: String::from_utf16_lossy(&desc.Description[..name_len]),
            dedicated_total_bytes: u64::try_from(desc.DedicatedVideoMemory).unwrap_or(u64::MAX),
        });
    }
    Ok(adapters)
}

/// The instances of the two GPU counters at one moment: `(engine utilisation, memory in use)`,
/// each as `(instance name, value)` pairs.
type Collected = (Vec<Instance>, Vec<Instance>);

/// One counter instance: its name and its value.
type Instance = (String, f64);

/// An open PDH query with the two GPU counters. Closing the query releases the counters.
struct PdhGpuCounters {
    query: PDH_HQUERY,
    engine: Option<PDH_HCOUNTER>,
    memory: Option<PDH_HCOUNTER>,
}

// SAFETY: a PDH query handle is an opaque token, not tied to the thread that opened it. The
// struct is only ever used through `&mut self` (and so by one thread at a time), which is the
// serialisation PDH asks for.
unsafe impl Send for PdhGpuCounters {}

impl PdhGpuCounters {
    /// Opens a query and adds the counters that exist on this machine. Fails when neither does.
    fn open() -> Result<Self, PortError> {
        let mut query = PDH_HQUERY::default();
        // SAFETY: a null data source means "the local machine's live counters"; `query` is a
        // valid out pointer for the new handle.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut query) };
        if status != ERROR_SUCCESS {
            return Err(PortError::Other(format!("PdhOpenQuery: 0x{status:08X}")));
        }
        let engine = add_counter(query, ENGINE_COUNTER);
        let memory = add_counter(query, MEMORY_COUNTER);
        let counters = Self {
            query,
            engine,
            memory,
        };
        if counters.engine.is_none() && counters.memory.is_none() {
            // Dropping closes the query.
            return Err(PortError::Unsupported(
                "this Windows has no GPU performance counters".into(),
            ));
        }
        Ok(counters)
    }

    /// Takes one reading of the query. The first call returns no utilisation (a rate needs two
    /// points); later calls do.
    fn collect(&mut self) -> Result<Collected, PortError> {
        // SAFETY: `self.query` is a live handle that `open` created and `drop` closes.
        let status = unsafe { PdhCollectQueryData(self.query) };
        if status != ERROR_SUCCESS {
            return Err(PortError::Other(format!(
                "PdhCollectQueryData: 0x{status:08X}"
            )));
        }
        // A counter whose value is not available yet (the first read of a rate) is "no data",
        // not a failure of the whole reading.
        let engine = self
            .engine
            .map(|counter| read_array(counter).unwrap_or_default())
            .unwrap_or_default();
        let memory = self
            .memory
            .map(|counter| read_array(counter).unwrap_or_default())
            .unwrap_or_default();
        Ok((engine, memory))
    }
}

impl Drop for PdhGpuCounters {
    fn drop(&mut self) {
        // SAFETY: `self.query` is a live handle owned by this struct and is closed exactly once,
        // here. The counter handles belong to the query and become invalid with it.
        let _ = unsafe { PdhCloseQuery(self.query) };
    }
}

/// Adds a counter to the query. A path that does not exist on this Windows gives `None`.
fn add_counter(query: PDH_HQUERY, path: PCWSTR) -> Option<PDH_HCOUNTER> {
    let mut counter = PDH_HCOUNTER::default();
    // SAFETY: `query` is a live handle, `path` is a NUL-terminated wide string (it comes from
    // `w!`) and `counter` is a valid out pointer. The English variant is used so the path does
    // not depend on the display language.
    let status = unsafe { PdhAddEnglishCounterW(query, path, 0, &raw mut counter) };
    (status == ERROR_SUCCESS).then_some(counter)
}

/// Reads every instance of a wildcard counter as `(instance name, value)`. Instances whose value
/// is not valid yet are left out.
fn read_array(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, u32> {
    let mut size = 0u32;
    let mut count = 0u32;
    // SAFETY: with no buffer this call only reports the size and item count it needs.
    let mut status = unsafe {
        PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &raw mut size, &raw mut count, None)
    };
    // The instance list can grow between the two calls (a process starts using the GPU), so
    // the second call is retried with the larger size it asks for.
    for _ in 0..4 {
        if status != PDH_MORE_DATA {
            break;
        }
        // 8-byte units: the items hold a pointer and an f64, so the buffer must be 8-aligned.
        let mut buffer = vec![0u64; (size as usize).div_ceil(8) + 1];
        let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        let mut capacity = u32::try_from(buffer.len() * 8).unwrap_or(u32::MAX);
        // SAFETY: `items` points at `capacity` writable, 8-aligned bytes that outlive the call;
        // PDH fills the item array at the start and the instance name strings after it, all
        // inside that buffer.
        status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &raw mut capacity,
                &raw mut count,
                Some(items),
            )
        };
        if status == ERROR_SUCCESS {
            let mut out = Vec::with_capacity(count as usize);
            for index in 0..count as usize {
                // SAFETY: PDH wrote `count` initialised items at the start of the buffer, and
                // `index < count`.
                let item = unsafe { &*items.add(index) };
                let state = item.FmtValue.CStatus;
                if state != PDH_CSTATUS_VALID_DATA && state != PDH_CSTATUS_NEW_DATA {
                    continue;
                }
                // SAFETY: `szName` points at a NUL-terminated UTF-16 string inside the buffer
                // that PDH just filled.
                let name = unsafe { item.szName.to_string() }.unwrap_or_default();
                // SAFETY: the format asked for was `PDH_FMT_DOUBLE`, so `doubleValue` is the
                // member PDH wrote.
                let value = unsafe { item.FmtValue.Anonymous.doubleValue };
                out.push((name, value));
            }
            return Ok(out);
        }
        size = capacity;
    }
    if status == ERROR_SUCCESS {
        // Zero instances: the first call succeeded without needing a buffer.
        return Ok(Vec::new());
    }
    Err(status)
}

/// The real reader: DXGI for the adapters, PDH for utilisation and memory in use.
pub(crate) struct PdhGpuReader {
    adapters: Vec<Adapter>,
    counters: PdhGpuCounters,
}

impl std::fmt::Debug for PdhGpuReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PdhGpuReader")
            .field("adapters", &self.adapters.len())
            .finish_non_exhaustive()
    }
}

impl PdhGpuReader {
    /// Opens the reader. Fails when there is no hardware adapter or no counter, which the caller
    /// turns into "this machine has no readable GPU".
    pub(crate) fn open() -> Result<Self, PortError> {
        let adapters = enumerate_adapters()?;
        if adapters.is_empty() {
            return Err(PortError::NotFound("no hardware GPU adapter".into()));
        }
        Ok(Self {
            adapters,
            counters: PdhGpuCounters::open()?,
        })
    }
}

impl GpuReader for PdhGpuReader {
    fn read(&mut self) -> Result<Vec<GpuReading>, PortError> {
        let (engine, memory) = self.counters.collect()?;
        Ok(build_readings(
            &self.adapters,
            &aggregate_engines(&engine),
            &aggregate_memory(&memory),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENGINE_A: &str = "pid_4242_luid_0x00000000_0x0000A1B2_phys_0_eng_0_engtype_3D";
    const ENGINE_A_COPY: &str = "pid_9_luid_0x00000000_0x0000A1B2_phys_0_eng_0_engtype_3D";
    const ENGINE_A_VIDEO: &str =
        "pid_9_luid_0x00000000_0x0000A1B2_phys_0_eng_5_engtype_VideoDecode";
    const ENGINE_B: &str = "pid_7_luid_0x00000001_0x0000C3D4_phys_0_eng_0_engtype_3D";
    const LUID_A: u64 = 0xA1B2;
    const LUID_B: u64 = (1 << 32) | 0xC3D4;

    fn inst(name: &str, value: f64) -> (String, f64) {
        (name.to_owned(), value)
    }

    #[test]
    fn luids_are_read_from_engine_and_memory_instance_names() {
        assert_eq!(parse_luid(ENGINE_A), Some(LUID_A));
        assert_eq!(parse_luid(ENGINE_B), Some(LUID_B));
        assert_eq!(
            parse_luid("luid_0x00000000_0x0000A1B2_phys_0"),
            Some(LUID_A)
        );
        assert_eq!(parse_luid("something_else"), None);
        assert_eq!(parse_luid("luid_0xZZ_0x1"), None);
        assert_eq!(parse_luid("luid_0x00000000"), None);
    }

    #[test]
    fn an_engine_key_names_the_adapter_and_the_engine_but_not_the_process() {
        assert_eq!(parse_engine_key(ENGINE_A), parse_engine_key(ENGINE_A_COPY));
        assert_eq!(parse_engine_key(ENGINE_A), Some((LUID_A, 0, 0)));
        assert_eq!(parse_engine_key(ENGINE_A_VIDEO), Some((LUID_A, 0, 5)));
        assert_eq!(parse_engine_key("garbage"), None);
    }

    #[test]
    fn processes_sharing_an_engine_add_up_and_the_busiest_engine_wins() {
        let utilisation = aggregate_engines(&[
            inst(ENGINE_A, 20.0),
            inst(ENGINE_A_COPY, 15.0),
            inst(ENGINE_A_VIDEO, 30.0),
            inst(ENGINE_B, 5.0),
        ]);
        // Engine 0 of A is 20 + 15 = 35, the video engine 30: the adapter shows 35.
        assert_eq!(utilisation.get(&LUID_A), Some(&35.0));
        assert_eq!(utilisation.get(&LUID_B), Some(&5.0));
    }

    #[test]
    fn utilisation_is_capped_and_bad_values_are_ignored() {
        let utilisation = aggregate_engines(&[
            inst(ENGINE_A, 80.0),
            inst(ENGINE_A_COPY, 60.0),
            inst(ENGINE_B, f64::NAN),
            inst("not an engine", 50.0),
            inst(ENGINE_A_VIDEO, -3.0),
        ]);
        assert_eq!(utilisation.get(&LUID_A), Some(&100.0));
        assert_eq!(utilisation.get(&LUID_B), None);
        assert_eq!(utilisation.len(), 1);
    }

    #[test]
    fn dedicated_memory_adds_up_per_adapter() {
        let memory = aggregate_memory(&[
            inst("luid_0x00000000_0x0000A1B2_phys_0", 1000.0),
            inst("luid_0x00000000_0x0000A1B2_phys_1", 500.4),
            inst("luid_0x00000001_0x0000C3D4_phys_0", 42.0),
            inst("luid_0x00000001_0x0000C3D4_phys_0", f64::INFINITY),
        ]);
        assert_eq!(memory.get(&LUID_A), Some(&1500));
        assert_eq!(memory.get(&LUID_B), Some(&42));
    }

    #[test]
    fn readings_join_adapters_with_counters_and_ignore_unknown_luids() {
        let adapters = [Adapter {
            luid: LUID_A,
            name: "Test GPU".into(),
            dedicated_total_bytes: 8_000,
        }];
        let utilisation = aggregate_engines(&[inst(ENGINE_A, 12.5), inst(ENGINE_B, 99.0)]);
        let used = aggregate_memory(&[inst("luid_0x00000000_0x0000A1B2_phys_0", 2_000.0)]);
        let readings = build_readings(&adapters, &utilisation, &used);
        assert_eq!(
            readings,
            vec![GpuReading {
                name: "Test GPU".into(),
                utilisation_percent: Some(12.5),
                dedicated_used_bytes: Some(2_000),
                dedicated_total_bytes: Some(8_000),
            }]
        );
    }

    #[test]
    fn an_adapter_without_counters_keeps_its_name_and_memory() {
        let adapters = [Adapter {
            luid: LUID_A,
            name: "Quiet GPU".into(),
            dedicated_total_bytes: 4_000,
        }];
        let readings = build_readings(&adapters, &HashMap::new(), &HashMap::new());
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].utilisation_percent, None);
        assert_eq!(readings[0].dedicated_used_bytes, None);
        assert_eq!(readings[0].dedicated_total_bytes, Some(4_000));
    }

    #[test]
    fn no_adapters_give_no_readings() {
        assert!(build_readings(&[], &HashMap::new(), &HashMap::new()).is_empty());
        assert_eq!(NoGpu.read(), Ok(Vec::new()));
    }

    /// Reads the real adapters and counters. Changes nothing. A machine without a GPU takes the
    /// `Err` branch, which is as valid as an NVIDIA one: only plausibility is asserted.
    #[test]
    fn the_real_reader_gives_plausible_numbers_or_cleanly_no_gpu() {
        let Ok(mut reader) = PdhGpuReader::open() else {
            return;
        };
        // The first read has no utilisation yet; the second has.
        let _ = reader.read();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let readings = reader.read().unwrap_or_default();
        for reading in &readings {
            assert!(!reading.name.is_empty());
            if let Some(percent) = reading.utilisation_percent {
                assert!((0.0..=100.0).contains(&percent), "{percent}");
            }
            if let (Some(used), Some(total)) =
                (reading.dedicated_used_bytes, reading.dedicated_total_bytes)
            {
                // Dedicated usage can exceed the installed amount only by a small spill; it must
                // stay in the same order of magnitude.
                assert!(used <= total.saturating_mul(2), "{used} of {total}");
            }
        }
    }
}
