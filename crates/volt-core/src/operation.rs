//! Operations: declarative, undoable changes.
//!
//! An [`Operation`] is plain data. `detect` is public and read-only. Applying
//! and reverting are crate-private on purpose: they must only run through the
//! [`Executor`](crate::Executor), which journals the prior state first.

use serde::{Deserialize, Serialize};

use crate::command::{Command, run_detect, run_expecting_success};
use crate::error::PortError;
use crate::ports::Ports;
use crate::registry::{RegistryValue, RegistryValueRef};
use crate::service::ServiceStartType;

/// One undoable change to the system.
///
/// File deletion and memory purges are intentionally not operations: they are
/// not undoable and have their own ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Operation {
    /// Write a registry value.
    SetRegistryValue {
        /// Which value.
        target: RegistryValueRef,
        /// The data to write.
        value: RegistryValue,
    },
    /// Delete a registry value. Deleting a value that is already absent is a no-op.
    DeleteRegistryValue {
        /// Which value.
        target: RegistryValueRef,
    },
    /// Change a service's start type.
    SetServiceStartType {
        /// Service name (not display name).
        service: String,
        /// The new start type.
        start_type: ServiceStartType,
    },
    /// Run an external command, with the commands that detect and undo it.
    ///
    /// Everything Volt applies must be undoable, so a revert command is part
    /// of the operation. Exit codes: `detect` exits `0` when the effect is in
    /// place and `1` when it is not (anything else is an error); `apply` and
    /// `revert` exit `0` on success. A detect command must not change anything;
    /// Volt cannot enforce that, so a catalogue reviewer has to.
    RunCommand {
        /// Reports whether the effect is in place. Read-only by contract.
        detect: Command,
        /// Makes the change.
        apply: Command,
        /// Undoes the change. Should be safe to run when nothing was changed.
        revert: Command,
    },
}

/// The state of what an operation touches, captured before the change.
///
/// Revert restores exactly this, including "the value did not exist".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PriorState {
    /// A registry value; `None` means it did not exist.
    RegistryValue(Option<RegistryValue>),
    /// The start type of a service.
    ServiceStartType(ServiceStartType),
    /// Whether the effect of a command was in place (its detect command said
    /// so).
    CommandState(bool),
}

impl Operation {
    /// Reads the current state of what this operation would change. Read-only
    /// (for a command: as read-only as its detect command).
    pub fn detect(&self, ports: &Ports) -> Result<PriorState, PortError> {
        match self {
            Self::SetRegistryValue { target, .. } | Self::DeleteRegistryValue { target } => Ok(
                PriorState::RegistryValue(ports.registry.read_value(target)?),
            ),
            Self::SetServiceStartType { service, .. } => Ok(PriorState::ServiceStartType(
                ports.services.start_type(service)?,
            )),
            Self::RunCommand { detect, .. } => {
                Ok(PriorState::CommandState(run_detect(ports, detect)?))
            }
        }
    }

    /// The state the system is in after this operation has been applied.
    pub(crate) fn target_state(&self) -> PriorState {
        match self {
            Self::SetRegistryValue { value, .. } => PriorState::RegistryValue(Some(value.clone())),
            Self::DeleteRegistryValue { .. } => PriorState::RegistryValue(None),
            Self::SetServiceStartType { start_type, .. } => {
                PriorState::ServiceStartType(*start_type)
            }
            Self::RunCommand { .. } => PriorState::CommandState(true),
        }
    }

    /// Mutates the system. Only the executor calls this, after journaling.
    pub(crate) fn apply(&self, ports: &Ports) -> Result<(), PortError> {
        match self {
            Self::SetRegistryValue { target, value } => ports.registry.write_value(target, value),
            Self::DeleteRegistryValue { target } => {
                ignore_not_found(ports.registry.delete_value(target))
            }
            Self::SetServiceStartType {
                service,
                start_type,
            } => ports.services.set_start_type(service, *start_type),
            Self::RunCommand { apply, .. } => run_expecting_success(ports, apply),
        }
    }

    /// Restores `prior`. Only the executor calls this.
    pub(crate) fn revert(&self, prior: &PriorState, ports: &Ports) -> Result<(), PortError> {
        match (self, prior) {
            (
                Self::SetRegistryValue { target, .. } | Self::DeleteRegistryValue { target },
                PriorState::RegistryValue(previous),
            ) => match previous {
                Some(value) => ports.registry.write_value(target, value),
                None => ignore_not_found(ports.registry.delete_value(target)),
            },
            (Self::SetServiceStartType { service, .. }, PriorState::ServiceStartType(previous)) => {
                ports.services.set_start_type(service, *previous)
            }
            // The effect was already in place before apply: there is nothing
            // of ours to undo, and running the revert command would remove it.
            (Self::RunCommand { .. }, PriorState::CommandState(true)) => Ok(()),
            (Self::RunCommand { revert, .. }, PriorState::CommandState(false)) => {
                run_expecting_success(ports, revert)
            }
            _ => Err(PortError::InvalidInput(
                "journal prior state does not match the operation".to_owned(),
            )),
        }
    }

    /// Human-readable description of the target and the current and wanted
    /// values, used by previews: `(subject, target value text)`.
    pub(crate) fn describe_target(&self) -> (String, String) {
        match self {
            Self::SetRegistryValue { target, value } => (target.to_string(), value.to_string()),
            Self::DeleteRegistryValue { target } => (target.to_string(), "(not set)".to_owned()),
            Self::SetServiceStartType {
                service,
                start_type,
            } => (
                format!("service {service} start type"),
                start_type.to_string(),
            ),
            Self::RunCommand { apply, .. } => (format!("command {apply}"), "applied".to_owned()),
        }
    }
}

impl PriorState {
    /// Human-readable form, used by previews.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::RegistryValue(Some(value)) => value.to_string(),
            Self::RegistryValue(None) => "(not set)".to_owned(),
            Self::ServiceStartType(start_type) => start_type.to_string(),
            Self::CommandState(true) => "applied".to_owned(),
            Self::CommandState(false) => "not applied".to_owned(),
        }
    }
}

fn ignore_not_found(result: Result<(), PortError>) -> Result<(), PortError> {
    match result {
        Err(PortError::NotFound(_)) => Ok(()),
        other => other,
    }
}
