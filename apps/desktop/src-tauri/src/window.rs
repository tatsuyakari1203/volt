//! Window lifecycle: the main window and its `WebView` exist only while someone is looking at them.
//!
//! The window is declared in `tauri.conf.json` with `create: false`, so the 460 x 700 default
//! and the 420 x 600 minimum stay in one place, and it is built from that declaration on demand.
//! Closing it destroys the window and its `WebView`; the process and the tray icon stay alive
//! because [`keep_running`] refuses the exit Tauri requests when the last window goes away.
//!
//! The decisions (create or focus, background launch, prevent exit) are plain functions so they
//! are tested without a Tauri runtime.

use std::fmt;
use std::sync::Mutex;

use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, RunEvent, Runtime, WebviewWindowBuilder};

/// Label of the main window, as declared in `tauri.conf.json` and the default capability.
pub(crate) const MAIN_WINDOW: &str = "main";

/// Argument the Start with Windows entry passes, so a login launch goes straight to the tray.
pub(crate) const BACKGROUND_ARG: &str = "--background";

/// Serialises open requests: two quick clicks must not both decide to create the window.
static OPEN_LOCK: Mutex<()> = Mutex::new(());

/// How a launch of the app should present itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchMode {
    /// The user started the app: show the window.
    Foreground,
    /// Started by Start with Windows: stay in the tray without a window.
    Background,
}

impl LaunchMode {
    /// Reads the mode from a command line, program name excluded.
    pub(crate) fn from_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if args.into_iter().any(|arg| arg.as_ref() == BACKGROUND_ARG) {
            Self::Background
        } else {
            Self::Foreground
        }
    }
}

/// What is known about an existing main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowState {
    /// The window is shown.
    pub visible: bool,
    /// The window is minimised.
    pub minimized: bool,
}

/// What an open request has to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAction {
    /// There is no window: build it from the configuration.
    Create,
    /// The window exists: bring it to the front.
    Focus {
        /// It is hidden and must be shown first.
        show: bool,
        /// It is minimised and must be restored first.
        unminimize: bool,
    },
}

/// Decides between creating the window and focusing the existing one.
#[must_use]
pub(crate) fn decide_open(existing: Option<WindowState>) -> OpenAction {
    match existing {
        None => OpenAction::Create,
        Some(state) => OpenAction::Focus {
            show: !state.visible,
            unminimize: state.minimized,
        },
    }
}

/// Whether Tauri's exit request must be refused. An exit with a code is explicit (Quit calls
/// `exit(0)`); one without a code is the "last window closed" request, which the tray outlives.
#[must_use]
pub(crate) fn should_prevent_exit(code: Option<i32>) -> bool {
    code.is_none()
}

/// What [`open_main_window`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opened {
    /// A new window (and `WebView`) was built.
    Created,
    /// The existing window was brought to the front.
    Focused,
}

/// Why the main window could not be opened.
#[derive(Debug)]
pub(crate) enum WindowError {
    /// `tauri.conf.json` has no window with the main label.
    MissingConfig,
    /// Tauri failed to build or focus the window.
    Tauri(tauri::Error),
}

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfig => {
                write!(f, "tauri.conf.json declares no \"{MAIN_WINDOW}\" window")
            }
            Self::Tauri(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for WindowError {}

impl From<tauri::Error> for WindowError {
    fn from(error: tauri::Error) -> Self {
        Self::Tauri(error)
    }
}

/// Creates the main window or focuses the existing one. Blocks while the window is built, so
/// call it from a worker thread: [`open_in_background`] does.
pub(crate) fn open_main_window<R: Runtime>(app: &AppHandle<R>) -> Result<Opened, WindowError> {
    // A poisoned lock only means another open request panicked; the guard protects no data.
    let _guard = OPEN_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let existing = app.get_webview_window(MAIN_WINDOW);
    let state = match &existing {
        Some(window) => Some(WindowState {
            visible: window.is_visible()?,
            minimized: window.is_minimized()?,
        }),
        None => None,
    };
    match decide_open(state) {
        OpenAction::Create => {
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|config| config.label == MAIN_WINDOW)
                .ok_or(WindowError::MissingConfig)?;
            WebviewWindowBuilder::from_config(app, config)?.build()?;
            Ok(Opened::Created)
        }
        OpenAction::Focus { show, unminimize } => {
            // `decide_open` only answers Focus for a window that exists.
            let window = existing.ok_or(tauri::Error::WindowNotFound)?;
            if unminimize {
                window.unminimize()?;
            }
            if show {
                window.show()?;
            }
            window.set_focus()?;
            Ok(Opened::Focused)
        }
    }
}

/// Opens the main window on a worker thread and hands the outcome to `then`.
///
/// Building a window waits for the event loop, so it must never run on the main thread from
/// inside a tray, menu or plugin callback (that deadlocks on Windows). A failure goes to the
/// diagnostic log (there is nowhere better to report it yet) and the next open request tries
/// again.
pub(crate) fn open_in_background<R, F>(app: &AppHandle<R>, then: F)
where
    R: Runtime,
    F: FnOnce(&AppHandle<R>, Opened) + Send + 'static,
{
    open_in_background_checked(app, move |app, result| {
        if let Ok(opened) = result {
            then(app, opened);
        }
    });
}

/// Like [`open_in_background`], but `then` also runs when the window could not be opened, so a
/// caller that prepared something before asking can clean it up. The failure is logged here.
pub(crate) fn open_in_background_checked<R, F>(app: &AppHandle<R>, then: F)
where
    R: Runtime,
    F: FnOnce(&AppHandle<R>, Result<Opened, ()>) + Send + 'static,
{
    let app = app.clone();
    std::thread::spawn(move || match open_main_window(&app) {
        Ok(opened) => then(&app, Ok(opened)),
        Err(error) => {
            log::error!("Volt could not open its window: {error}");
            then(&app, Err(()));
        }
    });
}

/// Run-event handler that keeps the process (and the tray icon) alive after the last window
/// closes. Passed to `App::run`.
pub(crate) fn keep_running<R: Runtime>(_app: &AppHandle<R>, event: RunEvent) {
    if let RunEvent::ExitRequested { code, api, .. } = event
        && should_prevent_exit(code)
    {
        api.prevent_exit();
    }
}

/// Single-instance plugin: a second launch focuses (or recreates) the first instance's window
/// and exits. A second launch that is itself a background start (a login entry racing a manual
/// start) changes nothing. Must be registered before every other plugin.
#[must_use]
pub(crate) fn single_instance_plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_single_instance::init(|app, args, _cwd| {
        if LaunchMode::from_args(args.iter().skip(1)) == LaunchMode::Foreground {
            open_in_background(app, |_, _| {});
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_window_means_create() {
        assert_eq!(decide_open(None), OpenAction::Create);
    }

    #[test]
    fn a_visible_window_is_only_focused() {
        let state = WindowState {
            visible: true,
            minimized: false,
        };
        assert_eq!(
            decide_open(Some(state)),
            OpenAction::Focus {
                show: false,
                unminimize: false
            }
        );
    }

    #[test]
    fn a_hidden_or_minimised_window_is_restored_before_focus() {
        let hidden = WindowState {
            visible: false,
            minimized: false,
        };
        assert_eq!(
            decide_open(Some(hidden)),
            OpenAction::Focus {
                show: true,
                unminimize: false
            }
        );
        let minimized = WindowState {
            visible: true,
            minimized: true,
        };
        assert_eq!(
            decide_open(Some(minimized)),
            OpenAction::Focus {
                show: false,
                unminimize: true
            }
        );
    }

    #[test]
    fn only_the_background_flag_selects_a_background_launch() {
        assert_eq!(
            LaunchMode::from_args::<[&str; 0], _>([]),
            LaunchMode::Foreground
        );
        assert_eq!(LaunchMode::from_args(["--other"]), LaunchMode::Foreground);
        assert_eq!(
            LaunchMode::from_args(["--other", BACKGROUND_ARG]),
            LaunchMode::Background
        );
        assert_eq!(
            LaunchMode::from_args(vec![String::from(BACKGROUND_ARG)]),
            LaunchMode::Background
        );
    }

    #[test]
    fn only_the_implicit_last_window_exit_is_prevented() {
        assert!(should_prevent_exit(None));
        assert!(!should_prevent_exit(Some(0)));
        assert!(!should_prevent_exit(Some(1)));
    }
}
