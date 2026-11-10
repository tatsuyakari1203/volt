//! [`RecycleBin`] port on the Windows shell.

use volt_core::{PortError, RecycleBin, RecycleBinStatus};
use windows::Win32::UI::Shell::{
    SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SHQUERYRBINFO,
    SHQueryRecycleBinW,
};
use windows::core::PCWSTR;

use crate::error::from_hresult;

/// The Recycle Bin of the current user, over every drive.
///
/// [`status`](RecycleBin::status) only reads. [`empty`](RecycleBin::empty) deletes
/// permanently and must be called only after the user confirmed it; no test here
/// calls it on a real machine.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRecycleBin;

impl WinRecycleBin {
    /// An adapter over the shell's Recycle Bin.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl RecycleBin for WinRecycleBin {
    fn status(&self) -> Result<RecycleBinStatus, PortError> {
        let mut info = SHQUERYRBINFO {
            cbSize: u32::try_from(std::mem::size_of::<SHQUERYRBINFO>()).unwrap_or(0),
            ..SHQUERYRBINFO::default()
        };
        // SAFETY: a null root path asks for every drive; `info` is a valid,
        // writable SHQUERYRBINFO whose `cbSize` is set, as the call requires.
        unsafe { SHQueryRecycleBinW(PCWSTR::null(), &raw mut info) }
            .map_err(|e| from_hresult(e.code().0, "the Recycle Bin"))?;
        Ok(RecycleBinStatus {
            item_count: u64::try_from(info.i64NumItems).unwrap_or(0),
            size_bytes: u64::try_from(info.i64Size).unwrap_or(0),
        })
    }

    fn empty(&self) -> Result<(), PortError> {
        // SAFETY: no owner window and a null root path (every drive); the flags
        // only suppress the shell's own confirmation, progress and sound, because
        // Volt asks the user first.
        unsafe {
            SHEmptyRecycleBinW(
                None,
                PCWSTR::null(),
                SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND,
            )
        }
        .map_err(|e| from_hresult(e.code().0, "emptying the Recycle Bin"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: the real Recycle Bin can be queried. Never calls `empty`.
    #[test]
    fn the_status_of_the_real_recycle_bin_can_be_read() {
        let status = WinRecycleBin::new().status().expect("status");
        // Any count is fine; an empty bin must not report a size.
        if status.item_count == 0 {
            assert_eq!(status.size_bytes, 0);
        }
    }
}
