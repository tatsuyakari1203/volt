//! Reads owners and access control lists into plain values, so rules about them are pure code that
//! tests can feed with made-up lists, and only the reading touches Windows.

use std::ffi::c_void;
use std::path::Path;

use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, HLOCAL, LocalFree, WIN32_ERROR};
use windows::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, GetSecurityInfo, SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_OBJECT_TYPE,
    SE_REGISTRY_KEY,
};
use windows::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetAce, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
};
use windows::Win32::System::Registry::HKEY;
use windows::core::PCWSTR;

use super::identity::sid_to_string;
use super::{OwnedHandle, wide};

/// The ACE flag that makes an entry apply to children only, not to the object itself.
pub const INHERIT_ONLY_ACE: u8 = 0x08;

/// One access control entry that grants or denies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ace {
    /// Whether the entry grants (`true`) or denies (`false`).
    pub allow: bool,
    /// Who, as a SID string.
    pub sid: String,
    /// The access mask.
    pub mask: u32,
    /// The ACE flags (inheritance).
    pub flags: u8,
}

/// An object's owner and discretionary ACL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityInfo {
    /// The owner, as a SID string.
    pub owner: String,
    /// The entries, in order; `None` for a NULL DACL, which grants everyone everything.
    pub dacl: Option<Vec<Ace>>,
}

/// A security descriptor allocated by the system, freed on drop.
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            // SAFETY: the descriptor was allocated by the Get*SecurityInfo call that filled it and
            // is freed exactly once, here.
            let _ = unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
        }
    }
}

fn check(status: WIN32_ERROR, what: &str) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("{what}: Win32 error {}", status.0))
    }
}

/// Turns the pieces the system returned into a [`SecurityInfo`].
///
/// # Safety
/// `owner` must be null or point to a valid SID, and `dacl` must be null or point to a valid ACL,
/// both alive for the call (they live inside the descriptor the caller still holds).
unsafe fn read(owner: PSID, dacl: *const ACL, has_dacl: bool) -> Result<SecurityInfo, String> {
    if owner.0.is_null() {
        return Err("the object has no owner".to_owned());
    }
    let owner = sid_to_string(owner)?;
    if !has_dacl || dacl.is_null() {
        return Ok(SecurityInfo { owner, dacl: None });
    }
    // SAFETY: `dacl` is a valid ACL by the caller's contract.
    let count = unsafe { (*dacl).AceCount };
    let mut aces = Vec::with_capacity(usize::from(count));
    for index in 0..u32::from(count) {
        let mut raw: *mut c_void = std::ptr::null_mut();
        // SAFETY: `dacl` is valid and `index` is below its ACE count; `raw` is a valid out pointer.
        unsafe { GetAce(dacl, index, &raw mut raw) }.map_err(|e| super::failed("GetAce", &e))?;
        // SAFETY: GetAce returned a pointer to an ACE inside the ACL, which is alive.
        aces.push(unsafe { parse_ace(raw.cast::<u8>()) }?);
    }
    Ok(SecurityInfo {
        owner,
        dacl: Some(aces),
    })
}

/// Reads one ACE. Allow and deny entries (plain, callback and object forms) are understood; any
/// other type in a DACL is an error, so an entry this code cannot read is never taken as harmless.
///
/// # Safety
/// `ace` must point to a complete ACE inside a live ACL.
unsafe fn parse_ace(ace: *const u8) -> Result<Ace, String> {
    // SAFETY: every ACE starts with the four-byte header: type, flags, size.
    let (kind, flags) = unsafe { (ace.read_unaligned(), ace.add(1).read_unaligned()) };
    let (allow, sid_offset) = match kind {
        // ACCESS_ALLOWED / ACCESS_DENIED / the callback forms: header (4) + mask (4) + SID.
        0 | 9 => (true, 8),
        1 | 10 => (false, 8),
        // The object forms: header, mask, flags, then up to two GUIDs, then the SID.
        5 | 6 | 11 | 12 => {
            // SAFETY: the object forms carry a four-byte flags word at offset 8.
            let object_flags = unsafe { ace.add(8).cast::<u32>().read_unaligned() };
            let guids = usize::from(object_flags & 1 != 0) + usize::from(object_flags & 2 != 0);
            (matches!(kind, 5 | 11), 12 + 16 * guids)
        }
        other => return Err(format!("unsupported ACE type {other}")),
    };
    // SAFETY: the mask follows the header; the SID follows at the computed offset, inside the ACE.
    let (mask, sid) = unsafe {
        (
            ace.add(4).cast::<u32>().read_unaligned(),
            PSID(ace.add(sid_offset).cast_mut().cast::<c_void>()),
        )
    };
    Ok(Ace {
        allow,
        sid: sid_to_string(sid)?,
        mask,
        flags,
    })
}

/// Owner and DACL of a file or folder.
///
/// # Errors
/// A message when the object cannot be read.
pub fn of_path(path: &Path) -> Result<SecurityInfo, String> {
    let name = wide(&path.to_string_lossy());
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: `name` is NUL-terminated and outlives the call; the out pointers are valid. The
    // owner and DACL pointers point into `descriptor`, which is freed by the guard below.
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(name.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            Some(&raw mut dacl),
            None,
            &raw mut descriptor,
        )
    };
    let descriptor = Descriptor(descriptor);
    check(status, "GetNamedSecurityInfoW")?;
    // SAFETY: the pointers came from the call above and `descriptor` is alive until after `read`.
    let info = unsafe { read(owner, dacl, true) };
    drop(descriptor);
    info
}

/// Owner and DACL of a kernel object (a pipe) through a handle with `READ_CONTROL`.
///
/// # Errors
/// A message when the object cannot be read.
pub(crate) fn of_kernel_handle(handle: &OwnedHandle) -> Result<SecurityInfo, String> {
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let object: SE_OBJECT_TYPE = SE_KERNEL_OBJECT;
    // SAFETY: a valid handle and valid out pointers; the owner and DACL pointers point into
    // `descriptor`, which is freed by the guard below.
    let status = unsafe {
        GetSecurityInfo(
            handle.raw(),
            object,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            Some(&raw mut dacl),
            None,
            Some(&raw mut descriptor),
        )
    };
    let descriptor = Descriptor(descriptor);
    check(status, "GetSecurityInfo")?;
    // SAFETY: as in `of_path`.
    let info = unsafe { read(owner, dacl, true) };
    drop(descriptor);
    info
}

/// Owner and DACL of an open registry key (a handle with `READ_CONTROL`).
///
/// # Errors
/// A message when the key cannot be read.
pub(crate) fn of_registry_key(key: HKEY) -> Result<SecurityInfo, String> {
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: an open key handle (a registry handle is a kernel object handle) and valid out
    // pointers; the owner and DACL pointers point into `descriptor`, which is freed by the guard
    // below.
    let status = unsafe {
        GetSecurityInfo(
            HANDLE(key.0),
            SE_REGISTRY_KEY,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&raw mut owner),
            None,
            Some(&raw mut dacl),
            None,
            Some(&raw mut descriptor),
        )
    };
    let descriptor = Descriptor(descriptor);
    check(status, "GetSecurityInfo")?;
    // SAFETY: as in `of_path`.
    let info = unsafe { read(owner, dacl, true) };
    drop(descriptor);
    info
}
