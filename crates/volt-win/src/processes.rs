//! [`Processes`] port on `NtQuerySystemInformation(SystemProcessInformation)` and
//! `TerminateProcess`.
//!
//! One system call returns every process with its parent, private working set and kernel plus
//! user CPU time, including the protected ones a handle cannot be opened to. That is the same
//! source Task Manager reads, it is cheap (one call), and reading changes nothing. The executable
//! path needs one limited-rights handle per process (it is left out for the processes a normal user
//! cannot open). Ending a process opens only that process with `PROCESS_TERMINATE`, and checks on
//! that handle that it started when the listing said, so a reused process id is never ended.
//!
//! Reading and changing the priority class, the CPU affinity and the power throttling of a process
//! opens only that process with the right to change it (`PROCESS_SET_INFORMATION`), which an
//! unelevated user has for their own unprotected processes and not for elevated, other users' or
//! protected ones. Asking for that right when reading means a process that can be read can be
//! changed, so a refusal shows up before anything is journaled. The creation time is checked on
//! the handle, as for ending a process.

use std::ffi::c_void;

use volt_core::{
    PortError, PowerThrottling, PriorityClass, ProcessChange, ProcessInfo, ProcessSettings,
    Processes,
};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE,
    STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH,
};
use windows::Win32::System::Threading::{
    ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS, GetPriorityClass,
    GetProcessAffinityMask, GetProcessInformation, GetProcessTimes, HIGH_PRIORITY_CLASS,
    IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, OpenProcess, PROCESS_CREATION_FLAGS,
    PROCESS_NAME_WIN32, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_TERMINATE,
    ProcessPowerThrottling, QueryFullProcessImageNameW, SetPriorityClass, SetProcessAffinityMask,
    SetProcessInformation, TerminateProcess,
};
use windows::core::PWSTR;

use crate::error::from_win32;

/// `SystemProcessInformation`.
const SYSTEM_PROCESS_INFORMATION_CLASS: i32 = 5;

// The record layout below is the 64-bit one (pointers are 8 bytes).
const _: () = assert!(size_of::<usize>() == 8);

// Offsets into one `SYSTEM_PROCESS_INFORMATION` record (phnt's `ntexapi.h`; the first fields are
// also in the documented `winternl.h` structure, which hides `WorkingSetPrivateSize` in
// `Reserved`).
const OFFSET_NEXT_ENTRY: usize = 0;
const OFFSET_PRIVATE_WORKING_SET: usize = 8;
const OFFSET_CREATE_TIME: usize = 32;
const OFFSET_USER_TIME: usize = 40;
const OFFSET_KERNEL_TIME: usize = 48;
const OFFSET_IMAGE_NAME_LENGTH: usize = 56;
const OFFSET_IMAGE_NAME_BUFFER: usize = 64;
const OFFSET_PID: usize = 80;
const OFFSET_PARENT_PID: usize = 88;
const OFFSET_SESSION_ID: usize = 100;
/// Bytes of a record up to and including the session id.
const RECORD_MIN: usize = 104;

/// The id Windows gives the idle pseudo-process; it is not a program and is left out.
const IDLE_PID: u32 = 0;
/// The id of the System process; the adapter refuses to end it whatever the caller says.
const SYSTEM_PID: u32 = 4;

/// The processes port backed by the system process list.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinProcesses;

impl WinProcesses {
    /// A processes adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// A process handle, closed on drop.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful OpenProcess call, is owned exclusively by
        // this value and is closed once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn read_u16(buf: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(buf.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(buf: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(buf: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(buf.get(at..at + 8)?.try_into().ok()?))
}

/// The executable name of the record at `start`: the UTF-16 text its `ImageName` points to,
/// which lives in the same buffer (`base` is the address of the buffer's first byte).
fn image_name(buf: &[u8], start: usize, base: usize) -> Option<String> {
    let length = usize::from(read_u16(buf, start + OFFSET_IMAGE_NAME_LENGTH)?);
    if length == 0 {
        return Some(String::new());
    }
    let address = usize::try_from(read_u64(buf, start + OFFSET_IMAGE_NAME_BUFFER)?).ok()?;
    let from = address.checked_sub(base)?;
    let bytes = buf.get(from..from.checked_add(length)?)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units))
}

/// Reads the records of a `SystemProcessInformation` buffer. Pure: it only reads `buf`, which
/// starts at address `base`. A record that does not fit the buffer ends the walk.
/// The executable path is filled in by the caller; here it is `None`.
fn parse_records(buf: &[u8], base: usize) -> Vec<ProcessInfo> {
    let mut processes = Vec::new();
    let mut start = 0usize;
    while start + RECORD_MIN <= buf.len() {
        let (Some(next), Some(pid), Some(parent)) = (
            read_u32(buf, start + OFFSET_NEXT_ENTRY),
            read_u64(buf, start + OFFSET_PID),
            read_u64(buf, start + OFFSET_PARENT_PID),
        ) else {
            break;
        };
        // Ids are 32 bits wide; the field is pointer sized.
        let pid = u32::try_from(pid).unwrap_or(IDLE_PID);
        if pid != IDLE_PID {
            let ticks = |at: usize| read_u64(buf, start + at).unwrap_or(0);
            processes.push(ProcessInfo {
                pid,
                parent_pid: u32::try_from(parent).unwrap_or(0),
                name: image_name(buf, start, base).unwrap_or_default(),
                working_set_bytes: ticks(OFFSET_PRIVATE_WORKING_SET),
                cpu_time_100ns: ticks(OFFSET_USER_TIME).saturating_add(ticks(OFFSET_KERNEL_TIME)),
                created_at_100ns: ticks(OFFSET_CREATE_TIME),
                path: None,
                session_id: read_u32(buf, start + OFFSET_SESSION_ID).unwrap_or(0),
            });
        }
        if next == 0 {
            break;
        }
        start += next as usize;
    }
    processes
}

/// Reads the raw process list into an 8-byte aligned buffer, growing it until it fits.
fn read_buffer() -> Result<Vec<u64>, PortError> {
    let mut words = 1usize << 16;
    loop {
        let mut buffer = vec![0u64; words];
        let bytes = u32::try_from(words * size_of::<u64>())
            .map_err(|_| PortError::Other("the process list is too large".into()))?;
        let mut needed = 0u32;
        // SAFETY: `buffer` is `bytes` writable bytes, aligned for the record fields, and
        // `needed` is a valid out pointer; both live for the call. The class only reads.
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_INFORMATION_CLASS(SYSTEM_PROCESS_INFORMATION_CLASS),
                buffer.as_mut_ptr().cast::<c_void>(),
                bytes,
                &raw mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH || status == STATUS_BUFFER_TOO_SMALL {
            // Processes may start between the two calls, so ask for a little more.
            let wanted = usize::try_from(needed).unwrap_or(0) / size_of::<u64>() + 4096;
            words = wanted.max(words * 2);
            continue;
        }
        if status.0 < 0 {
            return Err(PortError::Other(format!(
                "listing processes: NTSTATUS 0x{:08X}",
                status.0.cast_unsigned()
            )));
        }
        return Ok(buffer);
    }
}

impl Processes for WinProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        let buffer = read_buffer()?;
        // SAFETY: a `u64` buffer is valid as bytes (no padding, any bit pattern is a byte), the
        // length is its exact size and the slice does not outlive `buffer`.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                buffer.as_ptr().cast::<u8>(),
                buffer.len() * size_of::<u64>(),
            )
        };
        let mut list = parse_records(bytes, buffer.as_ptr() as usize);
        for process in &mut list {
            // The kernel's own processes (System, Registry, ...) have no file to name.
            if process.pid > SYSTEM_PID {
                process.path = image_path(process.pid, process.created_at_100ns);
            }
        }
        Ok(list)
    }

    fn terminate(&self, pid: u32, created_at_100ns: u64) -> Result<(), PortError> {
        // The core refuses these too; this is the last line of defence for the ones that can
        // never be right.
        if pid <= SYSTEM_PID || pid == std::process::id() {
            return Err(PortError::InvalidInput(format!(
                "process {pid} is never ended by Volt"
            )));
        }
        // SAFETY: plain call; the handle is wrapped at once so it is closed on every path.
        let handle = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                pid,
            )
        }
        .map(Handle)
        .map_err(|e| terminate_error(win32_code(&e), pid))?;
        // The id may have been reused since the listing: only the process that started when the
        // listing said is ended. A handle keeps its process, so the check cannot go stale.
        match created_at(&handle) {
            Some(created) if created == created_at_100ns => {}
            Some(_) => return Err(PortError::NotFound(format!("process {pid}"))),
            None => {
                return Err(PortError::Other(
                    "the start time of the process could not be read".into(),
                ));
            }
        }
        // SAFETY: `handle.0` is a valid process handle with the terminate right.
        unsafe { TerminateProcess(handle.0, 1) }.map_err(|e| terminate_error(win32_code(&e), pid))
    }

    fn settings(&self, pid: u32, created_at_100ns: u64) -> Result<ProcessSettings, PortError> {
        let handle = open_for_tuning(pid, created_at_100ns)?;
        read_settings(&handle, pid)
    }

    fn apply_settings(
        &self,
        pid: u32,
        created_at_100ns: u64,
        change: &ProcessChange,
    ) -> Result<(), PortError> {
        let handle = open_for_tuning(pid, created_at_100ns)?;
        let before = read_settings(&handle, pid)?;
        if let Err(error) = write_settings(&handle, change) {
            // All or nothing: put back the fields that were already written.
            let _ = write_settings(&handle, &change.restoring(&before));
            return Err(error);
        }
        Ok(())
    }
}

/// Opens `pid` with the right to read and change its settings, and checks that it is the process
/// that started at `created_at_100ns`.
fn open_for_tuning(pid: u32, created_at_100ns: u64) -> Result<Handle, PortError> {
    // The kernel's own processes are never tuned (the core refuses them too).
    if pid <= SYSTEM_PID || pid == std::process::id() {
        return Err(PortError::InvalidInput(format!(
            "process {pid} is never changed by Volt"
        )));
    }
    // SAFETY: plain call; the handle is wrapped at once so it is closed on every path.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION,
            false,
            pid,
        )
    }
    .map(Handle)
    .map_err(|e| tuning_error(win32_code(&e), pid))?;
    match created_at(&handle) {
        Some(created) if created == created_at_100ns => Ok(handle),
        Some(_) => Err(PortError::NotFound(format!("process {pid}"))),
        None => Err(PortError::Other(
            "the start time of the process could not be read".into(),
        )),
    }
}

/// Bytes of a `PROCESS_POWER_THROTTLING_STATE` (three 32-bit fields), as the calls want them.
const THROTTLING_STATE_SIZE: u32 = 12;
const _: () =
    assert!(size_of::<PROCESS_POWER_THROTTLING_STATE>() == THROTTLING_STATE_SIZE as usize);

fn priority_flags(class: PriorityClass) -> PROCESS_CREATION_FLAGS {
    match class {
        PriorityClass::Idle => IDLE_PRIORITY_CLASS,
        PriorityClass::BelowNormal => BELOW_NORMAL_PRIORITY_CLASS,
        PriorityClass::Normal => NORMAL_PRIORITY_CLASS,
        PriorityClass::AboveNormal => ABOVE_NORMAL_PRIORITY_CLASS,
        PriorityClass::High => HIGH_PRIORITY_CLASS,
    }
}

/// The class behind a `GetPriorityClass` value; `None` for one Volt does not offer (Realtime).
fn priority_of(flags: u32) -> Option<PriorityClass> {
    PriorityClass::OFFERED
        .into_iter()
        .find(|class| priority_flags(*class).0 == flags)
}

fn read_priority(handle: &Handle, pid: u32) -> Result<PriorityClass, PortError> {
    // SAFETY: `handle.0` is a valid process handle with the query right.
    let flags = unsafe { GetPriorityClass(handle.0) };
    if flags == 0 {
        return Err(from_win32(
            win32_code(&windows::core::Error::from_thread()),
            "reading a priority class",
        ));
    }
    priority_of(flags).ok_or_else(|| {
        PortError::Unsupported(format!(
            "process {pid} has a priority class Volt does not offer"
        ))
    })
}

fn read_affinity(handle: &Handle) -> Result<(u64, u64), PortError> {
    let (mut process, mut system) = (0usize, 0usize);
    // SAFETY: `handle.0` is a valid process handle with the query right and both out pointers
    // are valid for the call.
    unsafe { GetProcessAffinityMask(handle.0, &raw mut process, &raw mut system) }
        .map_err(|e| from_win32(win32_code(&e), "reading an affinity"))?;
    Ok((process as u64, system as u64))
}

fn read_throttling(handle: &Handle) -> Result<PowerThrottling, PortError> {
    // Windows wants the version in the structure it fills.
    let mut state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ..PROCESS_POWER_THROTTLING_STATE::default()
    };
    // SAFETY: `state` is a writable `PROCESS_POWER_THROTTLING_STATE` of the size passed, valid
    // for the call.
    unsafe {
        GetProcessInformation(
            handle.0,
            ProcessPowerThrottling,
            (&raw mut state).cast::<c_void>(),
            THROTTLING_STATE_SIZE,
        )
    }
    .map_err(|e| from_win32(win32_code(&e), "reading power throttling"))?;
    Ok(
        if state.ControlMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED == 0 {
            PowerThrottling::Default
        } else if state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0 {
            PowerThrottling::On
        } else {
            PowerThrottling::Off
        },
    )
}

fn write_throttling(handle: &Handle, throttling: PowerThrottling) -> Result<(), PortError> {
    // `Default` hands the decision back to Windows: the bit is no longer controlled.
    let (control, state) = match throttling {
        PowerThrottling::Default => (0, 0),
        PowerThrottling::On => (
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        ),
        PowerThrottling::Off => (PROCESS_POWER_THROTTLING_EXECUTION_SPEED, 0),
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control,
        StateMask: state,
    };
    // SAFETY: `state` is a valid `PROCESS_POWER_THROTTLING_STATE` of the size passed, and the
    // handle has the set-information right.
    unsafe {
        SetProcessInformation(
            handle.0,
            ProcessPowerThrottling,
            (&raw const state).cast::<c_void>(),
            THROTTLING_STATE_SIZE,
        )
    }
    .map_err(|e| from_win32(win32_code(&e), "setting power throttling"))
}

fn read_settings(handle: &Handle, pid: u32) -> Result<ProcessSettings, PortError> {
    let (affinity_mask, system_affinity_mask) = read_affinity(handle)?;
    Ok(ProcessSettings {
        priority: read_priority(handle, pid)?,
        affinity_mask,
        system_affinity_mask,
        throttling: read_throttling(handle)?,
    })
}

/// Writes the fields of `change`, in the order priority, affinity, throttling.
fn write_settings(handle: &Handle, change: &ProcessChange) -> Result<(), PortError> {
    if let Some(class) = change.priority {
        // SAFETY: `handle.0` is a valid process handle with the set-information right.
        unsafe { SetPriorityClass(handle.0, priority_flags(class)) }
            .map_err(|e| from_win32(win32_code(&e), "setting a priority class"))?;
    }
    if let Some(mask) = change.affinity_mask {
        let mask = usize::try_from(mask)
            .map_err(|_| PortError::InvalidInput("the affinity is not valid".into()))?;
        // SAFETY: `handle.0` is a valid process handle with the set-information right.
        unsafe { SetProcessAffinityMask(handle.0, mask) }
            .map_err(|e| from_win32(win32_code(&e), "setting an affinity"))?;
    }
    if let Some(throttling) = change.throttling {
        write_throttling(handle, throttling)?;
    }
    Ok(())
}

/// When the process behind `handle` started, as a file time in 100 ns ticks.
fn created_at(handle: &Handle) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: `handle.0` is a valid process handle with the query right and the four out pointers
    // are valid for the call.
    unsafe {
        GetProcessTimes(
            handle.0,
            &raw mut created,
            &raw mut exited,
            &raw mut kernel,
            &raw mut user,
        )
    }
    .ok()?;
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// The executable path of process `pid`, when a limited handle can be opened to it and the process
/// is still the one that started at `created_at_100ns` (else the id was reused and `None`).
fn image_path(pid: u32, created_at_100ns: u64) -> Option<String> {
    // SAFETY: plain call; the handle is wrapped at once so it is closed on every path.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(Handle)
        .ok()?;
    if created_at(&handle)? != created_at_100ns {
        return None;
    }
    let mut buffer = vec![0u16; 1024];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `length` writable UTF-16 units and both pointers are valid for the
    // call; on success `length` is the number of units written.
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut length,
        )
    }
    .ok()?;
    let written = buffer.get(..usize::try_from(length).ok()?)?;
    Some(String::from_utf16_lossy(written)).filter(|path| !path.is_empty())
}

/// The typed error of a failed open or terminate: an unknown id is not found, a refusal is
/// access denied (the process is protected or belongs to another user).
fn terminate_error(code: u32, pid: u32) -> PortError {
    if code == ERROR_INVALID_PARAMETER.0 {
        PortError::NotFound(format!("process {pid}"))
    } else if code == ERROR_ACCESS_DENIED.0 {
        PortError::AccessDenied(format!("ending process {pid}"))
    } else {
        from_win32(code, "ending a process")
    }
}

/// The typed error of a failed open for tuning, as for [`terminate_error`].
fn tuning_error(code: u32, pid: u32) -> PortError {
    if code == ERROR_INVALID_PARAMETER.0 {
        PortError::NotFound(format!("process {pid}"))
    } else if code == ERROR_ACCESS_DENIED.0 {
        PortError::AccessDenied(format!("changing process {pid}"))
    } else {
        from_win32(code, "changing a process")
    }
}

/// The Win32 code inside a `windows` error (its HRESULT's low word).
fn win32_code(error: &windows::core::Error) -> u32 {
    // `HRESULT_FROM_WIN32` keeps the code in the low 16 bits.
    u32::try_from(error.code().0 & 0xFFFF).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn a_hand_built_buffer_is_read_field_by_field() {
        // Two records: the idle process (left out) and one named "a.exe" whose name sits behind
        // the records in the same buffer.
        let mut buf = vec![0u8; 500];
        let base = 0x1000usize;
        let put = |buf: &mut [u8], at: usize, value: u64| {
            buf[at..at + 8].copy_from_slice(&value.to_le_bytes());
        };
        // Record 0: idle, next record at 200.
        buf[0..4].copy_from_slice(&200u32.to_le_bytes());
        // Record 1 at 200: no next record.
        let r = 200;
        put(&mut buf, r + OFFSET_PRIVATE_WORKING_SET, 4096);
        put(&mut buf, r + OFFSET_CREATE_TIME, 777);
        buf[r + OFFSET_SESSION_ID..r + OFFSET_SESSION_ID + 4].copy_from_slice(&3u32.to_le_bytes());
        put(&mut buf, r + OFFSET_USER_TIME, 30);
        put(&mut buf, r + OFFSET_KERNEL_TIME, 12);
        buf[r + OFFSET_IMAGE_NAME_LENGTH..r + OFFSET_IMAGE_NAME_LENGTH + 2]
            .copy_from_slice(&10u16.to_le_bytes());
        put(&mut buf, r + OFFSET_IMAGE_NAME_BUFFER, (base + 480) as u64);
        put(&mut buf, r + OFFSET_PID, 77);
        put(&mut buf, r + OFFSET_PARENT_PID, 5);
        for (i, unit) in "a.exe".encode_utf16().enumerate() {
            buf[480 + i * 2..482 + i * 2].copy_from_slice(&unit.to_le_bytes());
        }
        let list = parse_records(&buf, base);
        assert_eq!(
            list,
            [ProcessInfo {
                pid: 77,
                parent_pid: 5,
                name: "a.exe".into(),
                working_set_bytes: 4096,
                cpu_time_100ns: 42,
                created_at_100ns: 777,
                path: None,
                session_id: 3,
            }]
        );
        // A truncated buffer ends the walk instead of reading past it.
        assert!(parse_records(&buf[..100], base).is_empty());
    }

    #[test]
    fn the_list_has_this_test_process_with_memory_and_cpu_time() {
        let me = std::process::id();
        let list = WinProcesses::new().list().expect("list processes");
        assert!(list.iter().all(|p| p.pid != IDLE_PID));
        let mine = list.iter().find(|p| p.pid == me).expect("this process");
        assert!(mine.name.to_lowercase().contains("volt_win"));
        // Between 1 MiB and 64 GiB: proves the field offset, not an exact figure.
        assert!((1 << 20..64 << 30).contains(&mine.working_set_bytes));
        assert!(mine.cpu_time_100ns > 0);
        assert!(mine.created_at_100ns > 0);
        assert!(
            mine.path
                .as_deref()
                .is_some_and(|p| p.to_lowercase().contains("volt_win"))
        );
        assert_ne!(mine.session_id, 0, "a test run is a user's, not session 0");
        assert!(
            list.iter()
                .any(|p| p.pid == SYSTEM_PID && p.name == "System" && p.session_id == 0)
        );
    }

    /// Starts a child that only waits, so ending it harms nothing else.
    fn sleeper() -> std::process::Child {
        Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start the test's own child")
    }

    #[test]
    fn a_child_the_test_started_can_be_ended_and_nothing_else() {
        let mut child = sleeper();
        let pid = child.id();
        let processes = WinProcesses::new();
        let listed = processes.list().expect("list");
        let entry = listed.iter().find(|p| p.pid == pid).expect("the child");
        assert_eq!(entry.parent_pid, std::process::id());
        assert!(entry.name.eq_ignore_ascii_case("ping.exe"));
        let path = entry.path.clone().expect("the child's path is readable");
        assert!(path.to_lowercase().ends_with("ping.exe"), "{path}");
        let created = entry.created_at_100ns;
        assert!(created > 0);

        // A listing that is stale (the id now belongs to another process) ends nothing.
        assert!(matches!(
            processes.terminate(pid, created + 1),
            Err(PortError::NotFound(_))
        ));
        assert!(child.try_wait().expect("poll").is_none());

        processes
            .terminate(pid, created)
            .expect("end the test's own child");
        let status = child.wait().expect("wait");
        assert_eq!(status.code(), Some(1));
        assert!(!processes.list().expect("list").iter().any(|p| p.pid == pid));
    }

    #[test]
    fn a_child_the_test_started_can_be_tuned_and_put_back() {
        let mut child = sleeper();
        let pid = child.id();
        let processes = WinProcesses::new();
        let created = processes
            .list()
            .expect("list")
            .iter()
            .find(|p| p.pid == pid)
            .expect("the child")
            .created_at_100ns;

        let before = processes.settings(pid, created).expect("read");
        assert_eq!(before.priority, PriorityClass::Normal);
        assert_ne!(before.affinity_mask, 0);
        assert_eq!(before.affinity_mask & !before.system_affinity_mask, 0);
        assert_eq!(before.throttling, PowerThrottling::Default);

        // Efficiency mode, as the core asks for it: the lowest class and forced throttling.
        let efficiency = ProcessChange {
            priority: Some(PriorityClass::Idle),
            affinity_mask: None,
            throttling: Some(PowerThrottling::On),
        };
        processes
            .apply_settings(pid, created, &efficiency)
            .expect("efficiency");
        let tuned = processes.settings(pid, created).expect("read");
        assert_eq!(tuned.priority, PriorityClass::Idle);
        assert_eq!(tuned.throttling, PowerThrottling::On);
        assert_eq!(tuned.affinity_mask, before.affinity_mask);

        // One core: the lowest bit that the system offers.
        let one_core = before.system_affinity_mask & before.system_affinity_mask.wrapping_neg();
        processes
            .apply_settings(
                pid,
                created,
                &ProcessChange {
                    priority: Some(PriorityClass::AboveNormal),
                    affinity_mask: Some(one_core),
                    throttling: None,
                },
            )
            .expect("one core");
        let narrow = processes.settings(pid, created).expect("read");
        assert_eq!(
            (narrow.priority, narrow.affinity_mask),
            (PriorityClass::AboveNormal, one_core)
        );
        assert_eq!(
            narrow.throttling,
            PowerThrottling::On,
            "not asked for, so left alone"
        );

        // An empty affinity is refused by Windows, and nothing is half-changed.
        let refused = processes.apply_settings(
            pid,
            created,
            &ProcessChange {
                priority: Some(PriorityClass::High),
                affinity_mask: Some(0),
                throttling: None,
            },
        );
        assert!(refused.is_err());
        assert_eq!(processes.settings(pid, created).expect("read"), narrow);

        // Putting back what was read restores everything.
        let restore = ProcessChange {
            priority: Some(before.priority),
            affinity_mask: Some(before.affinity_mask),
            throttling: Some(before.throttling),
        };
        processes
            .apply_settings(pid, created, &restore)
            .expect("restore");
        assert_eq!(processes.settings(pid, created).expect("read"), before);

        // A stale listing (another creation time) touches nothing.
        assert!(matches!(
            processes.apply_settings(pid, created + 1, &efficiency),
            Err(PortError::NotFound(_))
        ));
        assert_eq!(processes.settings(pid, created).expect("read"), before);

        child.kill().expect("end the test's own child");
        child.wait().expect("wait");
        // An id that no process has is not found.
        assert!(matches!(
            processes.settings(0x7FFF_FFF0, 0),
            Err(PortError::NotFound(_))
        ));
    }

    #[test]
    fn the_adapter_refuses_the_kernel_and_itself_for_tuning_too() {
        let processes = WinProcesses::new();
        for pid in [0, 4, std::process::id()] {
            assert!(matches!(
                processes.settings(pid, 0),
                Err(PortError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn priority_flags_round_trip_and_realtime_is_not_offered() {
        for class in PriorityClass::OFFERED {
            assert_eq!(priority_of(priority_flags(class).0), Some(class));
        }
        assert_eq!(priority_of(0x100), None);
    }

    #[test]
    fn the_adapter_refuses_the_kernel_and_itself() {
        let processes = WinProcesses::new();
        for pid in [0, 4, std::process::id()] {
            assert!(matches!(
                processes.terminate(pid, 0),
                Err(PortError::InvalidInput(_))
            ));
        }
    }

    #[test]
    fn win32_codes_become_typed_errors() {
        assert!(matches!(
            terminate_error(ERROR_INVALID_PARAMETER.0, 9),
            PortError::NotFound(_)
        ));
        assert!(matches!(
            terminate_error(ERROR_ACCESS_DENIED.0, 9),
            PortError::AccessDenied(_)
        ));
        assert!(matches!(terminate_error(1, 9), PortError::Other(_)));
    }
}
