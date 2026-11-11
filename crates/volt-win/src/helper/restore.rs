//! System restore points, through `SRSetRestorePointW`.
//!
//! The call is the supported way for a program to ask Windows for a restore point. It is a pair:
//! `BEGIN_SYSTEM_CHANGE` creates the point, `END_SYSTEM_CHANGE` closes it. Windows decides the rest
//! and a program cannot see it: System Protection must be on for the drive, and by default Windows
//! creates at most one point per 24 hours (it skips the others without an error).
//!
//! This is the only place that makes the call, and only the helper calls this, only after the
//! confirmation of a Caution or Advanced administrator tweak.
//!
//! The call can take long (it waits for the Volume Shadow Copy service). It runs on its own thread
//! and the caller waits at most [`volt_core::RESTORE_POINT_TIMEOUT`]: a point that is not made in
//! that time counts as not made, so the tweak is refused inside the time the UI waits for the
//! answer. The abandoned call finishes by itself.

use std::sync::mpsc;
use std::time::Duration;

use volt_core::{PortError, RESTORE_POINT_TIMEOUT, RestorePoints};
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
        let description = description.to_owned();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("volt-restore-point".to_owned())
            .spawn(move || {
                let _ = done_tx.send(make_point(&description));
            })
            .map_err(|error| PortError::Other(format!("the restore point thread: {error}")))?;
        wait_for(&done_rx, RESTORE_POINT_TIMEOUT)
    }
}

/// The result of the call, or a timeout error when it does not come in `limit`.
fn wait_for(
    done: &mpsc::Receiver<Result<(), PortError>>,
    limit: Duration,
) -> Result<(), PortError> {
    done.recv_timeout(limit).unwrap_or_else(|_| {
        Err(PortError::Other(
            "Windows did not answer in time".to_owned(),
        ))
    })
}

fn make_point(description: &str) -> Result<(), PortError> {
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_that_does_not_answer_in_time_is_a_failure_not_a_wait_forever() {
        let (_keep_open, receiver) = mpsc::channel::<Result<(), PortError>>();
        let started = std::time::Instant::now();
        let result = wait_for(&receiver, Duration::from_millis(50));
        assert!(matches!(result, Err(PortError::Other(_))));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn an_answer_in_time_is_passed_on() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(Err(PortError::Unsupported("off".to_owned())))
            .ok();
        assert_eq!(
            wait_for(&receiver, Duration::from_secs(5)),
            Err(PortError::Unsupported("off".to_owned()))
        );
    }
}
