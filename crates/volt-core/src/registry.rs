//! Registry model and port. No Windows types leak through this module.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PortError;

/// Registry root key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegistryHive {
    /// `HKEY_CLASSES_ROOT`
    #[serde(alias = "HKCR", alias = "classes_root")]
    ClassesRoot,
    /// `HKEY_CURRENT_USER`
    #[serde(alias = "HKCU", alias = "current_user")]
    CurrentUser,
    /// `HKEY_LOCAL_MACHINE`
    #[serde(alias = "HKLM", alias = "local_machine")]
    LocalMachine,
    /// `HKEY_USERS`
    #[serde(alias = "HKU", alias = "users")]
    Users,
    /// `HKEY_CURRENT_CONFIG`
    #[serde(alias = "HKCC", alias = "current_config")]
    CurrentConfig,
}

impl RegistryHive {
    /// The conventional short name, such as `HKLM`.
    #[must_use]
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::ClassesRoot => "HKCR",
            Self::CurrentUser => "HKCU",
            Self::LocalMachine => "HKLM",
            Self::Users => "HKU",
            Self::CurrentConfig => "HKCC",
        }
    }
}

/// Which registry view to use on 64-bit Windows (`WOW64` redirection).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegistryView {
    /// The view native to the calling process.
    #[default]
    #[serde(alias = "native")]
    Native,
    /// The 32-bit view.
    #[serde(alias = "bit32")]
    Bit32,
    /// The 64-bit view.
    #[serde(alias = "bit64")]
    Bit64,
}

/// A registry value by type. Mirrors the registry's own value types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegistryValue {
    /// `REG_DWORD`
    #[serde(alias = "dword")]
    Dword(u32),
    /// `REG_QWORD`
    #[serde(alias = "qword")]
    Qword(u64),
    /// `REG_SZ`
    #[serde(alias = "sz")]
    Sz(String),
    /// `REG_EXPAND_SZ`: a string with unexpanded `%VAR%` references.
    #[serde(alias = "expand_sz")]
    ExpandSz(String),
    /// `REG_MULTI_SZ`
    #[serde(alias = "multi_sz")]
    MultiSz(Vec<String>),
    /// `REG_BINARY`
    #[serde(alias = "binary")]
    Binary(Vec<u8>),
}

impl fmt::Display for RegistryValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Dword(v) => write!(f, "REG_DWORD {v}"),
            Self::Qword(v) => write!(f, "REG_QWORD {v}"),
            Self::Sz(v) => write!(f, "REG_SZ \"{v}\""),
            Self::ExpandSz(v) => write!(f, "REG_EXPAND_SZ \"{v}\""),
            Self::MultiSz(v) => write!(f, "REG_MULTI_SZ [{}]", v.join(", ")),
            Self::Binary(v) => {
                write!(f, "REG_BINARY ")?;
                for byte in v {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
    }
}

/// Address of one registry value: hive, subkey path, value name and view.
///
/// An empty `name` addresses the key's default value.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegistryValueRef {
    /// Root key.
    pub hive: RegistryHive,
    /// Subkey path below the hive, with backslash separators and no leading one.
    pub subkey: String,
    /// Value name; empty for the default value.
    pub name: String,
    /// 32-bit or 64-bit view.
    pub view: RegistryView,
}

impl RegistryValueRef {
    /// A reference in the native view.
    #[must_use]
    pub fn new(hive: RegistryHive, subkey: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            hive,
            subkey: subkey.into(),
            name: name.into(),
            view: RegistryView::Native,
        }
    }

    /// The same reference in another view.
    #[must_use]
    pub fn with_view(mut self, view: RegistryView) -> Self {
        self.view = view;
        self
    }
}

impl fmt::Display for RegistryValueRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = if self.name.is_empty() {
            "(Default)"
        } else {
            self.name.as_str()
        };
        write!(f, "{}\\{}\\{}", self.hive.short_name(), self.subkey, name)?;
        match self.view {
            RegistryView::Native => Ok(()),
            RegistryView::Bit32 => f.write_str(" [32-bit]"),
            RegistryView::Bit64 => f.write_str(" [64-bit]"),
        }
    }
}

/// Address of one registry key: hive, subkey path and view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegistryKeyRef {
    /// Root key.
    pub hive: RegistryHive,
    /// Subkey path below the hive, with backslash separators and no leading one.
    pub subkey: String,
    /// 32-bit or 64-bit view.
    pub view: RegistryView,
}

impl RegistryKeyRef {
    /// A key reference in the given view.
    #[must_use]
    pub fn new(hive: RegistryHive, subkey: impl Into<String>, view: RegistryView) -> Self {
        Self {
            hive,
            subkey: subkey.into(),
            view,
        }
    }

    /// The address of the value `name` in this key.
    #[must_use]
    pub fn value(&self, name: impl Into<String>) -> RegistryValueRef {
        RegistryValueRef::new(self.hive, self.subkey.clone(), name).with_view(self.view)
    }
}

impl fmt::Display for RegistryKeyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\\{}", self.hive.short_name(), self.subkey)?;
        match self.view {
            RegistryView::Native => Ok(()),
            RegistryView::Bit32 => f.write_str(" [32-bit]"),
            RegistryView::Bit64 => f.write_str(" [64-bit]"),
        }
    }
}

/// One value found by [`Registry::list_values`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedValue {
    /// The value's name (empty for the default value).
    pub name: String,
    /// The data, or why it could not be read (for example a value type the
    /// model does not cover). One unreadable value does not fail the listing.
    pub value: Result<RegistryValue, PortError>,
}

/// Port: read and write registry values.
///
/// Adapters implement this over the real registry; the core only sees the trait.
pub trait Registry: Send + Sync {
    /// Lists the values directly in a key (not its subkeys), sorted by name
    /// ignoring case. Read-only. A missing key is `Ok` with an empty list.
    ///
    /// The default answers [`PortError::Unsupported`], so an adapter that has
    /// no enumeration yet still satisfies the port.
    fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<NamedValue>, PortError> {
        Err(PortError::Unsupported(format!(
            "listing the values of {key}"
        )))
    }

    /// Reads a value. A missing key or value is `Ok(None)`, not an error.
    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError>;

    /// Writes a value, creating the key if needed. Single and atomic.
    fn write_value(&self, target: &RegistryValueRef, data: &RegistryValue)
    -> Result<(), PortError>;

    /// Deletes a value. A missing value is [`PortError::NotFound`]. Keys are
    /// never deleted.
    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError>;

    /// Lists the names of the keys directly below `key`, sorted by name ignoring case. Read-only.
    /// A missing key is `Ok` with an empty list.
    ///
    /// The default answers [`PortError::Unsupported`].
    fn list_subkeys(&self, key: &RegistryKeyRef) -> Result<Vec<String>, PortError> {
        Err(PortError::Unsupported(format!(
            "listing the subkeys of {key}"
        )))
    }

    /// Creates a key and any missing parents. A key that exists is left as it is. Single and
    /// atomic. The default answers [`PortError::Unsupported`].
    fn create_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        Err(PortError::Unsupported(format!("creating {key}")))
    }

    /// Deletes a key together with the values in it. A missing key is [`PortError::NotFound`]; a
    /// key that has subkeys is refused and nothing is deleted. The default answers
    /// [`PortError::Unsupported`].
    fn delete_key(&self, key: &RegistryKeyRef) -> Result<(), PortError> {
        Err(PortError::Unsupported(format!("deleting {key}")))
    }

    /// Whether a key exists, read-only. Some Windows markers are keys with no value in them
    /// (such as the pending-restart keys), so they cannot be tested with
    /// [`read_value`](Self::read_value). A key the caller may not open still exists: an adapter
    /// answers `Ok(true)` for it rather than guessing "missing". `subkey` follows the same
    /// rules as [`RegistryValueRef::subkey`].
    fn key_exists(
        &self,
        hive: RegistryHive,
        subkey: &str,
        view: RegistryView,
    ) -> Result<bool, PortError>;
}

/// Most values a key may hold for Volt to record it before deleting it.
pub const MAX_RECORDED_VALUES: usize = 64;

/// Most bytes of value data (strings counted in UTF-8, binary as it is) Volt records for a key.
pub const MAX_RECORDED_BYTES: usize = 64 * 1024;

/// What a key held when it was read: everything Volt needs to put it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryKeyContents {
    /// The values directly in the key, by name (empty for the default value), sorted by name.
    pub values: Vec<(String, RegistryValue)>,
    /// `false` when the key holds more than `values` shows: it has subkeys, a value the model
    /// does not cover, or more data than Volt records. Such a key is never deleted.
    pub complete: bool,
}

impl RegistryKeyContents {
    /// Whether the key holds nothing at all: no values and no subkeys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.complete && self.values.is_empty()
    }
}

fn value_bytes(value: &RegistryValue) -> usize {
    match value {
        RegistryValue::Dword(_) => 4,
        RegistryValue::Qword(_) => 8,
        RegistryValue::Sz(v) | RegistryValue::ExpandSz(v) => v.len(),
        RegistryValue::MultiSz(v) => v.iter().map(String::len).sum(),
        RegistryValue::Binary(v) => v.len(),
    }
}

/// Reads a key with its values; `None` when it does not exist.
pub(crate) fn read_key_contents(
    registry: &dyn Registry,
    key: &RegistryKeyRef,
) -> Result<Option<RegistryKeyContents>, PortError> {
    if !registry.key_exists(key.hive, &key.subkey, key.view)? {
        return Ok(None);
    }
    let mut complete = registry.list_subkeys(key)?.is_empty();
    let mut values = Vec::new();
    let mut bytes = 0_usize;
    for named in registry.list_values(key)? {
        let Ok(value) = named.value else {
            complete = false;
            continue;
        };
        bytes = bytes.saturating_add(value_bytes(&value));
        if values.len() >= MAX_RECORDED_VALUES || bytes > MAX_RECORDED_BYTES {
            // Too much to record: stop collecting, the key is not complete.
            complete = false;
            break;
        }
        values.push((named.name, value));
    }
    Ok(Some(RegistryKeyContents { values, complete }))
}

/// How many levels of `key`'s path do not exist yet, counted from the key itself upwards: `0`
/// when the key exists. Writing a value or creating the key creates exactly these.
pub(crate) fn missing_levels(
    registry: &dyn Registry,
    key: &RegistryKeyRef,
) -> Result<u32, PortError> {
    let mut missing = 0;
    let mut path = key.subkey.as_str();
    while !path.is_empty() && !registry.key_exists(key.hive, path, key.view)? {
        missing += 1;
        path = parent_path(path);
    }
    Ok(missing)
}

/// Removes `key` and then up to `levels - 1` of its parents, each only while it holds nothing:
/// no value and no subkey. A key that is already gone is skipped. Stops at the first key that
/// holds something, which then is not Volt's to remove.
pub(crate) fn remove_empty_keys(
    registry: &dyn Registry,
    key: &RegistryKeyRef,
    levels: u32,
) -> Result<(), PortError> {
    let mut path = key.subkey.as_str();
    for _ in 0..levels {
        if path.is_empty() {
            break;
        }
        let current = RegistryKeyRef::new(key.hive, path, key.view);
        if let Some(contents) = read_key_contents(registry, &current)? {
            if !contents.is_empty() {
                break;
            }
            match registry.delete_key(&current) {
                Ok(()) | Err(PortError::NotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        path = parent_path(path);
    }
    Ok(())
}

/// The path one level up; empty for a top-level key.
fn parent_path(path: &str) -> &str {
    path.rsplit_once('\\').map_or("", |(parent, _)| parent)
}
