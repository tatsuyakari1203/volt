//! What the UI process needs to know before it decides whether to start while elevated: whether
//! UAC is on, whether the user is the built-in Administrator, and a native message to say no.
//!
//! Read-only apart from the message box, which only informs.

use volt_core::{Registry, RegistryHive, RegistryValue, RegistryValueRef};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW,
};
use windows::core::PCWSTR;

use super::identity::inspect_current;
use super::wide;
use crate::WinRegistry;

const SYSTEM_POLICIES: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System";

/// Whether the system-wide UAC switch (`EnableLUA`) is on. `None` when it cannot be read.
#[must_use]
pub fn uac_enabled() -> Option<bool> {
    let target = RegistryValueRef::new(RegistryHive::LocalMachine, SYSTEM_POLICIES, "EnableLUA");
    match WinRegistry::new().read_value(&target) {
        Ok(Some(RegistryValue::Dword(value))) => Some(value != 0),
        // The value is missing: Windows treats that as "on".
        Ok(None) => Some(true),
        _ => None,
    }
}

/// Whether `sid` is the built-in Administrator account (relative id 500).
#[must_use]
pub fn is_built_in_administrator_sid(sid: &str) -> bool {
    sid.starts_with("S-1-5-21-") && sid.ends_with("-500")
}

/// Whether this process's user is the built-in Administrator. `false` when it cannot be read.
#[must_use]
pub fn current_user_is_built_in_administrator() -> bool {
    inspect_current().is_ok_and(|me| is_built_in_administrator_sid(&me.token.user_sid))
}

/// Shows a modal warning with an OK button and returns when it is closed.
pub fn show_message(title: &str, text: &str) {
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both buffers are NUL-terminated UTF-16 that outlive the call; a null owner window
    // is allowed.
    let _ = unsafe {
        MessageBoxW(
            Some(HWND::default()),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_domain_or_machine_sid_ending_in_500_is_the_built_in_administrator() {
        assert!(is_built_in_administrator_sid("S-1-5-21-1-2-3-500"));
        assert!(!is_built_in_administrator_sid("S-1-5-21-1-2-3-1001"));
        assert!(!is_built_in_administrator_sid("S-1-5-21-1-2-3-5000"));
        assert!(!is_built_in_administrator_sid("S-1-5-18"));
    }

    #[test]
    fn the_uac_switch_can_be_read() {
        // Read-only; the value depends on the machine, so only that it answers is checked.
        assert!(uac_enabled().is_some());
    }
}
