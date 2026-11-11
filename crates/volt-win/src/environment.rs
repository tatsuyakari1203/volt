//! [`Environment`] on the Windows API: the three facts automation pauses on.
//!
//! - **Fullscreen app or presentation**: `SHQueryUserNotificationState`, the signal Windows itself
//!   uses to hold back notifications. It reports a presentation (Presentation Settings), a
//!   Direct3D exclusive fullscreen app, a fullscreen window ("busy") and a fullscreen Store app.
//!   What it misses: a game or video in a borderless window that Windows does not count as
//!   fullscreen, a fullscreen app on a second monitor while the first one is in use, and Focus
//!   Assist "quiet time" (not treated as a pause on purpose: it is a notification preference, not
//!   a sign that the user is busy with something heavy).
//! - **Battery saver**: `GetSystemPowerStatus`, whose status flag is 1 while it is on.
//! - **Time zone**: `GetTimeZoneInformation`, the bias in minutes (daylight saving included).
//!
//! A call that fails reports "no": automation then behaves as if nothing paused it, which is what
//! it does on a machine with no such signal.

use volt_core::Environment;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
use windows::Win32::UI::Shell::{
    QUNS_APP, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
    SHQueryUserNotificationState,
};

/// `TIME_ZONE_ID_DAYLIGHT`: daylight saving time is in effect.
const TIME_ZONE_ID_DAYLIGHT: u32 = 2;

/// Reads the fullscreen, battery saver and time zone facts from Windows.
#[derive(Debug, Clone, Copy, Default)]
pub struct WinEnvironment;

impl WinEnvironment {
    /// A reader. It holds no state.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl Environment for WinEnvironment {
    fn fullscreen_app_active(&self) -> bool {
        // SAFETY: no arguments; the call only reads the shell's notification state.
        match unsafe { SHQueryUserNotificationState() } {
            Ok(state) => matches!(
                state,
                QUNS_BUSY | QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE | QUNS_APP
            ),
            Err(_) => false,
        }
    }

    fn battery_saver_on(&self) -> bool {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: `status` is a valid, writable SYSTEM_POWER_STATUS for the duration of the call.
        match unsafe { GetSystemPowerStatus(&raw mut status) } {
            Ok(()) => status.SystemStatusFlag == 1,
            Err(_) => false,
        }
    }

    fn utc_offset_minutes(&self) -> i32 {
        let mut zone = TIME_ZONE_INFORMATION::default();
        // SAFETY: `zone` is a valid, writable TIME_ZONE_INFORMATION for the duration of the call.
        let id = unsafe { GetTimeZoneInformation(&raw mut zone) };
        // The bias is UTC minus local time. The extra bias is the one in effect now.
        let extra = if id == TIME_ZONE_ID_DAYLIGHT {
            zone.DaylightBias
        } else {
            zone.StandardBias
        };
        -(zone.Bias + extra)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readers_answer_without_failing() {
        // The values depend on the machine; what matters is that the calls work and the offset is
        // a real one (zones run from UTC-12 to UTC+14).
        let environment = WinEnvironment::new();
        let _ = environment.fullscreen_app_active();
        let _ = environment.battery_saver_on();
        assert!((-12 * 60..=14 * 60).contains(&environment.utc_offset_minutes()));
    }
}
