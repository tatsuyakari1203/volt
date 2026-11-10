//! [`Processes`] port on the tool help snapshot API.
//!
//! Listing reads the names of the running processes and changes nothing. The
//! working set is not part of a snapshot, so it is reported as 0. Ending a
//! process is not implemented: it is [`PortError::Unsupported`].

use volt_core::{PortError, ProcessInfo, Processes};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};

use crate::error::from_win32;

/// The processes port backed by a tool help snapshot.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinProcesses;

impl WinProcesses {
    /// A processes adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// A snapshot handle, closed on drop.
struct Snapshot(HANDLE);

impl Drop for Snapshot {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful CreateToolhelp32Snapshot
        // call, is owned exclusively by this value and is closed once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The executable name of an entry: the text before the first NUL.
fn entry_name(entry: &PROCESSENTRY32W) -> String {
    let end = entry
        .szExeFile
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(entry.szExeFile.len());
    String::from_utf16_lossy(&entry.szExeFile[..end])
}

impl Processes for WinProcesses {
    fn list(&self) -> Result<Vec<ProcessInfo>, PortError> {
        // SAFETY: plain call with constant arguments; the returned handle is
        // wrapped at once so it is closed on every path.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map(Snapshot)
            .map_err(|e| from_win32(win32_code(&e), "listing processes"))?;
        let mut entry = PROCESSENTRY32W {
            // The size field is what tells the API which structure it gets.
            dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(0),
            ..PROCESSENTRY32W::default()
        };
        let mut processes = Vec::new();
        // SAFETY: `snapshot.0` is a valid snapshot handle and `entry` is a
        // properly sized PROCESSENTRY32W that outlives the call.
        let mut more = unsafe { Process32FirstW(snapshot.0, &raw mut entry) }.is_ok();
        while more {
            processes.push(ProcessInfo {
                pid: entry.th32ProcessID,
                name: entry_name(&entry),
                working_set_bytes: 0,
            });
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.0, &raw mut entry) }.is_ok();
        }
        Ok(processes)
    }

    fn terminate(&self, _pid: u32) -> Result<(), PortError> {
        Err(PortError::Unsupported(
            "ending a process is not implemented".to_owned(),
        ))
    }
}

/// The Win32 code inside a `windows` error (its HRESULT's low word).
fn win32_code(error: &windows::core::Error) -> u32 {
    // `HRESULT_FROM_WIN32` keeps the code in the low 16 bits.
    u32::try_from(error.code().0 & 0xFFFF).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_lists_this_test_process() {
        let me = std::process::id();
        let list = WinProcesses::new().list().expect("list processes");
        assert!(list.iter().any(|p| p.pid == me && !p.name.is_empty()));
    }
}
