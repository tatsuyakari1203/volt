//! Windows service start types, without Windows types.

use std::fmt;

use serde::{Deserialize, Serialize};

/// When a service starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceStartType {
    /// Started by the boot loader (drivers).
    #[serde(alias = "boot")]
    Boot,
    /// Started during kernel initialisation (drivers).
    #[serde(alias = "system")]
    System,
    /// Started automatically at boot.
    #[serde(alias = "automatic")]
    Automatic,
    /// Started automatically shortly after boot.
    #[serde(alias = "automatic_delayed")]
    AutomaticDelayed,
    /// Started on demand.
    #[serde(alias = "manual")]
    Manual,
    /// Cannot be started.
    #[serde(alias = "disabled")]
    Disabled,
}

impl fmt::Display for ServiceStartType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Boot => "Boot",
            Self::System => "System",
            Self::Automatic => "Automatic",
            Self::AutomaticDelayed => "Automatic (delayed)",
            Self::Manual => "Manual",
            Self::Disabled => "Disabled",
        })
    }
}
