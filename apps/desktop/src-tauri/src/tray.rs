//! The tray icon and its menu. The tray is what keeps Volt reachable while no window exists.
//!
//! Boost has no engine behind it yet: it opens the window and emits [`TrayBoostRequested`] for a
//! later task to consume. Nothing here purges memory.

use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime};
use tauri_specta::Event as _;

use crate::window::{self, Opened};

/// Id of the tray icon.
const TRAY_ID: &str = "main-tray";

/// Event emitted when the user picks Boost in the tray menu, after the window is open on Home.
///
/// Boost has no engine yet; a later task listens to this event and starts the real work. When
/// the window had to be created, the event is sent once its page has loaded, so a listener
/// registered at start-up sees it, but registration is asynchronous: treat it as best effort
/// until a pull command exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct TrayBoostRequested;

/// A Boost request that arrived while the window had to be created, waiting for its page.
#[derive(Debug, Default)]
struct PendingBoost(AtomicBool);

/// One entry of the tray menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    /// Open or focus the window.
    Open,
    /// Open the window on Home and announce a boost request.
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
    app.manage(PendingBoost::default());

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
        TrayAction::Boost => window::open_in_background(app, |app, opened| match opened {
            Opened::Focused => announce_boost(app),
            // The page is not there yet: send the event once it has loaded.
            Opened::Created => app.state::<PendingBoost>().0.store(true, Ordering::SeqCst),
        }),
        TrayAction::Quit => app.exit(0),
    }
}

/// Sends the event to every window. A failed emit only means nobody hears it; Boost has no
/// effect yet, so there is nothing to recover.
fn announce_boost<R: Runtime>(app: &AppHandle<R>) {
    let _ = TrayBoostRequested.emit(app);
}

/// Called when a page finishes loading: delivers a Boost request that was waiting for it.
pub(crate) fn flush_pending_boost<R: Runtime>(app: &AppHandle<R>) {
    if let Some(pending) = app.try_state::<PendingBoost>()
        && pending.0.swap(false, Ordering::SeqCst)
    {
        announce_boost(app);
    }
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
    fn menu_ids_are_unique() {
        let mut ids = TrayAction::ALL.map(TrayAction::menu_id).to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TrayAction::ALL.len());
    }
}
