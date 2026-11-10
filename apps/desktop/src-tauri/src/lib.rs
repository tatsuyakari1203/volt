//! Volt desktop shell: the Tauri host. A composition root, so it may know every crate; it
//! holds commands and window wiring, never business rules.

use serde::Serialize;

/// Product name shown in the UI.
const APP_NAME: &str = "Volt";

/// Identity of the running app, returned to the UI by the `app_info` command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo::current()
}

/// Starts the Tauri application and blocks until it exits.
///
/// If the Tauri runtime fails to start there is no UI to report to, so the error goes to
/// stderr and the process exits with a non-zero code.
pub fn run() {
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("Volt failed to start: {error}");
        std::process::exit(1);
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
}
