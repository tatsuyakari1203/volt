//! Ports that have no Windows adapter yet.
//!
//! The processes, memory and commands ports are part of [`volt_core::Ports`] because the core
//! models them, but `volt-win` does not implement them yet. The composition root still has to
//! hand the executor a complete bundle, so it passes these small adapters. Every call fails with
//! [`PortError::Unsupported`] and says why, which the UI shows as "not supported". Nothing here
//! touches the system.
//!
//! The one tweak in the catalogue today is a registry value, so none of these are reached. A
//! catalogue tweak with a command step would read its state as unreadable (and could not be
//! applied) until a real commands adapter replaces [`UnsupportedCommands`].

use volt_core::{
    Command, CommandOutput, Commands, Memory, MemoryArea, MemoryStatus, PortError, ProcessInfo,
    Processes,
};

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

/// The commands port without an adapter: running an external program is unsupported.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedCommands;

impl Commands for UnsupportedCommands {
    fn run(&self, _command: &Command) -> Result<CommandOutput, PortError> {
        Err(unsupported("running an external command"))
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
        assert!(
            UnsupportedCommands
                .run(&Command::new("tool", ["--flag"]))
                .is_err_and(|e| is_unsupported(&e))
        );
    }
}
