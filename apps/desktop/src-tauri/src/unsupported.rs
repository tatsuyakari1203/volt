//! Ports that have no Windows adapter yet.
//!
//! The processes and memory ports are part of [`volt_core::Ports`] because the core models them,
//! but `volt-win` does not implement them yet. The composition root still has to
//! hand the executor a complete bundle, so it passes these small adapters. Every call fails with
//! [`PortError::Unsupported`] and says why, which the UI shows as "not supported". Nothing here
//! touches the system.
//!
//! The one tweak in the catalogue today is a registry value, so none of these are reached.

use volt_core::{Memory, MemoryArea, MemoryStatus, PortError, ProcessInfo, Processes};

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

    fn terminate(&self, _pid: u32) -> Result<(), PortError> {
        Err(unsupported("ending a process"))
    }
}

/// The memory port without an adapter: reading memory totals and purging are unsupported.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedMemory;

impl Memory for UnsupportedMemory {
    fn status(&self) -> Result<MemoryStatus, PortError> {
        Err(unsupported("reading memory totals"))
    }

    fn purge(&self, _area: MemoryArea) -> Result<(), PortError> {
        Err(unsupported("purging memory"))
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
                .terminate(1)
                .is_err_and(|e| is_unsupported(&e))
        );
        assert!(
            UnsupportedMemory
                .status()
                .is_err_and(|e| is_unsupported(&e))
        );
        assert!(
            UnsupportedMemory
                .purge(MemoryArea::WorkingSets)
                .is_err_and(|e| is_unsupported(&e))
        );
    }
}
