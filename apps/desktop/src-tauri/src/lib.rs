//! Volt desktop shell: the Tauri host. A composition root, so it may know every crate; it
//! holds commands and window wiring, never business rules.

pub mod api;
pub mod apps;
mod autostart;
pub mod boost;
pub mod catalogue;
pub mod cleaner;
mod commands;
pub mod elevated_start;
pub mod elevation;
pub mod exclusions;
pub mod health;
mod ipc;
pub mod language;
pub mod large_files;
mod logging;
pub mod memory;
pub mod metrics;
pub mod panel;
pub mod phrase;
pub mod processes;
mod sampler;
pub mod transfer;
mod tray;
mod trend;
pub mod tuning;
pub mod unsupported;
mod window;
pub mod wiring;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri::webview::PageLoadEvent;
use tauri::{RunEvent, WindowEvent};
use tauri_specta::Event as _;

pub use boost::BoostFinished;
pub use ipc::{AppReady, IpcError, bindings_builder, export_bindings};
pub use metrics::MetricsUpdated;
pub use trend::{MemoryPointView, MemoryTrendUpdated};

/// Product name shown in the UI.
const APP_NAME: &str = "Volt";

/// Identity of the running app, returned to the UI by the `app_info` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct AppInfo {
    /// Product name.
    pub name: String,
    /// Crate version (workspace version).
    pub version: String,
}

impl AppInfo {
    /// Builds the info for this build of the app.
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: APP_NAME.to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

/// Tauri command: returns the app name and version.
///
/// The sample end-to-end command. It cannot fail today, but it returns the shell's typed error
/// like every command will, so the generated binding carries the error type.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::unnecessary_wraps,
    reason = "every command returns Result<_, IpcError> so the UI handles one error shape"
)]
fn app_info() -> Result<AppInfo, IpcError> {
    Ok(AppInfo::current())
}

/// Starts the Tauri application and blocks until it exits.
///
/// If the Tauri runtime fails to start there is no UI to report to, so the error goes to
/// stderr and the process exits with a non-zero code.
pub fn run() {
    // Before any window, WebView or plugin exists: an elevated UI is never started (see
    // `elevated_start`).
    if !elevated_start::allow_start() {
        std::process::exit(1);
    }
    let bindings = bindings_builder();
    let built = tauri::Builder::default()
        // Must come first: a second launch is handled before anything else starts.
        .plugin(window::single_instance_plugin())
        .plugin(autostart::plugin())
        // Only the Rust side opens the save dialog (export_activity); no capability grants it,
        // or any file system access, to the UI.
        .plugin(tauri_plugin_dialog::init())
        // Only the tray's Boost shows a notification, from Rust; no capability grants it to the UI.
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(bindings.invoke_handler())
        .setup(move |app| {
            logging::install(app.handle());
            // The core outlives the window: the window and its WebView come and go, the engine
            // and the database stay. Built before any window exists, so the first page load can
            // already ask for tweaks.
            app.manage(wiring::init(app.handle()));
            // The sampler thread is not started here: it runs only while the main window exists.
            let trend = std::sync::Arc::new(trend::MemoryTrend::default());
            app.manage(std::sync::Arc::clone(&trend));
            app.manage(sampler::real(app.handle(), trend));
            bindings.mount_events(app);
            tray::setup(app)?;
            if window::LaunchMode::from_args(std::env::args().skip(1)) == window::LaunchMode::Foreground {
                window::open_in_background(app.handle(), |_, _| {});
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Sampling stops with the last window that shows it; the tick probe in the sampler
            // is the safety net for the order in which Tauri unregisters the window and sends
            // this event.
            let label = window.label();
            let shows_metrics = label == window::MAIN_WINDOW || label == panel::PANEL_WINDOW;
            if matches!(event, WindowEvent::Destroyed) && shows_metrics {
                sampler::sync_with_window(window.app_handle());
            }
            if label == panel::PANEL_WINDOW {
                panel::on_window_event(window.app_handle(), event);
            }
        })
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                // A failed emit only means the UI misses a notification; it can still ask via
                // the `app_info` command, so there is nothing to recover.
                let _ = AppReady(AppInfo::current()).emit(webview);
            }
        })
        .build(tauri::generate_context!());
    match built {
        Ok(app) => app.run(|handle, event| {
            // The administrator session (if one is open) ends with the app: bye, then close.
            if matches!(event, RunEvent::Exit) {
                elevation::end_session(handle);
            }
            window::keep_running(handle, event);
        }),
        Err(error) => {
            eprintln!("Volt failed to start: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_info_reports_product_name_and_crate_version() {
        let info = AppInfo::current();
        assert_eq!(info.name, "Volt");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn app_info_command_returns_the_current_info() {
        assert_eq!(app_info(), Ok(AppInfo::current()));
    }
}
