//! Pending restart: the three markers Windows leaves when it waits for a restart. Read-only.

use crate::error::PortError;
use crate::registry::{Registry, RegistryHive, RegistryValue, RegistryValueRef, RegistryView};

/// Servicing stack (updates and optional features) waits for a restart: the key exists.
const COMPONENT_SERVICING_KEY: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending";
/// Windows Update waits for a restart: the key exists.
const WINDOWS_UPDATE_KEY: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired";
/// Subkey of the session manager, which holds the list of files to rename or delete at boot.
const SESSION_MANAGER_KEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Session Manager";
/// Installers queue file replacements here when a file is in use: a non-empty list means
/// something finishes at the next start.
const PENDING_FILE_RENAMES_VALUE: &str = "PendingFileRenameOperations";

/// Which well-known marker says that Windows waits for a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RestartMarker {
    /// `HKLM\...\Component Based Servicing\RebootPending` exists.
    ComponentServicing,
    /// `HKLM\...\WindowsUpdate\Auto Update\RebootRequired` exists.
    WindowsUpdate,
    /// `HKLM\...\Session Manager\PendingFileRenameOperations` holds at least one entry.
    PendingFileRename,
}

/// The markers that currently say a restart is pending, empty when none does. Read-only: it
/// only asks the registry whether keys and a value exist.
///
/// All three are checked. When one of them cannot be read but another is set, the answer is
/// still "pending" (positive evidence stands). When none is set and one could not be read, the
/// result is the error: "could not tell" must not read as "no restart needed".
pub fn pending_restart_markers(registry: &dyn Registry) -> Result<Vec<RestartMarker>, PortError> {
    let mut found = Vec::new();
    let mut failure = None;
    let mut note = |result: Result<bool, PortError>, marker: RestartMarker| match result {
        Ok(true) => found.push(marker),
        Ok(false) => {}
        Err(error) => {
            failure.get_or_insert(error);
        }
    };

    note(
        registry.key_exists(
            RegistryHive::LocalMachine,
            COMPONENT_SERVICING_KEY,
            RegistryView::Native,
        ),
        RestartMarker::ComponentServicing,
    );
    note(
        registry.key_exists(
            RegistryHive::LocalMachine,
            WINDOWS_UPDATE_KEY,
            RegistryView::Native,
        ),
        RestartMarker::WindowsUpdate,
    );
    let renames = RegistryValueRef::new(
        RegistryHive::LocalMachine,
        SESSION_MANAGER_KEY,
        PENDING_FILE_RENAMES_VALUE,
    );
    note(
        registry.read_value(&renames).map(|value| match value {
            Some(RegistryValue::MultiSz(entries)) => !entries.is_empty(),
            // Another kind in this value is not what Windows writes; it queues nothing.
            _ => false,
        }),
        RestartMarker::PendingFileRename,
    );

    match failure {
        Some(error) if found.is_empty() => Err(error),
        _ => Ok(found),
    }
}
