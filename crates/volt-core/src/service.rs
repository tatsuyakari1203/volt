//! Windows service start types, without Windows types.

use std::fmt;

use serde::{Deserialize, Serialize};

/// When a service starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ServiceStartType {
    /// Started by the boot loader (drivers).
    Boot,
    /// Started during kernel initialisation (drivers).
    System,
    /// Started automatically at boot.
    Automatic,
    /// Started automatically shortly after boot.
    AutomaticDelayed,
    /// Started on demand.
    Manual,
    /// Cannot be started.
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
