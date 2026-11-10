//! What the helper does to itself first thing at start, before it reads its arguments or touches
//! anything else.
//!
//! The helper is a small, elevated program that must not be steerable by the user-controlled
//! state around it. [`harden_process`] therefore
//!
//! 1. scrubs the environment (every variable except the three Windows itself needs is removed, so
//!    a planted `PATH`, `TEMP` or loader variable has no effect);
//! 2. restricts the DLL search to the System32 folder (`SetDefaultDllDirectories`);
//! 3. moves the working directory to System32, so nothing resolves against the folder the helper
//!    was started from;
//! 4. applies process mitigation policies: no dynamic code, no extension points (legacy hooks), no
//!    image loads from remote or Low-integrity locations, System32 preferred for images, strict
//!    handle checking, and no child processes;
//! 5. removes every privilege from its token except the three the memory purges use.
//!
//! Each step fails closed: if one cannot be applied, [`harden_process`] returns the error and the
//! helper exits. The steps are separate functions so the rules (what is kept, what is removed) are
//! pure and tested; applying them to the test process itself would break the test harness, so the
//! real run is checked in a child process by the `volt-helper` tests.

use std::collections::HashSet;
use std::ffi::c_void;

use windows::Win32::Foundation::{ERROR_NOT_ALL_ASSIGNED, GetLastError, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeNameW,
    SE_PRIVILEGE_REMOVED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY, TokenPrivileges,
};
use windows::Win32::System::Environment::SetCurrentDirectoryW;
use windows::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcessToken, PROCESS_MITIGATION_POLICY, ProcessChildProcessPolicy,
    ProcessDynamicCodePolicy, ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy,
    ProcessStrictHandleCheckPolicy, SetProcessMitigationPolicy,
};
use windows::core::{PCWSTR, PWSTR};

use super::{OwnedHandle, failed, wide};

/// The privileges the helper keeps: the two the memory purges need and the one every process
/// holds to walk the file system.
pub const KEPT_PRIVILEGES: [&str; 3] = [
    "SeProfileSingleProcessPrivilege",
    "SeIncreaseQuotaPrivilege",
    "SeChangeNotifyPrivilege",
];

/// The environment variables that survive the scrub. Windows components read these.
pub const KEPT_VARIABLES: [&str; 3] = ["SystemRoot", "windir", "SystemDrive"];

/// Which of `held` privileges must be removed: every one that is not in [`KEPT_PRIVILEGES`].
/// Names compare case-insensitively.
#[must_use]
pub fn privileges_to_remove(held: &[String]) -> Vec<String> {
    held.iter()
        .filter(|name| {
            !KEPT_PRIVILEGES
                .iter()
                .any(|kept| kept.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect()
}

/// Which of the environment variable `names` must be removed: every one that is not in
/// [`KEPT_VARIABLES`]. Names compare case-insensitively, as Windows does.
#[must_use]
pub fn variables_to_remove(names: &[String]) -> Vec<String> {
    names
        .iter()
        .filter(|name| {
            !KEPT_VARIABLES
                .iter()
                .any(|kept| kept.eq_ignore_ascii_case(name))
        })
        .cloned()
        .collect()
}

// The mitigation policy flags. Each policy structure is one 32-bit union of bit flags.
/// `PROCESS_MITIGATION_DYNAMIC_CODE_POLICY.ProhibitDynamicCode`.
const PROHIBIT_DYNAMIC_CODE: u32 = 0x1;
/// `PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY.DisableExtensionPoints`.
const DISABLE_EXTENSION_POINTS: u32 = 0x1;
/// `PROCESS_MITIGATION_IMAGE_LOAD_POLICY`: `NoRemoteImages`, `NoLowMandatoryLabelImages`,
/// `PreferSystem32Images`.
const IMAGE_LOAD: u32 = 0x1 | 0x2 | 0x4;
/// `PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY`: `RaiseExceptionOnInvalidHandleReference`,
/// `HandleExceptionsPermanentlyEnabled`.
const STRICT_HANDLE_CHECK: u32 = 0x1 | 0x2;
/// `PROCESS_MITIGATION_CHILD_PROCESS_POLICY.NoChildProcessCreation`.
const NO_CHILD_PROCESS: u32 = 0x1;

/// The policies [`harden_process`] applies, by name and flags.
const POLICIES: [(&str, PROCESS_MITIGATION_POLICY, u32); 5] = [
    (
        "dynamic code",
        ProcessDynamicCodePolicy,
        PROHIBIT_DYNAMIC_CODE,
    ),
    (
        "extension points",
        ProcessExtensionPointDisablePolicy,
        DISABLE_EXTENSION_POINTS,
    ),
    ("image load", ProcessImageLoadPolicy, IMAGE_LOAD),
    (
        "strict handle check",
        ProcessStrictHandleCheckPolicy,
        STRICT_HANDLE_CHECK,
    ),
    (
        "child processes",
        ProcessChildProcessPolicy,
        NO_CHILD_PROCESS,
    ),
];

/// What hardening left behind, for a self-test to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardenReport {
    /// Environment variables left.
    pub variables: Vec<String>,
    /// The working directory now.
    pub working_directory: String,
    /// The privileges the token still holds.
    pub privileges: Vec<String>,
}

/// Applies every hardening step to this process, in order. Call it first thing in `main`.
///
/// # Errors
/// The first step that fails, in words. The caller must exit: a process that is not hardened must
/// not serve.
pub fn harden_process() -> Result<HardenReport, String> {
    scrub_environment();
    restrict_dll_search()?;
    let working_directory = enter_system_directory()?;
    for (name, policy, flags) in POLICIES {
        set_mitigation(policy, flags).map_err(|error| format!("mitigation {name}: {error}"))?;
    }
    strip_privileges()?;
    Ok(HardenReport {
        variables: std::env::vars_os()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect(),
        working_directory,
        privileges: held_privileges()?,
    })
}

/// Removes every environment variable that is not in [`KEPT_VARIABLES`].
fn scrub_environment() {
    let names: Vec<String> = std::env::vars_os()
        .map(|(name, _)| name.to_string_lossy().into_owned())
        .collect();
    for name in variables_to_remove(&names) {
        // Windows allows a name that starts with `=` (drive working directories) and std refuses
        // to remove such a name; those are not loader input and are left alone.
        if name.is_empty() || name.starts_with('=') || name.contains('=') {
            continue;
        }
        // SAFETY: the helper calls this before it starts any thread, so no other thread reads or
        // writes the environment while it changes. (Rust 2024 marks removal unsafe for that
        // reason.)
        unsafe { std::env::remove_var(&name) };
    }
}

/// Limits DLL loading to System32 for everything that loads after this call.
fn restrict_dll_search() -> Result<(), String> {
    // SAFETY: no pointers; the flag is a documented constant.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|e| failed("SetDefaultDllDirectories", &e))
}

/// The System32 folder, from Windows (not from the environment).
fn system_directory() -> Result<String, String> {
    let mut buffer = vec![0_u16; 512];
    // SAFETY: `buffer` is valid for its length; the call writes at most that many characters.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(failed(
            "GetSystemDirectoryW",
            &windows::core::Error::from_thread(),
        ));
    }
    Ok(String::from_utf16_lossy(&buffer[..length]))
}

/// Makes System32 the working directory and returns it.
fn enter_system_directory() -> Result<String, String> {
    let directory = system_directory()?;
    let wide_directory = wide(&directory);
    // SAFETY: `wide_directory` is NUL-terminated and outlives the call.
    unsafe { SetCurrentDirectoryW(PCWSTR(wide_directory.as_ptr())) }
        .ok()
        .map_err(|e| failed("SetCurrentDirectoryW", &e))?;
    Ok(directory)
}

/// Applies one mitigation policy given as its 32-bit flags word.
fn set_mitigation(policy: PROCESS_MITIGATION_POLICY, flags: u32) -> Result<(), String> {
    // SAFETY: every policy used here is a structure that is a single 32-bit flags union, so a
    // pointer to a `u32` and a length of four bytes describe it exactly; `flags` outlives the call.
    unsafe {
        SetProcessMitigationPolicy(
            policy,
            (&raw const flags).cast::<c_void>(),
            size_of::<u32>(),
        )
    }
    .map_err(|e| e.to_string())
}

/// Opens this process's token.
fn own_token(access: windows::Win32::Security::TOKEN_ACCESS_MASK) -> Result<OwnedHandle, String> {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle of this process needs no closing; `token` is a valid out pointer.
    unsafe { OpenProcessToken(GetCurrentProcess(), access, &raw mut token) }
        .map_err(|e| failed("OpenProcessToken", &e))?;
    Ok(OwnedHandle(token))
}

/// The privileges in this process's token, with their LUIDs.
fn token_privileges(token: &OwnedHandle) -> Result<Vec<(String, LUID)>, String> {
    let mut needed = 0_u32;
    // SAFETY: a size query: a null buffer of length 0 and a valid out pointer. It fails with
    // ERROR_INSUFFICIENT_BUFFER by design.
    let _ = unsafe { GetTokenInformation(token.raw(), TokenPrivileges, None, 0, &raw mut needed) };
    if needed == 0 {
        return Err("GetTokenInformation: no size".to_owned());
    }
    // 8-byte words, so the buffer is aligned for TOKEN_PRIVILEGES.
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    // SAFETY: `buffer` is writable and at least `needed` bytes long; both live for the call.
    unsafe {
        GetTokenInformation(
            token.raw(),
            TokenPrivileges,
            Some(buffer.as_mut_ptr().cast::<c_void>()),
            needed,
            &raw mut needed,
        )
    }
    .map_err(|e| failed("GetTokenInformation", &e))?;
    let header = buffer.as_ptr().cast::<TOKEN_PRIVILEGES>();
    // SAFETY: the call filled `buffer` with a TOKEN_PRIVILEGES header followed by
    // `PrivilegeCount` entries inside the bytes it wrote; the pointer is 8-byte aligned and
    // `buffer` outlives every read below.
    let (count, first) = unsafe {
        (
            (*header).PrivilegeCount as usize,
            (&raw const (*header).Privileges).cast::<LUID_AND_ATTRIBUTES>(),
        )
    };
    let mut found = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: `index < PrivilegeCount`, so the entry lies inside the filled buffer.
        let luid = unsafe { first.add(index).read_unaligned() }.Luid;
        found.push((privilege_name(luid)?, luid));
    }
    Ok(found)
}

/// The name of a privilege from its LUID.
fn privilege_name(mut luid: LUID) -> Result<String, String> {
    let mut length = 0_u32;
    // SAFETY: a size query with a null buffer; `luid` and `length` are valid for the call. It
    // fails with ERROR_INSUFFICIENT_BUFFER by design.
    let _ = unsafe { LookupPrivilegeNameW(PCWSTR::null(), &raw mut luid, None, &raw mut length) };
    if length == 0 {
        return Err(failed(
            "LookupPrivilegeNameW (size)",
            &windows::core::Error::from_thread(),
        ));
    }
    let mut buffer = vec![0_u16; length as usize + 1];
    length = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
    // SAFETY: `buffer` holds `length` characters; the call writes at most that many and sets
    // `length` to the number written without the terminator.
    unsafe {
        LookupPrivilegeNameW(
            PCWSTR::null(),
            &raw mut luid,
            Some(PWSTR(buffer.as_mut_ptr())),
            &raw mut length,
        )
    }
    .map_err(|e| failed("LookupPrivilegeNameW", &e))?;
    Ok(String::from_utf16_lossy(&buffer[..length as usize]))
}

/// The names of the privileges this process's token holds.
///
/// # Errors
/// A message when the token cannot be read.
pub fn held_privileges() -> Result<Vec<String>, String> {
    let token = own_token(TOKEN_QUERY)?;
    Ok(token_privileges(&token)?
        .into_iter()
        .map(|(name, _)| name)
        .collect())
}

/// Removes every privilege except [`KEPT_PRIVILEGES`] from this process's token, then reads the
/// token back and fails if anything else is left.
fn strip_privileges() -> Result<(), String> {
    let token = own_token(TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES)?;
    let held = token_privileges(&token)?;
    let names: Vec<String> = held.iter().map(|(name, _)| name.clone()).collect();
    let doomed: HashSet<String> = privileges_to_remove(&names)
        .into_iter()
        .map(|name| name.to_ascii_lowercase())
        .collect();
    for (name, luid) in &held {
        if !doomed.contains(&name.to_ascii_lowercase()) {
            continue;
        }
        let state = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: *luid,
                Attributes: SE_PRIVILEGE_REMOVED,
            }],
        };
        // SAFETY: a live token handle with TOKEN_ADJUST_PRIVILEGES and a valid one-entry
        // TOKEN_PRIVILEGES that lives for the call. No previous state is requested.
        unsafe { AdjustTokenPrivileges(token.raw(), false, Some(&raw const state), 0, None, None) }
            .map_err(|e| failed(&format!("removing {name}"), &e))?;
        // The call succeeds with ERROR_NOT_ALL_ASSIGNED when it could not do what was asked.
        // SAFETY: reads this thread's last error; nothing else ran since the call.
        if unsafe { GetLastError() } == ERROR_NOT_ALL_ASSIGNED {
            return Err(format!("removing {name}: not all privileges were assigned"));
        }
    }
    let left: Vec<String> = token_privileges(&token)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let extra = privileges_to_remove(&left);
    if extra.is_empty() {
        Ok(())
    } else {
        Err(format!("privileges still held: {}", extra.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn only_the_three_named_privileges_are_kept() {
        let held = names(&[
            "SeDebugPrivilege",
            "SeProfileSingleProcessPrivilege",
            "SeBackupPrivilege",
            "SeIncreaseQuotaPrivilege",
            "SeRestorePrivilege",
            "SeChangeNotifyPrivilege",
            "SeTakeOwnershipPrivilege",
            "SeLoadDriverPrivilege",
            "SeImpersonatePrivilege",
            "SeShutdownPrivilege",
        ]);
        assert_eq!(
            privileges_to_remove(&held),
            names(&[
                "SeDebugPrivilege",
                "SeBackupPrivilege",
                "SeRestorePrivilege",
                "SeTakeOwnershipPrivilege",
                "SeLoadDriverPrivilege",
                "SeImpersonatePrivilege",
                "SeShutdownPrivilege",
            ])
        );
        assert!(privileges_to_remove(&names(&KEPT_PRIVILEGES)).is_empty());
        assert!(privileges_to_remove(&[]).is_empty());
        // Case does not matter to Windows.
        assert!(privileges_to_remove(&names(&["sechangenotifyprivilege"])).is_empty());
    }

    #[test]
    fn only_the_windows_variables_survive_the_scrub() {
        let set = names(&[
            "PATH",
            "SystemRoot",
            "TEMP",
            "windir",
            "COMSPEC",
            "SYSTEMDRIVE",
            "COR_PROFILER",
            "_NT_SYMBOL_PATH",
        ]);
        assert_eq!(
            variables_to_remove(&set),
            names(&["PATH", "TEMP", "COMSPEC", "COR_PROFILER", "_NT_SYMBOL_PATH"])
        );
    }

    #[test]
    fn the_policy_flags_are_the_documented_bits() {
        // The structures are 32-bit bit fields; these are the bits their documentation lists
        // first, in order.
        assert_eq!(PROHIBIT_DYNAMIC_CODE, 1);
        assert_eq!(IMAGE_LOAD, 0b111);
        assert_eq!(STRICT_HANDLE_CHECK, 0b11);
        assert_eq!(POLICIES.len(), 5);
    }

    #[test]
    fn the_system_directory_comes_from_windows() {
        let directory = system_directory().expect("system directory");
        assert!(
            directory.to_lowercase().ends_with("\\system32"),
            "{directory}"
        );
    }

    #[test]
    fn this_process_holds_privileges_that_hardening_would_name() {
        // Reading only: the real hardening is applied to a child process by the volt-helper
        // tests, because applying it here would change the test harness itself.
        let held = held_privileges().expect("privileges");
        assert!(
            held.iter()
                .any(|name| name.eq_ignore_ascii_case("SeChangeNotifyPrivilege")),
            "{held:?}"
        );
    }
}
