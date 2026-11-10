//! Risk and evidence labels. The UI shows both next to every change.

use std::fmt;

use serde::{Deserialize, Serialize};

/// How dangerous a change is. Ordered: `Safe < Caution < Advanced`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Risk {
    /// Harmless and easy to undo.
    #[serde(alias = "safe")]
    Safe,
    /// Can change behaviour in ways the user may notice.
    #[serde(alias = "caution")]
    Caution,
    /// Can break software or the system if misused; for experts.
    #[serde(alias = "advanced")]
    Advanced,
}

impl fmt::Display for Risk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Safe => "Safe",
            Self::Caution => "Caution",
            Self::Advanced => "Advanced",
        })
    }
}

/// How well the claimed benefit of a change is established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Evidence {
    /// The effect was measured.
    #[serde(alias = "measured")]
    Measured,
    /// The effect is reported by users but not measured.
    #[serde(alias = "anecdotal")]
    Anecdotal,
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Measured => "Measured",
            Self::Anecdotal => "Anecdotal",
        })
    }
}
