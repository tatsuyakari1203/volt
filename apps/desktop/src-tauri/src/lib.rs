//! Volt desktop shell: the Tauri host. A composition root, so it may know every crate; it
//! holds commands and window wiring, never business rules.

pub mod api;
mod autostart;
pub mod catalogue;
mod commands;
mod ipc;
mod logging;
pub mod metrics;
mod sampler;
mod tray;
pub mod unsupported;
mod window;
pub mod wiring;

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::Manager as _;
use tauri::WindowEvent;
use tauri::webview::PageLoadEvent;
use tauri_specta::Event as _;

pub use ipc::{AppReady, IpcError, bindings_builder, export_bindings};
pub use metrics::MetricsUpdated;
pub use tray::TrayBoostRequested;

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
    let bindings = bindings_builder();
    let built = tauri::Builder::default()
        // Must come first: a second launch is handled before anything else starts.
        .plugin(window::single_instance_plugin())
        .plugin(autostart::plugin())
        // Only the Rust side opens the save dialog (export_activity); no capability grants it,
        // or any file system access, to the UI.
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(bindings.invoke_handler())
        .setup(move |app| {
            logging::install(app.handle());
            // The core outlives the window: the window and its WebView come and go, the engine
            // and the database stay. Built before any window exists, so the first page load can
            // already ask for tweaks.
            app.manage(wiring::init(app.handle()));
            // The sampler thread is not started here: it runs only while the main window exists.
            app.manage(sampler::real(app.handle()));
            bindings.mount_events(app);
            tray::setup(app)?;
            if window::LaunchMode::from_args(std::env::args().skip(1)) == window::LaunchMode::Foreground {
                window::open_in_background(app.handle(), |_, _| {});
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Sampling stops with the window; the tick probe in the sampler is the safety net
            // for the order in which Tauri unregisters the window and sends this event.
            if matches!(event, WindowEvent::Destroyed) && window.label() == window::MAIN_WINDOW {
                sampler::sync_with_window(window.app_handle());
            }
        })
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                // A failed emit only means the UI misses a notification; it can still ask via
                // the `app_info` command, so there is nothing to recover.
                let _ = AppReady(AppInfo::current()).emit(webview);
                tray::flush_pending_boost(webview.app_handle());
            }
        })
        .build(tauri::generate_context!());
    match built {
        Ok(app) => app.run(window::keep_running),
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
