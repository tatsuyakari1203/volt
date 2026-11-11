//! The automation timer: the one thing that wakes Volt up while no window exists.
//!
//! It is a single thread inside the tray process (no service, no scheduled task, no second
//! process). While both automation features are off and no profile names an app, it sleeps until
//! a setting changes, with no wake-up at all. While a feature is on it wakes once a minute
//! ([`volt_core::automation::TICK_SECS`]), never at the one-second pace of the live sampler, asks
//! [`crate::automation::tick_of`] whether anything is due and goes back to sleep. A run's result
//! reaches the user the way a tray Boost does: the tooltip, a desktop notification when no window
//! is open (unless the user turned notifications off) and an event for a window that is open.
//!
//! While a profile names an app to watch, the same thread also wakes every
//! [`volt_core::profile::TRIGGER_POLL_SECS`] seconds and reads the process list once (the same
//! read the process rules use; nothing is read when no profile watches an app) to see whether
//! that app started or exited. The sampler of the live metrics runs only while a window is open,
//! so it cannot serve a tray that sits without one: this is the one look that does. An app that
//! starts and ends between two looks is missed.

use std::sync::Mutex;
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event as _;
use volt_core::automation::TICK_SECS;
use volt_core::profile::TRIGGER_POLL_SECS;

use crate::automation::{self, AutomationUpdated};
use crate::language;
use crate::profiles;
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
    let mut last_tick: Option<Instant> = None;
    loop {
        let (automation_on, watching) = {
            let state = app.state::<AppState>();
            state.core().map_or((false, false), |core| {
                (
                    core.automation()
                        .is_ok_and(|automation| automation.config().any_enabled()),
                    profiles::watching(&core),
                )
            })
        };
        if !automation_on && !watching {
            // Off: no wake-up until the settings change (`wake`).
            last_tick = None;
            thread::park();
            continue;
        }
        let minute = Duration::from_secs(TICK_SECS);
        if automation_on && last_tick.is_none_or(|at| at.elapsed() >= minute) {
            tick(app);
            last_tick = Some(Instant::now());
        }
        if watching {
            profiles::tick(app);
        }
        let wait = if watching {
            Duration::from_secs(TRIGGER_POLL_SECS)
        } else {
            minute
        };
        let parked = Instant::now();
        thread::park_timeout(wait);
        if parked.elapsed() + Duration::from_secs(1) < wait {
            // Woken before the time was up: a setting changed, so look at it at once.
            last_tick = None;
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
