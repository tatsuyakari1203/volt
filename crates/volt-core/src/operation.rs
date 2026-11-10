//! Operations: declarative, undoable changes.
//!
//! An [`Operation`] is plain data. `detect` is public and read-only. Applying
//! and reverting are crate-private on purpose: they must only run through the
//! [`Executor`](crate::Executor), which journals the prior state first.

use serde::{Deserialize, Serialize};

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
}

impl Operation {
    /// Reads the current state of what this operation would change. Read-only.
    pub fn detect(&self, ports: &Ports) -> Result<PriorState, PortError> {
        match self {
            Self::SetRegistryValue { target, .. } | Self::DeleteRegistryValue { target } => Ok(
                PriorState::RegistryValue(ports.registry.read_value(target)?),
            ),
            Self::SetServiceStartType { service, .. } => Ok(PriorState::ServiceStartType(
                ports.services.start_type(service)?,
            )),
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
        }
    }
}

fn ignore_not_found(result: Result<(), PortError>) -> Result<(), PortError> {
    match result {
        Err(PortError::NotFound(_)) => Ok(()),
        other => other,
    }
}
