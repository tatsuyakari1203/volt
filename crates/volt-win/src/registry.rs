//! [`Registry`] port on the Windows registry API.

use volt_core::{PortError, Registry, RegistryHive, RegistryValue, RegistryValueRef, RegistryView};
use windows::Win32::Foundation::{
    ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_CONFIG, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    HKEY_USERS, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_BINARY,
    REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_OPTION_NON_VOLATILE, REG_QWORD, REG_SAM_FLAGS,
    REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW,
    RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

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
    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
        let (subkey, name) = validate(target)?;
        let key = match open(target, &subkey, KEY_QUERY_VALUE) {
            Ok(key) => key,
            Err(e) if e == ERROR_FILE_NOT_FOUND || e == ERROR_PATH_NOT_FOUND => return Ok(None),
            Err(e) => return Err(from_win32(e.0, &format!("open {target}"))),
        };

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
