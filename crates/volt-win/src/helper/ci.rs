//! Measurements and launch calls for the elevated CI job. Compiled only with the `elevated-ci`
//! cargo feature, which no product build enables.
//!
//! The job runs the real helper on a hosted Windows runner (a throwaway virtual machine whose user
//! holds an administrator token). Its harness (`volt-elevated-ci` in the `volt-helper` crate) uses
//! the real client path, [`WinElevator`](super::elevator::WinElevator), and this module adds what a
//! test needs and the product does not: the facts of a live connection
//! ([`WinElevator::ci_evidence`](super::elevator::WinElevator::ci_evidence) builds on it), a launch
//! call that takes the raw words (so a bad command line can be sent for real) and a wait for the
//! helper's exit code.
//!
//! Nothing here runs on a developer machine: the harness refuses to start outside a hosted runner.

use std::ffi::c_void;
use std::path::Path;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    WaitForSingleObject,
};

use super::failed;
use super::identity::ProcessHandle;
use super::launch::{Launcher, ShellLauncher};

/// What the process handle that the launch call returned can do, probed for real.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandleRights {
    /// The access mask the handle holds (`NtQueryObject`, basic information), when readable.
    pub granted_access: Option<u32>,
    /// A zero-timeout wait did not fail: the handle carries the synchronise right.
    pub can_wait: bool,
    /// `GetProcessId` worked, with this id.
    pub process_id: Option<u32>,
    /// `GetExitCodeProcess` worked: the handle carries a query right.
    pub can_query_exit_code: bool,
}

/// Memory of a process, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingSet {
    /// Current working set.
    pub working_set: u64,
    /// Peak working set.
    pub peak_working_set: u64,
    /// Private bytes (the commit charge of the process).
    pub private_bytes: u64,
}

unsafe extern "system" {
    /// `NtQueryObject`; class 0 is `ObjectBasicInformation`.
    fn NtQueryObject(
        handle: HANDLE,
        class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

/// Probes what `process` (the handle the launch call returned) really allows.
#[must_use]
pub fn handle_rights(process: &ProcessHandle) -> HandleRights {
    let handle = process.raw();
    // PUBLIC_OBJECT_BASIC_INFORMATION: Attributes, GrantedAccess, HandleCount, PointerCount and ten
    // reserved words, 56 bytes of u32.
    let mut basic = [0_u32; 14];
    let mut returned = 0_u32;
    // SAFETY: a valid handle, a writable buffer of exactly the size passed, a valid out pointer.
    let status = unsafe {
        NtQueryObject(
            handle,
            0,
            basic.as_mut_ptr().cast::<c_void>(),
            u32::try_from(std::mem::size_of_val(&basic)).unwrap_or(0),
            &raw mut returned,
        )
    };
    // SAFETY: a valid handle; a zero timeout only polls.
    let wait = unsafe { WaitForSingleObject(handle, 0) };
    let mut code = 0_u32;
    // SAFETY: a valid handle and a valid out pointer.
    let can_query_exit_code = unsafe { GetExitCodeProcess(handle, &raw mut code) }.is_ok();
    HandleRights {
        granted_access: (status >= 0).then_some(basic[1]),
        can_wait: wait != WAIT_FAILED,
        process_id: process.pid().ok(),
        can_query_exit_code,
    }
}

/// The working set of `process`.
///
/// # Errors
/// A message when Windows refuses (the handle lacks the query right).
pub fn working_set(process: &ProcessHandle) -> Result<WorkingSet, String> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
    counters.cb = size;
    // SAFETY: a valid handle and a counters structure of the size passed (the extended structure
    // starts with the plain one, which is what the call takes a pointer to).
    unsafe { GetProcessMemoryInfo(process.raw(), (&raw mut counters).cast(), size) }
        .map_err(|e| failed("GetProcessMemoryInfo", &e))?;
    Ok(WorkingSet {
        working_set: counters.WorkingSetSize as u64,
        peak_working_set: counters.PeakWorkingSetSize as u64,
        private_bytes: counters.PrivateUsage as u64,
    })
}

/// A handle to process `pid` that can be waited on and queried (for the helper's exit after the
/// connection that held the launch handle is gone).
///
/// # Errors
/// A message when the process cannot be opened.
pub fn open_waitable(pid: u32) -> Result<ProcessHandle, String> {
    // SAFETY: plain OpenProcess; the handle is wrapped and closed on drop.
    let handle = unsafe {
        OpenProcess(
            PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|e| failed(&format!("OpenProcess {pid}"), &e))?;
    ProcessHandle::from_launch(handle).ok_or_else(|| "no handle".to_owned())
}

/// Starts `image` through the real launch call (`ShellExecuteExW`, verb `runas`) with `args`
/// exactly as given, without any argument rendering: a malformed command line can be sent.
///
/// # Errors
/// A message when Windows does not start the process.
pub fn launch(image: &Path, args: &[String]) -> Result<ProcessHandle, String> {
    ShellLauncher
        .launch(image, args)
        .map_err(|error| format!("{error:?}"))
}

/// Waits up to `timeout` for the process to exit and returns its exit code, or `None` when it is
/// still running (or neither a wait nor a query is possible).
#[must_use]
pub fn wait_exit_code(process: &ProcessHandle, timeout: Duration) -> Option<u32> {
    let handle = process.raw();
    let millis = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: a valid handle; the wait is bounded.
    let wait = unsafe { WaitForSingleObject(handle, millis) };
    if wait == WAIT_FAILED {
        // No synchronise right: poll the exit code instead.
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Some(code) = exit_code(handle) {
                return Some(code);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        return exit_code(handle);
    }
    if wait != WAIT_OBJECT_0 {
        return None;
    }
    exit_code(handle)
}

/// `STILL_ACTIVE`.
const STILL_ACTIVE: u32 = 259;

fn exit_code(handle: HANDLE) -> Option<u32> {
    let mut code = 0_u32;
    // SAFETY: a valid handle and a valid out pointer.
    unsafe { GetExitCodeProcess(handle, &raw mut code) }
        .ok()
        .filter(|()| code != STILL_ACTIVE)
        .map(|()| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rights_and_the_memory_of_this_process_can_be_read() {
        // Reads only, about this very test process.
        let me = ProcessHandle::open(std::process::id()).expect("own handle");
        let rights = handle_rights(&me);
        assert_eq!(rights.process_id, Some(std::process::id()));
        assert!(rights.can_query_exit_code);
        // A handle opened for limited queries carries no synchronise right.
        assert!(!rights.can_wait);
        let mask = rights.granted_access.expect("the mask is readable");
        assert_ne!(mask & PROCESS_QUERY_LIMITED_INFORMATION.0, 0, "{mask:#x}");
        let memory = working_set(&me).expect("memory");
        assert!(memory.working_set > 0);
        assert!(memory.peak_working_set >= memory.working_set);
    }

    #[test]
    fn a_waitable_handle_to_a_running_process_times_out_with_no_exit_code() {
        let me = open_waitable(std::process::id()).expect("waitable handle");
        assert!(handle_rights(&me).can_wait);
        assert_eq!(wait_exit_code(&me, Duration::from_millis(50)), None);
    }

    #[test]
    fn an_unrenderable_command_line_word_is_refused_by_the_launch_call() {
        // Never reaches Windows: the helper's own parameter rule refuses a word with a space.
        let image = Path::new(r"C:\Program Files\Volt\volt-helper.exe");
        assert!(launch(image, &["two words".to_owned()]).is_err());
    }
}
