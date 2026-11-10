//! Registry model and port. No Windows types leak through this module.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::PortError;

/// Registry root key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegistryHive {
    /// `HKEY_CLASSES_ROOT`
    ClassesRoot,
    /// `HKEY_CURRENT_USER`
    CurrentUser,
    /// `HKEY_LOCAL_MACHINE`
    LocalMachine,
    /// `HKEY_USERS`
    Users,
    /// `HKEY_CURRENT_CONFIG`
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
    Native,
    /// The 32-bit view.
    Bit32,
    /// The 64-bit view.
    Bit64,
}

/// A registry value by type. Mirrors the registry's own value types.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RegistryValue {
    /// `REG_DWORD`
    Dword(u32),
    /// `REG_QWORD`
    Qword(u64),
    /// `REG_SZ`
    Sz(String),
    /// `REG_EXPAND_SZ`: a string with unexpanded `%VAR%` references.
    ExpandSz(String),
    /// `REG_MULTI_SZ`
    MultiSz(Vec<String>),
    /// `REG_BINARY`
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

/// Port: read and write registry values.
///
/// Adapters implement this over the real registry; the core only sees the trait.
pub trait Registry: Send + Sync {
    /// Reads a value. A missing key or value is `Ok(None)`, not an error.
    fn read_value(&self, target: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError>;

    /// Writes a value, creating the key if needed. Single and atomic.
    fn write_value(&self, target: &RegistryValueRef, data: &RegistryValue)
    -> Result<(), PortError>;

    /// Deletes a value. A missing value is [`PortError::NotFound`]. Keys are
    /// never deleted.
    fn delete_value(&self, target: &RegistryValueRef) -> Result<(), PortError>;
}
