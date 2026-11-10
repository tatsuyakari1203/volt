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
