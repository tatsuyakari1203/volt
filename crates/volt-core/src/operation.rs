//! Operations: declarative, undoable changes.
//!
//! An [`Operation`] is plain data. `detect` is public and read-only. Applying
//! and reverting are crate-private on purpose: they must only run through the
//! [`Executor`](crate::Executor), which journals the prior state first.

use serde::{Deserialize, Serialize};

use crate::command::{Command, run_detect, run_expecting_success};
use crate::error::PortError;
use crate::ports::Ports;
use crate::registry::{
    RegistryKeyContents, RegistryKeyRef, RegistryValue, RegistryValueRef, missing_levels,
    read_key_contents, remove_empty_keys,
};
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
    /// Make a registry key exist, creating it and its missing parents. Reverting removes the keys
    /// the apply created, each only while it holds nothing.
    EnsureRegistryKey {
        /// Which key.
        key: RegistryKeyRef,
    },
    /// Delete a registry key with the values in it. Refused for a key that has subkeys or holds
    /// more than Volt can record, so a revert can put the key back exactly.
    DeleteRegistryKey {
        /// Which key.
        key: RegistryKeyRef,
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
        /// Whether the effect is in place after `apply`. `true` for a tweak. A reset turns the
        /// effect off: `apply` is then the command that does so and `revert` the one that puts
        /// the effect back.
        #[serde(default = "effect_after_default")]
        effect_after: bool,
    },
}

const fn effect_after_default() -> bool {
    true
}

/// The state of what an operation touches, captured before the change.
///
/// Revert restores exactly this, including "the value did not exist".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "state", rename_all = "snake_case")]
#[non_exhaustive]
pub enum PriorState {
    /// A registry value; `None` means it did not exist (in a key that did).
    RegistryValue(Option<RegistryValue>),
    /// A registry value that did not exist because its key, and `missing_levels` levels of the
    /// key's path counted from the key upwards, did not exist either. Writing the value creates
    /// them, so a revert removes them again when they are empty.
    RegistryValueInNewKey {
        /// How many levels of the path were missing (at least one).
        missing_levels: u32,
    },
    /// A registry key that did not exist, with the number of missing levels of its path.
    RegistryKeyMissing {
        /// How many levels of the path were missing (at least one).
        missing_levels: u32,
    },
    /// A registry key that existed, with what it held.
    RegistryKeyPresent(RegistryKeyContents),
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
            Self::SetRegistryValue { target, .. } => {
                let value = ports.registry.read_value(target)?;
                if value.is_some() {
                    return Ok(PriorState::RegistryValue(value));
                }
                let key = RegistryKeyRef::new(target.hive, target.subkey.clone(), target.view);
                Ok(match missing_levels(ports.registry.as_ref(), &key)? {
                    0 => PriorState::RegistryValue(None),
                    missing_levels => PriorState::RegistryValueInNewKey { missing_levels },
                })
            }
            Self::DeleteRegistryValue { target } => Ok(PriorState::RegistryValue(
                ports.registry.read_value(target)?,
            )),
            Self::EnsureRegistryKey { key } | Self::DeleteRegistryKey { key } => {
                Ok(match read_key_contents(ports.registry.as_ref(), key)? {
                    Some(contents) => PriorState::RegistryKeyPresent(contents),
                    None => PriorState::RegistryKeyMissing {
                        missing_levels: missing_levels(ports.registry.as_ref(), key)?,
                    },
                })
            }
            Self::SetServiceStartType { service, .. } => Ok(PriorState::ServiceStartType(
                ports.services.start_type(service)?,
            )),
            Self::RunCommand { detect, .. } => {
                Ok(PriorState::CommandState(run_detect(ports, detect)?))
            }
        }
    }

    /// Whether `state`, as read by [`Operation::detect`], is what this operation leaves behind.
    pub(crate) fn is_target(&self, state: &PriorState) -> bool {
        match (self, state) {
            (Self::SetRegistryValue { value, .. }, PriorState::RegistryValue(Some(current))) => {
                current == value
            }
            (Self::DeleteRegistryValue { .. }, PriorState::RegistryValue(None))
            | (Self::EnsureRegistryKey { .. }, PriorState::RegistryKeyPresent(_))
            | (Self::DeleteRegistryKey { .. }, PriorState::RegistryKeyMissing { .. }) => true,
            (Self::SetServiceStartType { start_type, .. }, PriorState::ServiceStartType(now)) => {
                start_type == now
            }
            (Self::RunCommand { effect_after, .. }, PriorState::CommandState(now)) => {
                effect_after == now
            }
            _ => false,
        }
    }

    /// Mutates the system. Only the executor calls this, after journaling.
    pub(crate) fn apply(&self, ports: &Ports) -> Result<(), PortError> {
        match self {
            Self::SetRegistryValue { target, value } => ports.registry.write_value(target, value),
            Self::DeleteRegistryValue { target } => {
                ignore_not_found(ports.registry.delete_value(target))
            }
            Self::EnsureRegistryKey { key } => ports.registry.create_key(key),
            Self::DeleteRegistryKey { key } => {
                // Read again: the journal holds what the key held a moment ago, and a key that
                // grew since must not be deleted without a record of it.
                match read_key_contents(ports.registry.as_ref(), key)? {
                    None => Ok(()),
                    Some(contents) if !contents.complete => Err(PortError::InvalidInput(format!(
                        "{key} has subkeys or more data than Volt can record, so it is not deleted"
                    ))),
                    Some(_) => ignore_not_found(ports.registry.delete_key(key)),
                }
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
        let registry = ports.registry.as_ref();
        match (self, prior) {
            (
                Self::SetRegistryValue { target, .. } | Self::DeleteRegistryValue { target },
                PriorState::RegistryValue(previous),
            ) => match previous {
                Some(value) => registry.write_value(target, value),
                None => ignore_not_found(registry.delete_value(target)),
            },
            (
                Self::SetRegistryValue { target, .. } | Self::DeleteRegistryValue { target },
                PriorState::RegistryValueInNewKey { missing_levels },
            ) => {
                ignore_not_found(registry.delete_value(target))?;
                let key = RegistryKeyRef::new(target.hive, target.subkey.clone(), target.view);
                remove_empty_keys(registry, &key, *missing_levels)
            }
            // The key was there before the apply: there is nothing of ours to remove.
            (Self::EnsureRegistryKey { .. }, PriorState::RegistryKeyPresent(_))
            | (Self::DeleteRegistryKey { .. }, PriorState::RegistryKeyMissing { .. }) => Ok(()),
            (
                Self::EnsureRegistryKey { key },
                PriorState::RegistryKeyMissing { missing_levels },
            ) => remove_empty_keys(registry, key, *missing_levels),
            (Self::DeleteRegistryKey { key }, PriorState::RegistryKeyPresent(contents)) => {
                registry.create_key(key)?;
                contents
                    .values
                    .iter()
                    .try_for_each(|(name, value)| registry.write_value(&key.value(name), value))
            }
            (Self::SetServiceStartType { service, .. }, PriorState::ServiceStartType(previous)) => {
                ports.services.set_start_type(service, *previous)
            }
            // The effect was already as `apply` leaves it before the apply: there is nothing of
            // ours to undo, and running the revert command would take it away.
            (Self::RunCommand { effect_after, .. }, PriorState::CommandState(before))
                if effect_after == before =>
            {
                Ok(())
            }
            (Self::RunCommand { revert, .. }, PriorState::CommandState(_)) => {
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
            Self::EnsureRegistryKey { key } => (key.to_string(), "key exists".to_owned()),
            Self::DeleteRegistryKey { key } => (key.to_string(), "key removed".to_owned()),
            Self::SetServiceStartType {
                service,
                start_type,
            } => (
                format!("service {service} start type"),
                start_type.to_string(),
            ),
            Self::RunCommand {
                apply,
                effect_after,
                ..
            } => (
                format!("command {apply}"),
                if *effect_after {
                    "applied"
                } else {
                    "not applied"
                }
                .to_owned(),
            ),
        }
    }
}

impl PriorState {
    /// The registry value this state records: `None` when it is not about a registry value,
    /// `Some(None)` when the value did not exist.
    #[must_use]
    pub fn registry_value(&self) -> Option<Option<&RegistryValue>> {
        match self {
            Self::RegistryValue(value) => Some(value.as_ref()),
            Self::RegistryValueInNewKey { .. } => Some(None),
            _ => None,
        }
    }

    /// Human-readable form, used by previews.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::RegistryValue(Some(value)) => value.to_string(),
            Self::RegistryValue(None) | Self::RegistryValueInNewKey { .. } => {
                "(not set)".to_owned()
            }
            Self::RegistryKeyMissing { .. } => "key missing".to_owned(),
            Self::RegistryKeyPresent(contents) => match contents.values.len() {
                0 if contents.complete => "key exists, empty".to_owned(),
                1 => "key exists, 1 value".to_owned(),
                n => format!("key exists, {n} values"),
            },
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
