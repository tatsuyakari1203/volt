//! Ports that have no Windows adapter yet.
//!
//! The processes port is part of [`volt_core::Ports`] because the core models it, but `volt-win`
//! does not implement it yet (the memory port has its adapter, `WinMemory`). The composition root still has to
//! hand the executor a complete bundle, so it passes these small adapters. Every call fails with
//! [`PortError::Unsupported`] and says why, which the UI shows as "not supported". Nothing here
//! touches the system.
//!
//! The one tweak in the catalogue today is a registry value, so this is not reached.

use volt_core::{PortError, ProcessChange, ProcessInfo, ProcessSettings, Processes};

fn unsupported(what: &str) -> PortError {
    PortError::Unsupported(format!("{what} has no Windows adapter yet"))
}

/// The processes port without an adapter: listing and ending processes are unsupported.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedProcesses;

impl Processes for UnsupportedProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        Err(unsupported("listing processes"))
    }

    fn terminate(&self, _pid: u32, _created_at_100ns: u64) -> Result<(), PortError> {
        Err(unsupported("ending a process"))
    }

    fn settings(&self, _pid: u32, _created_at_100ns: u64) -> Result<ProcessSettings, PortError> {
        Err(unsupported("reading process settings"))
    }

    fn apply_settings(
        &self,
        _pid: u32,
        _created_at_100ns: u64,
        _change: &ProcessChange,
    ) -> Result<(), PortError> {
        Err(unsupported("changing process settings"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_is_unsupported_and_changes_nothing() {
        let is_unsupported = |error: &PortError| matches!(error, PortError::Unsupported(_));
        assert!(
            UnsupportedProcesses
                .list()
                .is_err_and(|e| is_unsupported(&e))
        );
        assert!(
            UnsupportedProcesses
                .terminate(1, 0)
                .is_err_and(|e| is_unsupported(&e))
        );
    }
}
