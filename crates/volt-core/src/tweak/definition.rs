//! The tweak model: a tweak is data that says exactly what it changes.

use serde::{Deserialize, Serialize};

use crate::command::Command;
use crate::label::{Evidence, Risk};
use crate::operation::Operation;
use crate::registry::{RegistryHive, RegistryValue, RegistryValueRef, RegistryView};
use crate::service::ServiceStartType;

/// The group a tweak belongs to in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Speed and responsiveness.
    Performance,
    /// Settings for games.
    Gaming,
    /// Telemetry, advertising and tracking.
    Privacy,
    /// Look and feel of the shell.
    Appearance,
    /// Networking.
    Network,
    /// Windows Update behaviour.
    WindowsUpdate,
    /// Security features; always an expert area.
    Security,
    /// Preinstalled software and background tasks.
    Debloat,
}

/// What the user has to do before a tweak takes full effect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestartRequirement {
    /// Nothing; it takes effect at once.
    #[default]
    None,
    /// The user has to sign out and in again.
    SignOut,
    /// The machine has to restart.
    Reboot,
}

/// One change a tweak makes. Steps run in order; revert undoes them newest
/// first.
///
/// In a catalogue file the step kind is the `kind` field; see
/// [`Catalogue::from_toml`](crate::tweak::Catalogue::from_toml).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TweakStep {
    /// Write a registry value.
    RegistrySet {
        /// Root key.
        hive: RegistryHive,
        /// Subkey path below the hive.
        subkey: String,
        /// Value name; empty for the key's default value.
        name: String,
        /// Registry view.
        view: RegistryView,
        /// The data to write.
        value: RegistryValue,
    },
    /// Delete a registry value.
    RegistryDelete {
        /// Root key.
        hive: RegistryHive,
        /// Subkey path below the hive.
        subkey: String,
        /// Value name; empty for the key's default value.
        name: String,
        /// Registry view.
        view: RegistryView,
    },
    /// Change a service's start type.
    Service {
        /// Service name (not display name).
        service: String,
        /// The start type to set.
        start_type: ServiceStartType,
    },
    /// Run a command. The revert command is mandatory: everything Volt
    /// applies must be undoable.
    Command {
        /// Exits `0` when the effect is in place and `1` when it is not. Must
        /// not change anything.
        detect: Command,
        /// Makes the change.
        apply: Command,
        /// Undoes the change.
        revert: Command,
    },
}

impl TweakStep {
    /// The operation that carries this step out through the executor.
    #[must_use]
    pub fn operation(&self) -> Operation {
        match self {
            Self::RegistrySet {
                hive,
                subkey,
                name,
                view,
                value,
            } => Operation::SetRegistryValue {
                target: RegistryValueRef::new(*hive, subkey.clone(), name.clone()).with_view(*view),
                value: value.clone(),
            },
            Self::RegistryDelete {
                hive,
                subkey,
                name,
                view,
            } => Operation::DeleteRegistryValue {
                target: RegistryValueRef::new(*hive, subkey.clone(), name.clone()).with_view(*view),
            },
            Self::Service {
                service,
                start_type,
            } => Operation::SetServiceStartType {
                service: service.clone(),
                start_type: *start_type,
            },
            Self::Command {
                detect,
                apply,
                revert,
            } => Operation::RunCommand {
                detect: detect.clone(),
                apply: apply.clone(),
                revert: revert.clone(),
            },
        }
    }
}

/// A tweak: what it is, how risky it is, how well its benefit is known, and
/// the ordered steps that make it.
///
/// Only [`Catalogue::from_toml`](crate::tweak::Catalogue::from_toml) creates
/// tweaks, and it validates every one, so a `Tweak` obtained from a catalogue
/// always has a unique well-formed id, at least one step and a revert for
/// every command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Tweak {
    /// Stable identifier: lower-case letters, digits and `-`. It labels the
    /// tweak's journal batches, so it must never change once released.
    pub id: String,
    /// Short name shown in the UI.
    pub title: String,
    /// What the tweak does and why.
    pub description: String,
    /// The group it is listed under.
    pub category: Category,
    /// How dangerous the change is.
    pub risk: Risk,
    /// Whether the benefit was measured or is anecdotal.
    pub evidence: Evidence,
    /// A short note saying where the evidence comes from.
    pub evidence_note: String,
    /// Whether the user has to restart or sign out afterwards.
    pub restart: RestartRequirement,
    /// The changes, in order.
    pub steps: Vec<TweakStep>,
}
