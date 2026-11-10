//! The tray icon and its menu. The tray is what keeps Volt reachable while no window exists.
//!
//! Boost runs here, in Rust, through the same use case as the Home button ([`boost::boost_of`]),
//! so it works with the window closed: the engines record it in the activity history and the
//! tooltip shows the result. A window that is open also receives [`BoostFinished`] and shows the
//! report on Home.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime};
use tauri_specta::Event as _;

use crate::boost::{self, BoostFinished};
use crate::window;
use crate::wiring::AppState;

/// Id of the tray icon.
const TRAY_ID: &str = "main-tray";

/// Whether a tray Boost is running, so a second click while it runs does nothing.
#[derive(Debug, Default)]
struct BoostRunning(AtomicBool);

impl BoostRunning {
    /// Marks a run as started. `false` when one is already running.
    fn try_start(&self) -> bool {
        !self.0.swap(true, Ordering::SeqCst)
    }

    fn finish(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// One entry of the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    /// Open or focus the window.
    Open,
    /// Run Boost, with or without a window.
    Boost,
    /// Quit the application, tray icon included.
    Quit,
}

impl TrayAction {
    /// Every action, in menu order.
    pub(crate) const ALL: [Self; 3] = [Self::Open, Self::Boost, Self::Quit];

    /// The menu item id of this action.
    #[must_use]
    pub(crate) const fn menu_id(self) -> &'static str {
        match self {
            Self::Open => "tray-open",
            Self::Boost => "tray-boost",
            Self::Quit => "tray-quit",
        }
    }

    /// The label shown in the menu.
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Boost => "Boost",
            Self::Quit => "Quit",
        }
    }

    /// Maps a menu item id back to its action. Unknown ids map to nothing.
    #[must_use]
    pub(crate) fn from_menu_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.menu_id() == id)
    }
}

/// Whether a tray icon event is the left click that opens the window (on release, so the press
/// that started the click is not mistaken for a second one).
#[must_use]
pub(crate) fn opens_window(event: &TrayIconEvent) -> bool {
    matches!(
        event,
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
    )
}

/// Builds the tray icon and its menu. Called once from the app's setup.
pub(crate) fn setup(app: &App) -> tauri::Result<()> {
    app.manage(BoostRunning::default());

    let items = TrayAction::ALL
        .map(|action| MenuItem::with_id(app, action.menu_id(), action.label(), true, None::<&str>));
    let [open, boost, quit] = items;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open?, &boost?, &separator, &quit?])?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Volt")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if let Some(action) = TrayAction::from_menu_id(event.id().as_ref()) {
                run(app, action);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if opens_window(&event) {
                run(tray.app_handle(), TrayAction::Open);
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

/// Carries out a tray action.
fn run<R: Runtime>(app: &AppHandle<R>, action: TrayAction) {
    match action {
        TrayAction::Open => window::open_in_background(app, |_, _| {}),
        TrayAction::Boost => start_boost(app),
        TrayAction::Quit => app.exit(0),
    }
}

/// Runs Boost on a worker thread (it reads and changes memory, so never on the main thread).
/// The result goes to the tooltip and, as an event, to a window that is open.
fn start_boost<R: Runtime>(app: &AppHandle<R>) {
    if !app.state::<BoostRunning>().try_start() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = app
            .state::<AppState>()
            .core()
            .and_then(|core| boost::boost_of(&core));
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            // A failed tooltip update changes nothing that matters.
            let _ = tray.set_tooltip(Some(boost::tooltip_line(&outcome)));
        }
        if let Ok(result) = outcome {
            // A failed emit only means nobody is listening.
            let _ = BoostFinished(result).emit(&app);
        }
        app.state::<BoostRunning>().finish();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_round_trip_to_their_action() {
        for action in TrayAction::ALL {
            assert_eq!(TrayAction::from_menu_id(action.menu_id()), Some(action));
        }
    }

    #[test]
    fn the_menu_offers_open_boost_and_quit_in_that_order() {
        let labels = TrayAction::ALL.map(TrayAction::label);
        assert_eq!(labels, ["Open", "Boost", "Quit"]);
    }

    #[test]
    fn unknown_menu_ids_map_to_nothing() {
        assert_eq!(TrayAction::from_menu_id(""), None);
        assert_eq!(TrayAction::from_menu_id("quit"), None);
        assert_eq!(TrayAction::from_menu_id("tray-purge"), None);
    }

    fn click(button: MouseButton, button_state: MouseButtonState) -> TrayIconEvent {
        TrayIconEvent::Click {
            id: TRAY_ID.into(),
            position: tauri::PhysicalPosition::new(0.0, 0.0),
            rect: tauri::Rect::default(),
            button,
            button_state,
        }
    }

    #[test]
    fn only_a_left_click_release_opens_the_window() {
        assert!(opens_window(&click(
            MouseButton::Left,
            MouseButtonState::Up
        )));
        assert!(!opens_window(&click(
            MouseButton::Left,
            MouseButtonState::Down
        )));
        assert!(!opens_window(&click(
            MouseButton::Right,
            MouseButtonState::Up
        )));
        assert!(!opens_window(&click(
            MouseButton::Middle,
            MouseButtonState::Up
        )));
    }

    #[test]
    fn a_second_boost_cannot_start_while_one_runs() {
        let running = BoostRunning::default();
        assert!(running.try_start());
        assert!(!running.try_start(), "the second click does nothing");
        running.finish();
        assert!(running.try_start(), "it can run again afterwards");
    }

    #[test]
    fn menu_ids_are_unique() {
        let mut ids = TrayAction::ALL.map(TrayAction::menu_id).to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TrayAction::ALL.len());
    }
}
