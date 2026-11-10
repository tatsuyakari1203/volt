//! [`Registry`] port on the Windows registry API.

use volt_core::{
    NamedValue, PortError, Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef,
    RegistryView,
};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS,
    ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    HKEY_USERS, KEY_ENUMERATE_SUB_KEYS, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, REG_BINARY, REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_OPTION_NON_VOLATILE,
    REG_QWORD, REG_SAM_FLAGS, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW,
    RegDeleteKeyExW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::{PCWSTR, PWSTR};

use crate::error::from_win32;
use crate::wide::to_wide_nul;

/// The registry port backed by the real Windows registry.
///
/// Stateless and cheap to construct. Reads open keys with `KEY_QUERY_VALUE`
/// only, writes and deletes with `KEY_SET_VALUE` only.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRegistry;

impl WinRegistry {
    /// A registry adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// An open registry key, closed on drop.
struct KeyHandle(HKEY);

impl Drop for KeyHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by a successful RegOpenKeyExW or
        // RegCreateKeyExW call, is owned exclusively by this value and is
        // closed exactly once, here.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

const fn root(hive: RegistryHive) -> HKEY {
    match hive {
        RegistryHive::ClassesRoot => HKEY_CLASSES_ROOT,
        RegistryHive::CurrentUser => HKEY_CURRENT_USER,
        RegistryHive::LocalMachine => HKEY_LOCAL_MACHINE,
        RegistryHive::Users => HKEY_USERS,
        RegistryHive::CurrentConfig => HKEY_CURRENT_CONFIG,
    }
}

fn view_flags(view: RegistryView) -> REG_SAM_FLAGS {
    match view {
        RegistryView::Native => REG_SAM_FLAGS(0),
        RegistryView::Bit32 => KEY_WOW64_32KEY,
        RegistryView::Bit64 => KEY_WOW64_64KEY,
    }
}

/// Validates a key reference that has to name a key below a hive (not the hive itself) and
/// returns it as the value reference the helpers below take, with its NUL-terminated subkey.
fn validate_key(key: &RegistryKeyRef) -> Result<(RegistryValueRef, Vec<u16>), PortError> {
    if key.subkey.is_empty() {
        return Err(PortError::InvalidInput(
            "a registry key to create or delete must be below a hive".into(),
        ));
    }
    let as_value = key.value(String::new());
    let (subkey, _) = validate(&as_value)?;
    Ok((as_value, subkey))
}

/// Validates the target and returns NUL-terminated UTF-16 subkey and name.
fn validate(target: &RegistryValueRef) -> Result<(Vec<u16>, Vec<u16>), PortError> {
    if target.subkey.starts_with('\\') || target.subkey.ends_with('\\') {
        return Err(PortError::InvalidInput(format!(
            "registry subkey must not start or end with a backslash: {}",
            target.subkey
        )));
    }
    Ok((
        to_wide_nul(&target.subkey, "registry subkey")?,
        to_wide_nul(&target.name, "registry value name")?,
    ))
}

fn open(
    target: &RegistryValueRef,
    subkey: &[u16],
    access: REG_SAM_FLAGS,
) -> Result<KeyHandle, WIN32_ERROR> {
    let mut key = HKEY::default();
    // SAFETY: `subkey` is a NUL-terminated UTF-16 buffer that outlives the
    // call, and `key` is a valid out-pointer for the duration of the call.
    let status = unsafe {
        RegOpenKeyExW(
            root(target.hive),
            PCWSTR(subkey.as_ptr()),
            None,
            access | view_flags(target.view),
            &raw mut key,
        )
    };
    if status == ERROR_SUCCESS {
        Ok(KeyHandle(key))
    } else {
        Err(status)
    }
}

impl Registry for WinRegistry {
    fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<NamedValue>, PortError> {
        // A key reference is validated like a value reference with an empty name.
        let as_value = key.value(String::new());
        let (subkey, _) = validate(&as_value)?;
        let handle = match open(&as_value, &subkey, KEY_QUERY_VALUE) {
            Ok(handle) => handle,
            Err(e) if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(from_win32(e.0, &format!("open {key}"))),
        };
        let mut listing = Vec::new();
        // A value name is at most 16,383 characters long.
        let mut name_buffer = vec![0_u16; 16_384];
        for index in 0_u32.. {
            let mut name_len = u32::try_from(name_buffer.len())
                .map_err(|_| PortError::Other("registry value name too long".into()))?;
            // SAFETY: `handle` is open with KEY_QUERY_VALUE; `name_buffer` is
            // valid for `name_len` UTF-16 units of writes and `name_len` is a
            // valid in/out pointer; the reserved, type and data arguments are
            // optional and passed as None, all for the duration of the call.
            let status = unsafe {
                RegEnumValueW(
                    handle.0,
                    index,
                    Some(PWSTR(name_buffer.as_mut_ptr())),
                    &raw mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(from_win32(status.0, &format!("list {key}")));
            }
            let name = String::from_utf16_lossy(&name_buffer[..name_len as usize]);
            let mut wide_name: Vec<u16> = name_buffer[..name_len as usize].to_vec();
            wide_name.push(0);
            match read_open(&handle, &wide_name, &as_value) {
                // Deleted between the enumeration and the read.
                Ok(None) => {}
                Ok(Some(value)) => listing.push(NamedValue {
                    name,
                    value: Ok(value),
                }),
                Err(error) => listing.push(NamedValue {
                    name,
                    value: Err(error),
                }),
            }
        }
        listing.sort_by_key(|entry| entry.name.to_lowercase());
        Ok(listing)
    }

    fn list_subkeys(&self, key: &RegistryKeyRef) -> Result<Vec<String>, PortError> {
        let as_value = key.value(String::new());
        let (subkey, _) = validate(&as_value)?;
        let handle = match open(&as_value, &subkey, KEY_ENUMERATE_SUB_KEYS) {
            Ok(handle) => handle,
            Err(e) if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND => {
                return Ok(Vec::new());
            }
            Err(e) => return Err(from_win32(e.0, &format!("open {key}"))),
        };
        let mut names = Vec::new();
        // A key name is at most 255 characters long.
        let mut name_buffer = vec![0_u16; 256];
        for index in 0_u32.. {
            let mut name_len = u32::try_from(name_buffer.len())
                .map_err(|_| PortError::Other("registry key name too long".into()))?;
            // SAFETY: `handle` is open with KEY_ENUMERATE_SUB_KEYS; `name_buffer` is valid for
            // `name_len` UTF-16 units of writes and `name_len` is a valid in/out pointer; the
            // reserved, class and last-write-time arguments are optional and passed as None, all
            // for the duration of the call.
            let status = unsafe {
                RegEnumKeyExW(
                    handle.0,
                    index,
                    Some(PWSTR(name_buffer.as_mut_ptr())),
                    &raw mut name_len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_NO_MORE_ITEMS {
                break;
            }
            if status != ERROR_SUCCESS {
                return Err(from_win32(status.0, &format!("list the subkeys of {key}")));
            }
            names.push(String::from_utf16_lossy(&name_buffer[..name_len as usize]));
        }
        names.sort_by_key(|name| name.to_lowercase());
        Ok(names)
    }

    fn create_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        let (as_value, subkey) = validate_key(key)?;
        let mut handle = HKEY::default();
        // SAFETY: `subkey` is NUL-terminated UTF-16 outliving the call; the class, security
        // attributes and disposition arguments are optional and passed as null/None; `handle`
        // is a valid out-pointer.
        let status = unsafe {
            RegCreateKeyExW(
                root(as_value.hive),
                PCWSTR(subkey.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | view_flags(as_value.view),
                None,
                &raw mut handle,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(from_win32(status.0, &format!("create {key}")));
        }
        // Closed by its drop.
        drop(KeyHandle(handle));
        Ok(())
    }

    fn delete_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        let (as_value, subkey) = validate_key(key)?;
        if !self.list_subkeys(key)?.is_empty() {
            return Err(PortError::Other(format!("{key} has subkeys")));
        }
        // SAFETY: `subkey` is NUL-terminated UTF-16 outliving the call; the view flag is the
        // only access right the function takes and `reserved` must be zero. Windows itself
        // refuses a key that has subkeys, and nothing but this one key is deleted.
        let status = unsafe {
            RegDeleteKeyExW(
                root(as_value.hive),
                PCWSTR(subkey.as_ptr()),
                view_flags(as_value.view).0,
                None,
            )
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(from_win32(status.0, &key.to_string()))
        }
    }

    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
        let (subkey, name) = validate(target)?;
        let key = match open(target, &subkey, KEY_QUERY_VALUE) {
            Ok(key) => key,
            Err(e) if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND => return Ok(None),
            Err(e) => return Err(from_win32(e.0, &format!("open {target}"))),
        };
        read_open(&key, &name, target)
    }

    fn write_value(
        &self,
        target: &RegistryValueRef,
        data: &RegistryValue,
    ) -> Result<(), PortError> {
        let (subkey, name) = validate(target)?;
        let (kind, bytes) = encode(data)?;

        let mut key = HKEY::default();
        // SAFETY: `subkey` is NUL-terminated UTF-16 outliving the call; the
        // class, security attributes and disposition arguments are optional
        // and passed as null/None; `key` is a valid out-pointer.
        let status = unsafe {
            RegCreateKeyExW(
                root(target.hive),
                PCWSTR(subkey.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE | view_flags(target.view),
                None,
                &raw mut key,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(from_win32(status.0, &format!("open {target} for writing")));
        }
        let key = KeyHandle(key);

        // SAFETY: `key` is open with KEY_SET_VALUE; `name` is NUL-terminated
        // UTF-16; `bytes` is a valid slice that outlives the call.
        let status =
            unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, kind, Some(&bytes)) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(from_win32(status.0, &format!("write {target}")))
        }
    }

    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError> {
        let (subkey, name) = validate(target)?;
        let key = open(target, &subkey, KEY_SET_VALUE)
            .map_err(|e| from_win32(e.0, &target.to_string()))?;
        // SAFETY: `key` is open with KEY_SET_VALUE and `name` is NUL-terminated
        // UTF-16 outliving the call.
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(from_win32(status.0, &target.to_string()))
        }
    }

    fn key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<bool, PortError> {
        // Reuse the value validation with a dummy name, so a key path is held to the same rules.
        let target = RegistryValueRef::new(hive, subkey, "").with_view(view);
        let (wide, _) = validate(&target)?;
        match open(&target, &wide, KEY_QUERY_VALUE) {
            // The handle is closed by its drop.
            Ok(_key) => Ok(true),
            Err(e) if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND => Ok(false),
            // The key is there, this process may just not open it: that is still "exists".
            Err(e) if e == ERROR_ACCESS_DENIED => Ok(true),
            Err(e) => Err(from_win32(e.0, &format!("open {hive:?}\\{subkey}"))),
        }
    }
}

/// Reads the value `name` (NUL-terminated UTF-16) of an open key. A missing
/// value is `Ok(None)`. `target` only names the value in error messages.
fn read_open(
    key: &KeyHandle,
    name: &[u16],
    target: &RegistryValueRef,
) -> Result<Option<RegistryValue>, PortError> {
    let mut buffer = vec![0_u8; 256];
    loop {
        let mut kind = REG_VALUE_TYPE(0);
        let mut len = u32::try_from(buffer.len())
            .map_err(|_| PortError::Other("registry value too large".into()))?;
        // SAFETY: `key` is open with KEY_QUERY_VALUE; `name` is a
        // NUL-terminated UTF-16 buffer; `buffer` is valid for `len` bytes
        // of writes and `kind`/`len` are valid out-pointers, all for the
        // duration of the call.
        let status = unsafe {
            RegQueryValueExW(
                key.0,
                PCWSTR(name.as_ptr()),
                None,
                Some(&raw mut kind),
                Some(buffer.as_mut_ptr()),
                Some(&raw mut len),
            )
        };
        if status == ERROR_MORE_DATA {
            // `len` holds the required size; add slack in case the value
            // grows between the two calls.
            buffer.resize(len as usize + 16, 0);
            continue;
        }
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(from_win32(status.0, &format!("read {target}")));
        }
        buffer.truncate(len as usize);
        return decode(kind, &buffer).map(Some).map_err(|e| match e {
            PortError::Unsupported(m) => PortError::Unsupported(format!("{target}: {m}")),
            other => other,
        });
    }
}

fn utf16_bytes(units: &[u16]) -> Vec<u8> {
    units.iter().flat_map(|u| u.to_le_bytes()).collect()
}

fn utf16_units(bytes: &[u8]) -> Vec<u16> {
    bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// Converts a value to its registry type and raw bytes.
fn encode(value: &RegistryValue) -> Result<(REG_VALUE_TYPE, Vec<u8>), PortError> {
    Ok(match value {
        RegistryValue::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
        RegistryValue::Qword(v) => (REG_QWORD, v.to_le_bytes().to_vec()),
        RegistryValue::Sz(s) => (REG_SZ, utf16_bytes(&to_wide_nul(s, "REG_SZ value")?)),
        RegistryValue::ExpandSz(s) => (
            REG_EXPAND_SZ,
            utf16_bytes(&to_wide_nul(s, "REG_EXPAND_SZ value")?),
        ),
        RegistryValue::MultiSz(items) => {
            let mut units = Vec::new();
            for item in items {
                if item.is_empty() {
                    return Err(PortError::InvalidInput(
                        "REG_MULTI_SZ cannot hold an empty string".into(),
                    ));
                }
                units.extend(to_wide_nul(item, "REG_MULTI_SZ item")?);
            }
            // The list ends with an extra NUL; an empty list is two NULs.
            if units.is_empty() {
                units.push(0);
            }
            units.push(0);
            (REG_MULTI_SZ, utf16_bytes(&units))
        }
        RegistryValue::Binary(b) => (REG_BINARY, b.clone()),
    })
}

/// Converts raw registry data of type `kind` to a value.
fn decode(kind: REG_VALUE_TYPE, bytes: &[u8]) -> Result<RegistryValue, PortError> {
    let malformed = |what: &str| PortError::Other(format!("malformed {what} registry data"));
    match kind {
        REG_DWORD => Ok(RegistryValue::Dword(u32::from_le_bytes(
            bytes.try_into().map_err(|_| malformed("REG_DWORD"))?,
        ))),
        REG_QWORD => Ok(RegistryValue::Qword(u64::from_le_bytes(
            bytes.try_into().map_err(|_| malformed("REG_QWORD"))?,
        ))),
        REG_SZ => Ok(RegistryValue::Sz(decode_string(bytes))),
        REG_EXPAND_SZ => Ok(RegistryValue::ExpandSz(decode_string(bytes))),
        REG_MULTI_SZ => {
            let units = utf16_units(bytes);
            let mut items: Vec<String> = units
                .split(|u| *u == 0)
                .map(String::from_utf16_lossy)
                .collect();
            // The terminator(s) produce trailing empty segments.
            while items.last().is_some_and(String::is_empty) {
                items.pop();
            }
            Ok(RegistryValue::MultiSz(items))
        }
        REG_BINARY => Ok(RegistryValue::Binary(bytes.to_vec())),
        other => Err(PortError::Unsupported(format!(
            "registry value type {} is not modelled",
            other.0
        ))),
    }
}

/// Decodes a string value, which may or may not carry its terminating NUL.
fn decode_string(bytes: &[u8]) -> String {
    let units = utf16_units(bytes);
    let end = units.iter().position(|u| *u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use volt_core::fakes::FakeRegistry;
    use windows::Win32::System::Registry::{RegDeleteKeyW, RegDeleteTreeW};

    use super::*;

    const BASE: &str = "Software\\VoltTest";

    /// A unique subkey under `HKCU\Software\VoltTest`, removed on drop, also
    /// when the test fails.
    struct TestKey {
        subkey: String,
        // Serialises tests: one test's cleanup removes the shared base key,
        // which would otherwise race with another test creating its subkey.
        // Declared last so it is released after `Drop::drop` has cleaned up.
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl TestKey {
        fn new() -> Self {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let serial = SERIAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            Self {
                _serial: serial,
                subkey: format!(
                    "{BASE}\\t-{}-{nanos}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ),
            }
        }

        fn value(&self, name: &str) -> RegistryValueRef {
            RegistryValueRef::new(RegistryHive::CurrentUser, self.subkey.clone(), name)
        }

        fn child(&self, child: &str, name: &str) -> RegistryValueRef {
            RegistryValueRef::new(
                RegistryHive::CurrentUser,
                format!("{}\\{child}", self.subkey),
                name,
            )
        }
    }

    impl Drop for TestKey {
        fn drop(&mut self) {
            let own = to_wide_nul(&self.subkey, "k").unwrap_or_default();
            let base = to_wide_nul(BASE, "k").unwrap_or_default();
            // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the
            // calls; HKEY_CURRENT_USER is a predefined key that needs no
            // closing. Only the unique subkey created by this test is
            // deleted; the base key is removed only when it is empty
            // (RegDeleteKeyW fails while subkeys remain, ignored on purpose).
            unsafe {
                let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(own.as_ptr()));
                let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(base.as_ptr()));
            }
        }
    }

    /// Behavioural contract every `Registry` implementation must satisfy.
    fn registry_contract(reg: &dyn Registry, key: &TestKey) {
        // A key that was never created does not exist.
        let exists =
            |subkey: &str| reg.key_exists(RegistryHive::CurrentUser, subkey, RegistryView::Native);
        assert_eq!(exists(&key.subkey), Ok(false));
        assert_eq!(exists(&format!("{}\\deep\\er", key.subkey)), Ok(false));

        // Missing key and missing value read as None.
        assert_eq!(reg.read_value(&key.value("nope")), Ok(None));
        assert_eq!(reg.read_value(&key.child("no\\such\\key", "v")), Ok(None));

        // Deleting a missing value is NotFound.
        assert!(matches!(
            reg.delete_value(&key.value("nope")),
            Err(PortError::NotFound(_))
        ));
        assert!(matches!(
            reg.delete_value(&key.child("no\\such\\key", "v")),
            Err(PortError::NotFound(_))
        ));

        // Every value kind round-trips, and writing creates missing keys.
        let values = [
            ("dword", RegistryValue::Dword(0xDEAD_BEEF)),
            ("qword", RegistryValue::Qword(u64::MAX - 7)),
            (
                "sz",
                RegistryValue::Sz("h\u{e9}llo w\u{f6}rld \u{1F600}".into()),
            ),
            ("empty_sz", RegistryValue::Sz(String::new())),
            ("expand", RegistryValue::ExpandSz("%SystemRoot%\\x".into())),
            (
                "multi",
                RegistryValue::MultiSz(vec!["a".into(), "b c".into()]),
            ),
            ("empty_multi", RegistryValue::MultiSz(vec![])),
            ("bin", RegistryValue::Binary(vec![0, 1, 2, 255])),
            ("empty_bin", RegistryValue::Binary(vec![])),
            ("", RegistryValue::Sz("default value".into())),
        ];
        for (name, value) in &values {
            let target = key.child("deep\\er", name);
            reg.write_value(&target, value).expect("write");
            assert_eq!(reg.read_value(&target), Ok(Some(value.clone())), "{name}");
        }

        // Writing created the key and its parents; a sibling that was not written still does
        // not exist, and neither does a longer path below the written one.
        assert_eq!(exists(&key.subkey), Ok(true));
        assert_eq!(exists(&format!("{}\\deep", key.subkey)), Ok(true));
        assert_eq!(exists(&format!("{}\\deep\\er", key.subkey)), Ok(true));
        assert_eq!(
            exists(&format!("{}\\deep\\er\\more", key.subkey)),
            Ok(false)
        );
        assert_eq!(exists(&format!("{}\\other", key.subkey)), Ok(false));
        // Case does not matter, like the registry itself.
        assert_eq!(exists(&key.subkey.to_uppercase()), Ok(true));
        listing_contract(reg, key);

        // Overwrite may change the type.
        let target = key.value("morph");
        reg.write_value(&target, &RegistryValue::Dword(1))
            .expect("write");
        reg.write_value(&target, &RegistryValue::Sz("one".into()))
            .expect("write");
        assert_eq!(
            reg.read_value(&target),
            Ok(Some(RegistryValue::Sz("one".into())))
        );

        // Delete removes exactly that value; a second delete is NotFound.
        reg.delete_value(&target).expect("delete");
        assert_eq!(reg.read_value(&target), Ok(None));
        assert!(matches!(
            reg.delete_value(&target),
            Err(PortError::NotFound(_))
        ));
        // Neighbours survive.
        assert_eq!(
            reg.read_value(&key.child("deep\\er", "dword")),
            Ok(Some(RegistryValue::Dword(0xDEAD_BEEF)))
        );
    }

    /// Listing returns the values directly in a key, with the spelling they were written with,
    /// sorted by name ignoring case; a missing key is empty.
    fn listing_contract(reg: &dyn Registry, key: &TestKey) {
        reg.write_value(&key.child("deep\\er", "MiXed"), &RegistryValue::Dword(9))
            .expect("write");
        let listing = reg
            .list_values(&RegistryKeyRef::new(
                RegistryHive::CurrentUser,
                format!("{}\\deep\\er", key.subkey),
                RegistryView::Native,
            ))
            .expect("list");
        let names: Vec<_> = listing.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "",
                "bin",
                "dword",
                "empty_bin",
                "empty_multi",
                "empty_sz",
                "expand",
                "MiXed",
                "multi",
                "qword",
                "sz"
            ]
        );
        let dword = listing.iter().find(|v| v.name == "dword").expect("dword");
        assert_eq!(dword.value, Ok(RegistryValue::Dword(0xDEAD_BEEF)));
        // Subkeys are not values: `deep` holds only the key `er`.
        let parent = RegistryKeyRef::new(
            RegistryHive::CurrentUser,
            format!("{}\\deep", key.subkey),
            RegistryView::Native,
        );
        assert_eq!(reg.list_values(&parent), Ok(vec![]));
        let missing = RegistryKeyRef::new(
            RegistryHive::CurrentUser,
            format!("{}\\no\\such", key.subkey),
            RegistryView::Native,
        );
        assert_eq!(reg.list_values(&missing), Ok(vec![]));
    }

    /// Creating, listing and deleting keys. Every key lives below the test key.
    fn key_contract(reg: &dyn Registry, key: &TestKey) {
        let at = |path: &str| {
            RegistryKeyRef::new(
                RegistryHive::CurrentUser,
                format!("{}\\{path}", key.subkey),
                RegistryView::Native,
            )
        };
        let exists = |path: &str| {
            reg.key_exists(
                RegistryHive::CurrentUser,
                &at(path).subkey,
                RegistryView::Native,
            )
        };

        // Creating makes the key and its parents; doing it again changes nothing.
        reg.create_key(&at("a\\b")).expect("create");
        reg.create_key(&at("a\\b")).expect("create again");
        assert_eq!(exists("a"), Ok(true));
        assert_eq!(exists("a\\b"), Ok(true));
        assert_eq!(reg.list_subkeys(&at("a")), Ok(vec!["b".to_owned()]));
        assert_eq!(reg.list_subkeys(&at("a\\b")), Ok(vec![]));
        assert_eq!(reg.list_subkeys(&at("nope")), Ok(vec![]));
        assert_eq!(reg.list_values(&at("a\\b")), Ok(vec![]));

        // A key with a subkey is refused and nothing is deleted.
        assert!(reg.delete_key(&at("a")).is_err());
        assert_eq!(exists("a\\b"), Ok(true));

        // A key is deleted with the values in it; its parent stays.
        reg.write_value(&at("a\\b").value("v"), &RegistryValue::Dword(1))
            .expect("write");
        reg.delete_key(&at("a\\b")).expect("delete");
        assert_eq!(exists("a\\b"), Ok(false));
        assert_eq!(exists("a"), Ok(true));
        assert_eq!(reg.read_value(&at("a\\b").value("v")), Ok(None));
        reg.delete_key(&at("a")).expect("delete the parent");
        assert_eq!(exists("a"), Ok(false));

        // A missing key is NotFound.
        assert!(matches!(
            reg.delete_key(&at("a")),
            Err(PortError::NotFound(_))
        ));

        // A hive itself is never created or deleted.
        let hive = RegistryKeyRef::new(RegistryHive::CurrentUser, "", RegistryView::Native);
        assert!(matches!(
            reg.delete_key(&hive),
            Err(PortError::InvalidInput(_))
        ));
    }

    #[test]
    fn key_operations_hold_for_the_fake() {
        key_contract(&FakeRegistry::new(), &TestKey::new());
    }

    #[test]
    fn key_operations_hold_for_the_real_registry() {
        let key = TestKey::new();
        key_contract(&WinRegistry::new(), &key);
    }

    #[test]
    fn contract_holds_for_the_fake() {
        registry_contract(&FakeRegistry::new(), &TestKey::new());
    }

    #[test]
    fn contract_holds_for_the_real_registry() {
        let key = TestKey::new();
        registry_contract(&WinRegistry::new(), &key);
    }

    #[test]
    fn both_registry_views_work_under_hkcu() {
        let key = TestKey::new();
        let reg = WinRegistry::new();
        for view in [
            RegistryView::Bit32,
            RegistryView::Bit64,
            RegistryView::Native,
        ] {
            let target = key.value("view").with_view(view);
            reg.write_value(&target, &RegistryValue::Dword(7))
                .expect("write");
            assert_eq!(reg.read_value(&target), Ok(Some(RegistryValue::Dword(7))));
            reg.delete_value(&target).expect("delete");
        }
    }

    #[test]
    fn malformed_subkeys_are_invalid_input() {
        let reg = WinRegistry::new();
        for subkey in ["\\Software", "Software\\", "a\0b"] {
            let target = RegistryValueRef::new(RegistryHive::CurrentUser, subkey, "v");
            assert!(
                matches!(reg.read_value(&target), Err(PortError::InvalidInput(_))),
                "{subkey:?}"
            );
        }
    }

    #[test]
    fn key_existence_rejects_malformed_subkeys_and_counts_a_denied_key_as_existing() {
        let reg = WinRegistry::new();
        for subkey in ["\\Software", "Software\\", "a\0b"] {
            assert!(
                matches!(
                    reg.key_exists(RegistryHive::CurrentUser, subkey, RegistryView::Native),
                    Err(PortError::InvalidInput(_))
                ),
                "{subkey:?}"
            );
        }
        // HKLM\SAM\SAM is readable by SYSTEM only, so opening it is denied, yet it exists.
        // Read-only: nothing is modified.
        assert_eq!(
            reg.key_exists(RegistryHive::LocalMachine, "SAM\\SAM", RegistryView::Native),
            Ok(true)
        );
    }

    #[test]
    fn reading_a_key_the_user_cannot_open_is_access_denied() {
        // HKLM\SAM\SAM is readable by SYSTEM only, even for administrators.
        // Read-only: nothing is modified.
        let target = RegistryValueRef::new(RegistryHive::LocalMachine, "SAM\\SAM", "x");
        assert!(matches!(
            WinRegistry::new().read_value(&target),
            Err(PortError::AccessDenied(_))
        ));
    }

    #[test]
    fn value_encoding_round_trips_without_the_registry() {
        for value in [
            RegistryValue::MultiSz(vec!["x".into()]),
            RegistryValue::MultiSz(vec![]),
            RegistryValue::Sz("a".into()),
        ] {
            let (kind, bytes) = encode(&value).expect("encode");
            assert_eq!(decode(kind, &bytes), Ok(value));
        }
        assert!(matches!(
            encode(&RegistryValue::MultiSz(vec![String::new()])),
            Err(PortError::InvalidInput(_))
        ));
        assert!(matches!(
            decode(REG_DWORD, &[1, 2]),
            Err(PortError::Other(_))
        ));
        assert!(matches!(
            decode(REG_VALUE_TYPE(0), &[]),
            Err(PortError::Unsupported(_))
        ));
    }
}
