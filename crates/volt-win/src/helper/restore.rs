//! System restore points, through `SRSetRestorePointW`.
//!
//! The call is the supported way for a program to ask Windows for a restore point. It is a pair:
//! `BEGIN_SYSTEM_CHANGE` creates the point, `END_SYSTEM_CHANGE` closes it. Windows decides the rest
//! and a program cannot see it: System Protection must be on for the drive, and by default Windows
//! creates at most one point per 24 hours (it skips the others without an error).
//!
//! This is the only place that makes the call, and only the helper calls this, only after the
//! confirmation of a Caution or Advanced administrator tweak.

use volt_core::{PortError, RestorePoints};
use windows::Win32::Foundation::ERROR_SERVICE_DISABLED;
use windows::Win32::System::Restore::{
    BEGIN_SYSTEM_CHANGE, END_SYSTEM_CHANGE, MODIFY_SETTINGS, RESTOREPOINTINFOW, SRSetRestorePointW,
    STATEMGRSTATUS,
};

/// The longest description System Restore keeps (characters, without the terminator).
const DESCRIPTION_MAX: usize = 255;

/// The real [`RestorePoints`].
#[derive(Debug, Default, Clone, Copy)]
pub struct WinRestorePoints;

impl WinRestorePoints {
    /// A port that asks Windows.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn describe(status: &STATEMGRSTATUS) -> PortError {
    // The structure is packed: copy the field before using it.
    let code = { status.nStatus };
    if code == ERROR_SERVICE_DISABLED {
        PortError::Unsupported("System Protection is turned off".to_owned())
    } else {
        PortError::Other(format!("Windows answered {}", code.0))
    }
}

impl RestorePoints for WinRestorePoints {
    fn create(&self, description: &str) -> Result<(), PortError> {
        let mut text = [0_u16; 256];
        for (slot, unit) in text
            .iter_mut()
            .zip(description.encode_utf16().take(DESCRIPTION_MAX))
        {
            *slot = unit;
        }
        let begin = RESTOREPOINTINFOW {
            dwEventType: BEGIN_SYSTEM_CHANGE,
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: 0,
            szDescription: text,
        };
        let mut status = STATEMGRSTATUS::default();
        // SAFETY: both structures are valid for the call, which reads the first and writes the
        // second; the description is NUL-terminated (the buffer was zeroed and at most 255 units
        // were copied into 256).
        let ok = unsafe { SRSetRestorePointW(&raw const begin, &raw mut status) };
        if !ok.as_bool() {
            return Err(describe(&status));
        }
        // Close the change. A failure here leaves the point made; it is only reported.
        let end = RESTOREPOINTINFOW {
            dwEventType: END_SYSTEM_CHANGE,
            dwRestorePtType: MODIFY_SETTINGS,
            llSequenceNumber: { status.llSequenceNumber },
            szDescription: [0; 256],
        };
        let mut end_status = STATEMGRSTATUS::default();
        // SAFETY: as above.
        let closed = unsafe { SRSetRestorePointW(&raw const end, &raw mut end_status) };
        if !closed.as_bool() {
            let code = { end_status.nStatus }.0;
            log::warn!("the restore point change could not be closed: {code}");
        }
        Ok(())
    }
}
