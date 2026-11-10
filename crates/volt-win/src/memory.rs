//! [`Memory`] port on the Windows API: memory by list, and the five purges.
//!
//! - **Reading** the lists needs no rights: `NtQuerySystemInformation(SystemMemoryListInformation)`.
//! - **Trimming working sets** needs no administrator rights. It opens every process the user may
//!   open with `PROCESS_SET_QUOTA` and calls `SetProcessWorkingSetSize(-1, -1)` on it. Processes
//!   that cannot be opened (protected, other users', system) are counted and left alone.
//! - **The three list purges and the modified-list flush** are `NtSetSystemInformation` commands
//!   that need `SeProfileSingleProcessPrivilege`, which only an elevated token holds.
//! - **The file cache trim** is `SetSystemFileCacheSize(-1, -1)`, which needs
//!   `SeIncreaseQuotaPrivilege`, also held only by an elevated token.
//!
//! A missing privilege is [`PortError::AccessDenied`] and is found out before anything is called
//! (enabling a privilege the token does not hold fails). A status that says the build does not
//! know the call is [`PortError::Unsupported`]. Nothing here panics.
//!
//! # Sources of the constants
//!
//! The class number, the structure and the command numbers are undocumented by Microsoft. They are
//! taken from the System Informer (formerly Process Hacker) `phnt` headers, `phnt/include/
//! ntexapi.h` in <https://github.com/winsiderss/systeminformer>:
//!
//! - `SystemMemoryListInformation` is class 80 (`// q: SYSTEM_MEMORY_LIST_INFORMATION; s:
//!   SYSTEM_MEMORY_LIST_COMMAND (requires SeProfileSingleProcessPrivilege) // 80`).
//! - `SYSTEM_MEMORY_LIST_INFORMATION` is `ZeroPageCount, FreePageCount, ModifiedPageCount,
//!   ModifiedNoWritePageCount, BadPageCount` (all `SIZE_T`), then `PageCountByPriority[8]`,
//!   `RepurposedPagesByPriority[8]`, then `ModifiedPageCountPageFile`.
//! - `SYSTEM_MEMORY_LIST_COMMAND` counts from 0: `MemoryCaptureAccessedBits`,
//!   `MemoryCaptureAndResetAccessedBits`, `MemoryEmptyWorkingSets` (2), `MemoryFlushModifiedList`
//!   (3), `MemoryPurgeStandbyList` (4), `MemoryPurgeLowPriorityStandbyList` (5).
//!
//! The NTSTATUS values, the privilege names and the token constants come from the `windows`
//! crate, which is generated from Microsoft's own metadata. `SetSystemFileCacheSize` and its
//! flush convention (`(SIZE_T)-1` for both sizes) and its `SE_INCREASE_QUOTA_NAME` requirement are
//! documented on Microsoft Learn (`memoryapi.h`).

use std::ffi::c_void;

use volt_core::{
    Availability, Memory, MemoryArea, MemoryLists, MemoryStatus, PortError, PurgeOutcome,
};
use windows::Wdk::System::SystemInformation::{NtQuerySystemInformation, SYSTEM_INFORMATION_CLASS};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_NOT_ALL_ASSIGNED, ERROR_PRIVILEGE_NOT_HELD, GetLastError, HANDLE, LUID,
    STATUS_ACCESS_DENIED, STATUS_INFO_LENGTH_MISMATCH, STATUS_INVALID_INFO_CLASS,
    STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED, STATUS_NOT_SUPPORTED,
    STATUS_PRIVILEGE_NOT_HELD,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_INCREASE_QUOTA_NAME,
    SE_PRIVILEGE_ENABLED, SE_PROF_SINGLE_PROCESS_NAME, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES,
};
use windows::Win32::System::Memory::SetSystemFileCacheSize;
use windows::Win32::System::ProcessStatus::EnumProcesses;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_SET_QUOTA, SetProcessWorkingSetSize,
};
use windows::core::PCWSTR;

use crate::metrics::read_memory;

/// `SystemMemoryListInformation` (phnt `ntexapi.h`).
const SYSTEM_MEMORY_LIST_INFORMATION_CLASS: i32 = 80;
/// `MemoryFlushModifiedList` (phnt `SYSTEM_MEMORY_LIST_COMMAND`).
const MEMORY_FLUSH_MODIFIED_LIST: u32 = 3;
/// `MemoryPurgeStandbyList` (phnt `SYSTEM_MEMORY_LIST_COMMAND`).
const MEMORY_PURGE_STANDBY_LIST: u32 = 4;
/// `MemoryPurgeLowPriorityStandbyList` (phnt `SYSTEM_MEMORY_LIST_COMMAND`).
const MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST: u32 = 5;

/// Windows on x64 and ARM64 manages memory in 4 KiB pages.
const PAGE_BYTES: u64 = 4096;

/// `SYSTEM_MEMORY_LIST_INFORMATION` (phnt `ntexapi.h`). Every field is a `SIZE_T`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
struct RawMemoryLists {
    zero_page_count: usize,
    free_page_count: usize,
    modified_page_count: usize,
    modified_no_write_page_count: usize,
    bad_page_count: usize,
    page_count_by_priority: [usize; 8],
    repurposed_pages_by_priority: [usize; 8],
    modified_page_count_page_file: usize,
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    /// `NtSetSystemInformation`: not in the `windows` crate. Returns an NTSTATUS.
    fn NtSetSystemInformation(class: i32, information: *mut c_void, information_length: u32)
    -> i32;
}

/// The memory port backed by the Windows API. It holds nothing: every call goes to the system.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinMemory;

impl WinMemory {
    /// The adapter.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Which privilege an area needs, or `None` when it needs none.
fn privilege_for(area: MemoryArea) -> Option<PCWSTR> {
    match area {
        MemoryArea::WorkingSets => None,
        MemoryArea::FileCache => Some(SE_INCREASE_QUOTA_NAME),
        // The list commands (and any area added later) are the kernel's memory list calls.
        _ => Some(SE_PROF_SINGLE_PROCESS_NAME),
    }
}

/// The `SYSTEM_MEMORY_LIST_COMMAND` for a list area.
fn list_command(area: MemoryArea) -> Option<u32> {
    match area {
        MemoryArea::ModifiedList => Some(MEMORY_FLUSH_MODIFIED_LIST),
        MemoryArea::StandbyList => Some(MEMORY_PURGE_STANDBY_LIST),
        MemoryArea::StandbyListLowPriority => Some(MEMORY_PURGE_LOW_PRIORITY_STANDBY_LIST),
        _ => None,
    }
}

/// Turns a failing NTSTATUS into the typed error the UI explains.
fn status_error(status: i32, what: &str) -> PortError {
    let code = status.cast_unsigned();
    if status == STATUS_PRIVILEGE_NOT_HELD.0 || status == STATUS_ACCESS_DENIED.0 {
        PortError::AccessDenied(format!("{what} needs administrator rights"))
    } else if [
        STATUS_INVALID_INFO_CLASS.0,
        STATUS_NOT_IMPLEMENTED.0,
        STATUS_NOT_SUPPORTED.0,
        STATUS_INFO_LENGTH_MISMATCH.0,
        STATUS_INVALID_PARAMETER.0,
    ]
    .contains(&status)
    {
        PortError::Unsupported(format!(
            "{what} is not supported by this Windows build (status {code:#010x})"
        ))
    } else {
        PortError::Other(format!("{what} failed (status {code:#010x})"))
    }
}

/// Closes a handle when dropped.
struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful open call, is owned by this value and
        // is closed exactly once, here. A failure leaves nothing more to do.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Enables a privilege in this process's own token. A token that does not hold it (a standard or
/// not elevated token) is [`PortError::AccessDenied`]. This changes only this process's token and
/// is what the call that follows needs anyway.
fn enable_privilege(name: PCWSTR) -> Result<(), PortError> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns the pseudo handle of this process, which needs no
    // closing; `token` is a valid out pointer that lives for the call.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &raw mut token) }
        .map_err(|e| PortError::Other(format!("OpenProcessToken: {e}")))?;
    let token = OwnedHandle(token);

    let mut luid = LUID::default();
    // SAFETY: `name` is a NUL-terminated constant string, the system name is null (this machine)
    // and `luid` is a valid out pointer that lives for the call.
    unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &raw mut luid) }
        .map_err(|e| PortError::Other(format!("LookupPrivilegeValueW: {e}")))?;

    let state = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `token.0` is the live token handle opened with `TOKEN_ADJUST_PRIVILEGES`, and
    // `state` is a valid one-privilege `TOKEN_PRIVILEGES` that lives for the call. No previous
    // state is requested.
    unsafe { AdjustTokenPrivileges(token.0, false, Some(&raw const state), 0, None, None) }
        .map_err(|e| PortError::Other(format!("AdjustTokenPrivileges: {e}")))?;
    // The call succeeds even when the token does not hold the privilege: it then sets
    // ERROR_NOT_ALL_ASSIGNED. Nothing else ran since, so the last error is still the call's.
    // SAFETY: reads this thread's last error; no preconditions.
    if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
        return Err(PortError::AccessDenied(
            "this process does not hold the privilege, which needs administrator rights".into(),
        ));
    }
    Ok(())
}

/// Reads the raw memory lists. Needs no privilege.
fn read_raw_lists() -> Result<RawMemoryLists, PortError> {
    let mut raw = RawMemoryLists::default();
    let length = u32::try_from(size_of::<RawMemoryLists>())
        .map_err(|_| PortError::Other("memory list structure too large".into()))?;
    let mut returned = 0u32;
    // SAFETY: `raw` is a writable `#[repr(C)]` structure of `length` bytes that matches phnt's
    // `SYSTEM_MEMORY_LIST_INFORMATION`, and `returned` is a valid out pointer; both live for the
    // call. The class only reads the lists, it changes nothing.
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_INFORMATION_CLASS(SYSTEM_MEMORY_LIST_INFORMATION_CLASS),
            (&raw mut raw).cast::<c_void>(),
            length,
            &raw mut returned,
        )
    };
    if status.0 < 0 {
        return Err(status_error(status.0, "reading the memory lists"));
    }
    Ok(raw)
}

/// Adds up the lists in a reading, in bytes. Pure.
fn lists_from_raw(total_bytes: u64, raw: &RawMemoryLists) -> MemoryLists {
    let pages = |count: usize| u64::try_from(count).unwrap_or(u64::MAX);
    let standby: u64 = raw
        .page_count_by_priority
        .iter()
        .map(|&count| pages(count))
        .fold(0, u64::saturating_add);
    MemoryLists::from_lists(
        total_bytes,
        standby.saturating_mul(PAGE_BYTES),
        pages(raw.modified_page_count)
            .saturating_add(pages(raw.modified_no_write_page_count))
            .saturating_mul(PAGE_BYTES),
        pages(raw.zero_page_count)
            .saturating_add(pages(raw.free_page_count))
            .saturating_mul(PAGE_BYTES),
    )
}

/// The ids of the running processes.
fn process_ids() -> Result<Vec<u32>, PortError> {
    let mut capacity = 1024usize;
    loop {
        let mut ids = vec![0u32; capacity];
        let bytes = u32::try_from(capacity * size_of::<u32>())
            .map_err(|_| PortError::Other("too many processes".into()))?;
        let mut needed = 0u32;
        // SAFETY: `ids` holds `bytes` writable bytes and `needed` is a valid out pointer; both
        // live for the call.
        unsafe { EnumProcesses(ids.as_mut_ptr(), bytes, &raw mut needed) }
            .map_err(|e| PortError::Other(format!("EnumProcesses: {e}")))?;
        if needed < bytes {
            let count = usize::try_from(needed).unwrap_or(0) / size_of::<u32>();
            ids.truncate(count);
            return Ok(ids);
        }
        // The buffer was full: there may be more processes. Try a larger one.
        capacity *= 2;
    }
}

/// Trims the working set of one process: opens it with `PROCESS_SET_QUOTA` only and asks Windows
/// to move its pages out of the working set (`SetProcessWorkingSetSize(-1, -1)`). The pages go to
/// the standby or modified list and come back on demand. A process that cannot be opened is an
/// error the caller counts and moves past.
pub(crate) fn trim_working_set(pid: u32) -> Result<(), PortError> {
    // SAFETY: opening a process by id has no preconditions; the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_SET_QUOTA, false, pid) }
        .map_err(|e| PortError::AccessDenied(format!("cannot open process {pid}: {e}")))?;
    let handle = OwnedHandle(handle);
    // SAFETY: `handle.0` is a live process handle with `PROCESS_SET_QUOTA`, which the call
    // requires. `usize::MAX` for both sizes is the documented "trim to empty" request.
    unsafe { SetProcessWorkingSetSize(handle.0, usize::MAX, usize::MAX) }
        .map_err(|e| PortError::Other(format!("cannot trim process {pid}: {e}")))
}

/// Trims every process the user can open.
fn trim_all_working_sets() -> Result<PurgeOutcome, PortError> {
    let mut outcome = PurgeOutcome::default();
    for pid in process_ids()? {
        // 0 is the idle pseudo process and 4 is the System process: neither can be opened.
        if pid == 0 || pid == 4 {
            continue;
        }
        match trim_working_set(pid) {
            Ok(()) => outcome.processes_trimmed += 1,
            Err(_) => outcome.processes_skipped += 1,
        }
    }
    Ok(outcome)
}

fn run_list_command(command: u32, what: &str) -> Result<(), PortError> {
    let mut command = command;
    // SAFETY: the information is a live, writable `u32` holding a `SYSTEM_MEMORY_LIST_COMMAND`
    // of exactly 4 bytes, as phnt documents for class 80. This is only reached after
    // `enable_privilege` succeeded, so the token holds `SeProfileSingleProcessPrivilege`.
    let status = unsafe {
        NtSetSystemInformation(
            SYSTEM_MEMORY_LIST_INFORMATION_CLASS,
            (&raw mut command).cast::<c_void>(),
            4,
        )
    };
    if status < 0 {
        return Err(status_error(status, what));
    }
    Ok(())
}

fn trim_file_cache() -> Result<(), PortError> {
    // SAFETY: no pointers. `(SIZE_T)-1` for both sizes is the documented flush request and the
    // flags leave the cache limits unchanged. Only reached after `enable_privilege` succeeded
    // for `SE_INCREASE_QUOTA_NAME`.
    unsafe { SetSystemFileCacheSize(usize::MAX, usize::MAX, 0) }.map_err(|e| {
        if e.code() == ERROR_PRIVILEGE_NOT_HELD.to_hresult() {
            PortError::AccessDenied("trimming the file cache needs administrator rights".into())
        } else {
            PortError::Other(format!("SetSystemFileCacheSize: {e}"))
        }
    })
}

impl Memory for WinMemory {
    fn status(&self) -> Result<MemoryStatus, PortError> {
        read_memory()
    }

    fn lists(&self) -> Result<MemoryLists, PortError> {
        let total = read_memory()?.total_physical_bytes;
        Ok(lists_from_raw(total, &read_raw_lists()?))
    }

    fn availability(&self, area: MemoryArea) -> Availability {
        match privilege_for(area) {
            None => Availability::Available,
            Some(name) => match enable_privilege(name) {
                Ok(()) => Availability::Available,
                Err(PortError::AccessDenied(_)) => Availability::NeedsAdministrator,
                Err(_) => Availability::Unsupported,
            },
        }
    }

    fn purge(&self, area: MemoryArea) -> Result<PurgeOutcome, PortError> {
        if let Some(name) = privilege_for(area) {
            enable_privilege(name)?;
        }
        match area {
            MemoryArea::WorkingSets => trim_all_working_sets(),
            MemoryArea::FileCache => trim_file_cache().map(|()| PurgeOutcome::default()),
            other => match list_command(other) {
                Some(command) => {
                    run_list_command(command, other.name()).map(|()| PurgeOutcome::default())
                }
                None => Err(PortError::Unsupported(format!(
                    "cleaning the {} is not supported",
                    other.name()
                ))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Threading::GetCurrentProcessId;

    use super::*;

    // SAFETY note for the whole module: these tests only READ memory information and trim the
    // working set of the TEST PROCESS ITSELF. They never trim another process and never purge or
    // flush a list or the file cache.

    #[test]
    fn the_structure_has_the_size_phnt_documents() {
        // 5 counters, two arrays of 8 and one more counter, all pointer-sized.
        assert_eq!(
            size_of::<RawMemoryLists>(),
            (5 + 8 + 8 + 1) * size_of::<usize>()
        );
    }

    #[test]
    fn the_command_numbers_match_the_reference() {
        assert_eq!(list_command(MemoryArea::ModifiedList), Some(3));
        assert_eq!(list_command(MemoryArea::StandbyList), Some(4));
        assert_eq!(list_command(MemoryArea::StandbyListLowPriority), Some(5));
        assert_eq!(list_command(MemoryArea::WorkingSets), None);
        assert_eq!(list_command(MemoryArea::FileCache), None);
    }

    #[test]
    fn statuses_map_to_typed_errors() {
        assert!(matches!(
            status_error(STATUS_PRIVILEGE_NOT_HELD.0, "x"),
            PortError::AccessDenied(_)
        ));
        assert!(matches!(
            status_error(STATUS_ACCESS_DENIED.0, "x"),
            PortError::AccessDenied(_)
        ));
        for unsupported in [
            STATUS_INVALID_INFO_CLASS.0,
            STATUS_NOT_IMPLEMENTED.0,
            STATUS_NOT_SUPPORTED.0,
            STATUS_INFO_LENGTH_MISMATCH.0,
            STATUS_INVALID_PARAMETER.0,
        ] {
            assert!(matches!(
                status_error(unsupported, "x"),
                PortError::Unsupported(_)
            ));
        }
        assert!(matches!(status_error(-1, "x"), PortError::Other(_)));
    }

    #[test]
    fn the_raw_lists_become_bytes_with_in_use_as_the_remainder() {
        let raw = RawMemoryLists {
            zero_page_count: 10,
            free_page_count: 20,
            modified_page_count: 5,
            modified_no_write_page_count: 1,
            page_count_by_priority: [1, 2, 3, 4, 5, 6, 7, 8],
            ..RawMemoryLists::default()
        };
        let lists = lists_from_raw(100 * PAGE_BYTES, &raw);
        assert_eq!(lists.free_bytes, 30 * PAGE_BYTES);
        assert_eq!(lists.modified_bytes, 6 * PAGE_BYTES);
        assert_eq!(lists.standby_bytes, 36 * PAGE_BYTES);
        assert_eq!(lists.in_use_bytes, 28 * PAGE_BYTES);
    }

    #[test]
    fn the_real_lists_can_be_read_and_agree_with_the_available_memory() {
        let adapter = WinMemory::new();
        let lists = adapter.lists().expect("memory lists are readable");
        let status = adapter.status().expect("status");
        assert_eq!(lists.total_bytes, status.total_physical_bytes);
        assert!(lists.free_bytes + lists.standby_bytes + lists.modified_bytes <= lists.total_bytes);
        assert!(lists.in_use_bytes > 0);
        // Windows calls free plus standby "available". The lists move while we read, so allow a
        // generous gap.
        let listed = lists.free_bytes + lists.standby_bytes;
        let gap = listed.abs_diff(status.available_physical_bytes);
        assert!(
            gap < lists.total_bytes / 6,
            "gap {gap} of {}",
            lists.total_bytes
        );
    }

    #[test]
    fn the_test_process_can_trim_its_own_working_set() {
        // SAFETY: reads this process's id; no preconditions.
        let own = unsafe { GetCurrentProcessId() };
        trim_working_set(own).expect("trim the test process itself");
    }

    #[test]
    fn a_process_that_does_not_exist_is_skipped_not_a_crash() {
        assert!(trim_working_set(u32::MAX - 1).is_err());
    }

    #[test]
    fn working_sets_need_no_privilege_and_the_others_report_what_the_token_holds() {
        let adapter = WinMemory::new();
        assert_eq!(
            adapter.availability(MemoryArea::WorkingSets),
            Availability::Available
        );
        // Whether the others are available depends on whether the test runs elevated; either
        // answer is valid, and asking only reads the token (and enables a privilege in it).
        for area in [MemoryArea::StandbyList, MemoryArea::FileCache] {
            assert!(matches!(
                adapter.availability(area),
                Availability::Available | Availability::NeedsAdministrator
            ));
        }
    }
}
