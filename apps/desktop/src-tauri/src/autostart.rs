//! Start with Windows: optional, off by default, and never elevated.
//!
//! This is the unelevated path only: a per-user entry in
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which needs no administrator rights and
//! shows no UAC prompt. The elevation model is not decided yet; an elevated build could not use
//! this entry (Windows does not start elevated programs from the Run key without a prompt) and
//! would need a scheduled task with highest privileges instead. See docs/architecture.md.
//!
//! The logic lives behind the small [`Autostart`] trait so it is tested against a fake. The real
//! adapter, [`PluginAutostart`], wraps `tauri-plugin-autostart` and is the only code that writes
//! the Run key. No test, script or ad-hoc run may call it on a developer machine.

use std::fmt;

use tauri::AppHandle;
use tauri::Runtime;
use tauri::plugin::TauriPlugin;
use tauri_plugin_autostart::ManagerExt as _;

use crate::IpcError;
use crate::window::BACKGROUND_ARG;

/// The platform's start-at-login switch.
pub(crate) trait Autostart {
    /// Whether Volt is registered to start at login.
    fn is_enabled(&self) -> Result<bool, AutostartError>;
    /// Registers Volt to start at login.
    fn enable(&self) -> Result<(), AutostartError>;
    /// Removes the registration.
    fn disable(&self) -> Result<(), AutostartError>;
}

/// A failure reading or changing the start-at-login registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AutostartError(pub String);

impl fmt::Display for AutostartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "start with Windows: {}", self.0)
    }
}

impl std::error::Error for AutostartError {}

impl From<AutostartError> for IpcError {
    fn from(error: AutostartError) -> Self {
        Self::Other {
            message: error.to_string(),
        }
    }
}

/// Reads whether start with Windows is on.
pub(crate) fn current_state(port: &dyn Autostart) -> Result<bool, AutostartError> {
    port.is_enabled()
}

/// Turns start with Windows on or off and returns the state read back afterwards, so the UI
/// shows what is true and not what was asked. Idempotent: the registration is only touched when
/// it differs from the request, so switching on twice writes once.
pub(crate) fn set_state(port: &dyn Autostart, enabled: bool) -> Result<bool, AutostartError> {
    if port.is_enabled()? != enabled {
        if enabled {
            port.enable()?;
        } else {
            port.disable()?;
        }
    }
    port.is_enabled()
}

/// The real adapter, over `tauri-plugin-autostart`. Writes the per-user Run entry.
#[derive(Debug)]
pub(crate) struct PluginAutostart<'a, R: Runtime>(&'a AppHandle<R>);

impl<R: Runtime> Autostart for PluginAutostart<'_, R> {
    fn is_enabled(&self) -> Result<bool, AutostartError> {
        self.0
            .autolaunch()
            .is_enabled()
            .map_err(|error| AutostartError(error.to_string()))
    }

    fn enable(&self) -> Result<(), AutostartError> {
        self.0
            .autolaunch()
            .enable()
            .map_err(|error| AutostartError(error.to_string()))
    }

    fn disable(&self) -> Result<(), AutostartError> {
        self.0
            .autolaunch()
            .disable()
            .map_err(|error| AutostartError(error.to_string()))
    }
}

/// The plugin, configured so a login launch starts in the tray (see [`BACKGROUND_ARG`]).
///
/// Its own JavaScript commands stay unreachable: no capability grants `autostart:*`, and the UI
/// goes through the typed commands below instead.
#[must_use]
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_autostart::Builder::new()
        .arg(BACKGROUND_ARG)
        .build()
}

/// Tauri command: whether Volt starts with Windows. Reads the Run key, changes nothing.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects `AppHandle` by value; commands cannot borrow it"
)]
pub(crate) fn autostart_enabled(app: AppHandle) -> Result<bool, IpcError> {
    Ok(current_state(&PluginAutostart(&app))?)
}

/// Tauri command: turns Volt starting with Windows on or off and returns the resulting state.
/// Per-user, so it never asks for elevation.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::needless_pass_by_value,
    reason = "Tauri injects `AppHandle` by value; commands cannot borrow it"
)]
pub(crate) fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, IpcError> {
    Ok(set_state(&PluginAutostart(&app), enabled)?)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// An in-memory registration that records what was asked of it.
    #[derive(Default)]
    struct FakeAutostart {
        on: Cell<bool>,
        calls: RefCell<Vec<&'static str>>,
        fail_with: RefCell<Option<String>>,
    }

    impl FakeAutostart {
        fn failing(message: &str) -> Self {
            let fake = Self::default();
            *fake.fail_with.borrow_mut() = Some(message.to_owned());
            fake
        }

        fn check(&self) -> Result<(), AutostartError> {
            match self.fail_with.borrow().clone() {
                Some(message) => Err(AutostartError(message)),
                None => Ok(()),
            }
        }
    }

    impl Autostart for FakeAutostart {
        fn is_enabled(&self) -> Result<bool, AutostartError> {
            self.check()?;
            Ok(self.on.get())
        }

        fn enable(&self) -> Result<(), AutostartError> {
            self.check()?;
            self.calls.borrow_mut().push("enable");
            self.on.set(true);
            Ok(())
        }

        fn disable(&self) -> Result<(), AutostartError> {
            self.check()?;
            self.calls.borrow_mut().push("disable");
            self.on.set(false);
            Ok(())
        }
    }

    #[test]
    fn it_is_off_by_default_and_reading_changes_nothing() {
        let fake = FakeAutostart::default();
        assert_eq!(current_state(&fake), Ok(false));
        assert!(fake.calls.borrow().is_empty());
    }

    #[test]
    fn turning_it_on_registers_once_and_reports_the_new_state() {
        let fake = FakeAutostart::default();
        assert_eq!(set_state(&fake, true), Ok(true));
        assert_eq!(*fake.calls.borrow(), ["enable"]);
        assert_eq!(current_state(&fake), Ok(true));
    }

    #[test]
    fn turning_it_off_removes_the_registration() {
        let fake = FakeAutostart::default();
        fake.on.set(true);
        assert_eq!(set_state(&fake, false), Ok(false));
        assert_eq!(*fake.calls.borrow(), ["disable"]);
    }

    #[test]
    fn asking_for_the_current_state_touches_nothing() {
        let off = FakeAutostart::default();
        assert_eq!(set_state(&off, false), Ok(false));
        let on = FakeAutostart::default();
        on.on.set(true);
        assert_eq!(set_state(&on, true), Ok(true));
        assert!(off.calls.borrow().is_empty());
        assert!(on.calls.borrow().is_empty());
    }

    #[test]
    fn a_failure_is_reported_and_nothing_is_claimed() {
        let fake = FakeAutostart::failing("registry unavailable");
        let error = set_state(&fake, true).expect_err("the fake fails");
        assert_eq!(error, AutostartError("registry unavailable".into()));
        assert!(fake.calls.borrow().is_empty());
        assert_eq!(
            IpcError::from(error),
            IpcError::Other {
                message: "start with Windows: registry unavailable".into()
            }
        );
    }
}
