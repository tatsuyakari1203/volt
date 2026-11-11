//! The tray icon and its menu. The tray is what keeps Volt reachable while no window exists.
//!
//! Boost runs here, in Rust, through the same use case as the Home button ([`boost::boost_of`]),
//! so it works with the window closed: the engines record it in the activity history, the tooltip
//! shows the result and, when no window is open, a desktop notification says what happened. A
//! window that is open also receives [`BoostFinished`] and shows the report on Home.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_specta::Event as _;

use crate::IpcError;
use crate::boost::{self, BoostFinished, BoostResult};
use crate::language::{self, Lang};
use crate::panel;
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

    /// A guard that clears the flag when dropped, also when the worker returns early or panics.
    fn guard(&self) -> RunningGuard<'_> {
        RunningGuard(self)
    }
}

/// Clears [`BoostRunning`] on drop, so a failed or panicked Boost cannot block later ones.
struct RunningGuard<'a>(&'a BoostRunning);

impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// The items of the tray menu, kept so that a language change can rewrite their text at once.
struct TrayItems<R: Runtime> {
    open: MenuItem<R>,
    boost: MenuItem<R>,
    quit: MenuItem<R>,
}

impl<R: Runtime> TrayItems<R> {
    fn of(&self, action: TrayAction) -> &MenuItem<R> {
        match action {
            TrayAction::Open => &self.open,
            TrayAction::Boost => &self.boost,
            TrayAction::Quit => &self.quit,
        }
    }
}

/// Writes the menu item texts in `lang`, at once. Nothing happens before the tray exists.
pub(crate) fn apply_language<R: Runtime>(app: &AppHandle<R>, lang: Lang) {
    let Some(items) = app.try_state::<TrayItems<R>>() else {
        return;
    };
    for action in TrayAction::ALL {
        if let Err(error) = items.of(action).set_text(action.label(lang)) {
            // A menu that keeps its old text is a nuisance, never a reason to fail.
            log::warn!("the tray menu text could not be changed: {error}");
        }
    }
}

/// Sets the tray tooltip. A failure changes nothing that matters, so it is ignored.
pub(crate) fn set_tooltip<R: Runtime>(app: &AppHandle<R>, text: String) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_tooltip(Some(text));
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

    /// The label shown in the menu, in the language the app is set to.
    #[must_use]
    pub(crate) fn label(self, lang: Lang) -> String {
        let code = match self {
            Self::Open => "tray.open",
            Self::Boost => "tray.boost",
            Self::Quit => "tray.quit",
        };
        crate::phrase::write(&volt_core::Phrase::new(code), lang)
    }

    /// Maps a menu item id back to its action. Unknown ids map to nothing.
    #[must_use]
    pub(crate) fn from_menu_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.menu_id() == id)
    }
}

/// Whether a tray icon event is the left click that opens the panel (on release, so the press
/// that started the click is not mistaken for a second one).
#[must_use]
pub(crate) fn opens_panel(event: &TrayIconEvent) -> bool {
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
    panel::setup(app);

    // The menu starts in the language that is set now; [`apply_language`] rewrites it when the
    // user changes the language.
    let lang = language::current(&app.state::<AppState>());
    let item = |action: TrayAction| {
        MenuItem::with_id(
            app,
            action.menu_id(),
            action.label(lang),
            true,
            None::<&str>,
        )
    };
    let items = TrayItems {
        open: item(TrayAction::Open)?,
        boost: item(TrayAction::Boost)?,
        quit: item(TrayAction::Quit)?,
    };
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&items.open, &items.boost, &separator, &items.quit])?;
    app.manage(items);

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
            if opens_panel(&event)
                && let TrayIconEvent::Click { position, rect, .. } = &event
            {
                panel::toggle(tray.app_handle(), panel::Anchor::of(rect, *position));
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
fn start_boost<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        boost_now(&app);
    });
}

/// Runs Boost on the calling thread, which must not be the main thread, and returns how it
/// ended; None when another Boost is running (nothing is started then). The result goes to the
/// tooltip, to a notification when no main window is open and, as an event, to every window.
/// The tray menu and the tray panel both call this, so they follow the same rules.
pub(crate) fn boost_now<R: Runtime>(app: &AppHandle<R>) -> Option<Result<BoostResult, IpcError>> {
    let running = app.state::<BoostRunning>();
    if !running.try_start() {
        return None;
    }
    let _clears_the_flag = running.guard();
    let outcome = app
        .state::<AppState>()
        .core()
        .and_then(|core| boost::boost_of(&core));
    let lang = language::current(&app.state::<AppState>());
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        // A failed tooltip update changes nothing that matters.
        let _ = tray.set_tooltip(Some(boost::tooltip_line(&outcome, lang)));
    }
    if app.get_webview_window(window::MAIN_WINDOW).is_none() {
        // With a window open the report is on Home; with none, this is the only feedback.
        // A notification that cannot be shown (disabled in Windows) leaves the tooltip.
        let _ = app
            .notification()
            .builder()
            .title("Volt Boost")
            .body(boost::notification_body(&outcome, lang))
            .show();
    }
    if let Ok(result) = &outcome {
        // A failed emit only means nobody is listening.
        let _ = BoostFinished(result.clone()).emit(app);
    }
    Some(outcome)
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
        let labels = TrayAction::ALL.map(|action| action.label(Lang::English));
        assert_eq!(labels, ["Open", "Boost", "Quit"]);
    }

    #[test]
    fn the_menu_has_a_vietnamese_form_with_the_same_items() {
        let labels = TrayAction::ALL.map(|action| action.label(Lang::Vietnamese));
        assert_eq!(labels, ["Mở", "Boost", "Thoát"]);
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
    fn only_a_left_click_release_opens_the_panel() {
        assert!(opens_panel(&click(MouseButton::Left, MouseButtonState::Up)));
        assert!(!opens_panel(&click(
            MouseButton::Left,
            MouseButtonState::Down
        )));
        assert!(!opens_panel(&click(
            MouseButton::Right,
            MouseButtonState::Up
        )));
        assert!(!opens_panel(&click(
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
    fn the_flag_is_cleared_when_the_worker_fails_or_panics() {
        let running = BoostRunning::default();

        assert!(running.try_start());
        {
            let _guard = running.guard();
            // The worker returns early with an error: the guard drops here.
        }
        assert!(
            running.try_start(),
            "a failed Boost does not block the next"
        );
        running.finish();

        assert!(running.try_start());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = running.guard();
            panic!("the worker panics");
        }));
        assert!(panicked.is_err());
        assert!(
            running.try_start(),
            "a panicked Boost does not block the next"
        );
    }

    #[test]
    fn menu_ids_are_unique() {
        let mut ids = TrayAction::ALL.map(TrayAction::menu_id).to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TrayAction::ALL.len());
    }
}
