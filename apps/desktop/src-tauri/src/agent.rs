//! The automation timer: the one thing that wakes Volt up while no window exists.
//!
//! It is a single thread inside the tray process (no service, no scheduled task, no second
//! process). While both automation features are off it sleeps until a setting changes, with no
//! wake-up at all. While one is on it wakes once a minute ([`volt_core::automation::TICK_SECS`]),
//! never at the one-second pace of the live sampler, asks [`crate::automation::tick_of`] whether
//! anything is due and goes back to sleep. A run's result reaches the user the way a tray Boost
//! does: the tooltip, a desktop notification when no window is open (unless the user turned
//! notifications off) and an event for a window that is open.

use std::sync::Mutex;
use std::thread::{self, Thread};
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event as _;
use volt_core::automation::TICK_SECS;

use crate::automation::{self, AutomationUpdated};
use crate::language;
use crate::tray;
use crate::window;
use crate::wiring::AppState;

/// The handle that wakes the timer thread. Lives in Tauri's managed state.
#[derive(Debug, Default)]
pub(crate) struct AutomationTimer {
    thread: Mutex<Option<Thread>>,
}

/// Starts the timer thread. Called once from the app's setup, after the core exists. Without a
/// core there is nothing to look after and no thread is started.
pub(crate) fn start<R: Runtime>(app: &AppHandle<R>) {
    app.manage(AutomationTimer::default());
    if app.state::<AppState>().core().is_err() {
        return;
    }
    let worker = app.clone();
    let spawned = thread::Builder::new()
        .name("volt-automation".to_owned())
        .stack_size(512 * 1024)
        .spawn(move || run(&worker));
    match spawned {
        Ok(handle) => {
            if let Ok(mut slot) = app.state::<AutomationTimer>().thread.lock() {
                *slot = Some(handle.thread().clone());
            }
        }
        Err(error) => log::error!("automation: the timer thread could not start: {error}"),
    }
}

/// Wakes the timer so it looks at the settings now (they just changed).
pub(crate) fn wake<R: Runtime>(app: &AppHandle<R>) {
    if let Some(timer) = app.try_state::<AutomationTimer>()
        && let Ok(slot) = timer.thread.lock()
        && let Some(thread) = slot.as_ref()
    {
        thread.unpark();
    }
}

fn run<R: Runtime>(app: &AppHandle<R>) {
    loop {
        let on = app
            .state::<AppState>()
            .core()
            .ok()
            .and_then(|core| core.automation().ok().map(|a| a.config().any_enabled()))
            .unwrap_or(false);
        if on {
            tick(app);
            thread::park_timeout(Duration::from_secs(TICK_SECS));
        } else {
            // Off: no wake-up until the settings change (`wake`).
            thread::park();
        }
    }
}

fn tick<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>();
    let Ok(core) = state.core() else {
        return;
    };
    let report = match automation::tick_of(&core) {
        Ok(report) => report,
        Err(error) => {
            log::warn!("automation: a tick could not run: {error:?}");
            return;
        }
    };
    if report.memory.is_none() && report.cleaning.is_none() {
        return;
    }
    let lang = language::current(&state);
    if let Some(line) = automation::tooltip_line(&report, lang) {
        tray::set_tooltip(app, line);
    }
    let notify = core.automation().is_ok_and(|a| a.config().notify);
    if notify
        && app.get_webview_window(window::MAIN_WINDOW).is_none()
        && let Some(body) = automation::notification_body(&report, lang)
    {
        // A notification that cannot be shown (disabled in Windows) leaves the tooltip.
        let _ = app.notification().builder().title("Volt").body(body).show();
    }
    if let Ok(status) = automation::automation_status_of(&core) {
        // A failed emit only means nobody is listening.
        let _ = AutomationUpdated(status).emit(app);
    }
}
