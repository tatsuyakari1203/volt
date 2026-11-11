//! The tray panel: a small borderless window that a left click on the tray icon opens, for a
//! glance at memory and CPU and a one-click Boost without the full window.
//!
//! The window is declared in `tauri.conf.json` (`create: false`, like the main window), built
//! when the icon is clicked and destroyed when it loses focus, so no `WebView` stays alive for it:
//! the tray-only footprint is the same before and after. It is also destroyed by Escape and by
//! the two buttons of the panel (see the commands below).
//!
//! Where it goes ([`place`]) is plain arithmetic on rectangles in physical pixels: the edge of
//! the monitor nearest to the icon is the taskbar edge, the panel sits against the work area on
//! that edge, centred on the icon and kept inside the work area. A click on the icon while the
//! panel is open first takes its focus away (which closes it) and then arrives as a click; that
//! second event must not open it again, see [`decide_toggle`].

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{
    AppHandle, Manager, PhysicalPosition, Rect, Runtime, WebviewWindow, WebviewWindowBuilder,
    WindowEvent,
};

use crate::IpcError;
use crate::boost;
use crate::language;
use crate::tray;
use crate::window;
use crate::wiring::AppState;

/// Label of the panel window, as declared in `tauri.conf.json` and its capability.
pub(crate) const PANEL_WINDOW: &str = "tray-panel";

/// Space between the panel and the taskbar, and between the panel and the screen edge, in
/// physical pixels.
const GAP: i32 = 8;

/// A click on the tray icon this soon after the panel closed itself on focus loss is the click
/// that caused the loss, not a request to open the panel again.
const REOPEN_GUARD: Duration = Duration::from_millis(400);

/// Right after the panel is shown the taskbar or the tray flyout it was opened from may still
/// take the focus back; a focus loss inside this time is not the user clicking elsewhere.
const BLUR_GRACE: Duration = Duration::from_millis(600);

/// A rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Px {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Px {
    const fn right(self) -> i32 {
        self.x + self.w
    }

    const fn bottom(self) -> i32 {
        self.y + self.h
    }
}

/// Where the click that opens the panel happened.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Anchor {
    /// The tray icon. Zero-sized when Windows did not report it.
    pub icon: Px,
    /// The cursor, in physical pixels of the virtual screen.
    pub cursor: (f64, f64),
}

impl Anchor {
    /// Reads the anchor from a tray click event's icon rectangle and cursor position.
    pub(crate) fn of(rect: &Rect, cursor: PhysicalPosition<f64>) -> Self {
        let position = rect.position.to_physical::<i32>(1.0);
        let size = rect.size.to_physical::<u32>(1.0);
        Self {
            icon: Px {
                x: position.x,
                y: position.y,
                w: i32::try_from(size.width).unwrap_or(0),
                h: i32::try_from(size.height).unwrap_or(0),
            },
            cursor: (cursor.x, cursor.y),
        }
    }

    /// The point the panel points at: the middle of the icon, or the cursor when the icon has
    /// no size.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "a screen coordinate is far below the i32 range"
    )]
    fn centre(self) -> (i32, i32) {
        if self.icon.w > 0 && self.icon.h > 0 {
            (self.icon.x + self.icon.w / 2, self.icon.y + self.icon.h / 2)
        } else {
            (self.cursor.0.round() as i32, self.cursor.1.round() as i32)
        }
    }
}

/// `value` limited to `low..=high`; a range that is empty (a panel larger than the area) gives
/// `low`, so the panel starts at the corner instead of panicking.
fn clamp_into(value: i32, low: i32, high: i32) -> i32 {
    value.min(high.max(low)).max(low)
}

/// The top-left corner of a panel of `width` x `height` pixels for a click at `anchor`.
///
/// `monitor` is the whole monitor the click was on and `work` its work area (the monitor minus
/// the taskbar). The edge of `monitor` nearest to the icon is where the taskbar is; the panel
/// sits against `work` on that edge with a [`GAP`], centred on the icon along the edge, and is
/// kept inside `work` on every side.
#[must_use]
pub(crate) fn place(anchor: Anchor, monitor: Px, work: Px, width: i32, height: i32) -> (i32, i32) {
    let (cx, cy) = anchor.centre();
    let to_left = cx - monitor.x;
    let to_right = monitor.right() - cx;
    let to_top = cy - monitor.y;
    let to_bottom = monitor.bottom() - cy;
    let nearest = to_left.min(to_right).min(to_top).min(to_bottom);

    let (x, y) = if nearest == to_bottom {
        (cx - width / 2, work.bottom() - height - GAP)
    } else if nearest == to_top {
        (cx - width / 2, work.y + GAP)
    } else if nearest == to_left {
        (work.x + GAP, cy - height / 2)
    } else {
        (work.right() - width - GAP, cy - height / 2)
    };
    (
        clamp_into(x, work.x + GAP, work.right() - width - GAP),
        clamp_into(y, work.y + GAP, work.bottom() - height - GAP),
    )
}

/// What a click on the tray icon does to the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToggleAction {
    /// The panel is open: close it.
    Close,
    /// Build and show the panel.
    Open,
    /// The click that closed the panel by taking its focus: do nothing.
    Ignore,
}

/// Decides what a click does, from whether the panel exists and how long ago it closed itself on
/// focus loss (`None` when it did not).
#[must_use]
pub(crate) fn decide_toggle(exists: bool, since_focus_loss: Option<Duration>) -> ToggleAction {
    if exists {
        ToggleAction::Close
    } else if since_focus_loss.is_some_and(|elapsed| elapsed < REOPEN_GUARD) {
        ToggleAction::Ignore
    } else {
        ToggleAction::Open
    }
}

/// Whether a focus loss closes the panel, from how long ago it was shown.
#[must_use]
pub(crate) fn blur_closes(since_shown: Option<Duration>) -> bool {
    since_shown.is_none_or(|elapsed| elapsed >= BLUR_GRACE)
}

/// What the panel remembers between clicks. Managed state; created by [`setup`].
#[derive(Debug, Default)]
pub(crate) struct PanelState {
    /// The window has been focused at least once since it was built, so a focus-lost event that
    /// arrives while it is still being shown does not close it before anyone saw it.
    focused: AtomicBool,
    /// When the panel was last shown.
    shown_at: Mutex<Option<Instant>>,
    /// When the panel last closed itself because it lost focus.
    closed_on_blur: Mutex<Option<Instant>>,
    /// Serialises open requests: two quick clicks must not both build the window.
    opening: Mutex<()>,
}

fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The guarded values are replaced as a whole, so a poisoned lock holds nothing half-written.
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Registers the panel's state. Called once from the tray's setup.
pub(crate) fn setup<R: Runtime>(app: &impl Manager<R>) {
    app.manage(PanelState::default());
}

/// Why the panel could not be opened.
#[derive(Debug)]
enum PanelError {
    /// `tauri.conf.json` has no window with the panel's label.
    MissingConfig,
    /// Tauri failed to build or show the window.
    Tauri(tauri::Error),
}

impl fmt::Display for PanelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfig => {
                write!(f, "tauri.conf.json declares no \"{PANEL_WINDOW}\" window")
            }
            Self::Tauri(error) => write!(f, "{error}"),
        }
    }
}

impl From<tauri::Error> for PanelError {
    fn from(error: tauri::Error) -> Self {
        Self::Tauri(error)
    }
}

/// A left click on the tray icon: closes the panel when it is open, otherwise opens it near the
/// icon. Building a window waits for the event loop, so it happens on a worker thread (never in
/// the tray callback, which runs on the main thread).
pub(crate) fn toggle<R: Runtime>(app: &AppHandle<R>, anchor: Anchor) {
    let Some(state) = app.try_state::<PanelState>() else {
        return;
    };
    let exists = app.get_webview_window(PANEL_WINDOW).is_some();
    let since = locked(&state.closed_on_blur).map(|at| at.elapsed());
    match decide_toggle(exists, since) {
        ToggleAction::Ignore => {}
        ToggleAction::Close => close(app),
        ToggleAction::Open => {
            let app = app.clone();
            std::thread::spawn(move || {
                if let Err(error) = open(&app, anchor) {
                    log::error!("Volt could not open the tray panel: {error}");
                }
            });
        }
    }
}

/// Builds the panel next to the tray icon, shows it and focuses it.
fn open<R: Runtime>(app: &AppHandle<R>, anchor: Anchor) -> Result<(), PanelError> {
    let state = app.state::<PanelState>();
    let _opening = locked(&state.opening);
    if app.get_webview_window(PANEL_WINDOW).is_some() {
        return Ok(());
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == PANEL_WINDOW)
        .ok_or(PanelError::MissingConfig)?;
    state.focused.store(false, Ordering::SeqCst);
    // Hidden in the configuration: it is moved before it is shown, so it never flashes at the
    // default position.
    let window = WebviewWindowBuilder::from_config(app, config)?.build()?;
    move_next_to_icon(app, &window, anchor, (config.width, config.height));
    *locked(&state.shown_at) = Some(Instant::now());
    window.show()?;
    window.set_focus()?;
    // A focus loss inside the grace time is ignored, so when the focus never comes (or is taken
    // for good) nothing would close the panel: look once the grace time is over.
    let watcher = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(BLUR_GRACE + Duration::from_millis(150));
        if let Some(window) = watcher.get_webview_window(PANEL_WINDOW)
            && !window.is_focused().unwrap_or(true)
        {
            close_on_blur(&watcher);
        }
    });
    // The panel shows live metrics, so sampling runs while it exists.
    crate::sampler::sync_with_window(app);
    Ok(())
}

/// Puts the window where [`place`] says. A monitor that cannot be found leaves it where the
/// system put it: a misplaced panel is better than none.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a logical size times a scale factor is far below the i32 range"
)]
fn move_next_to_icon<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    anchor: Anchor,
    logical: (f64, f64),
) {
    let (cx, cy) = anchor.centre();
    let monitor = app
        .monitor_from_point(f64::from(cx), f64::from(cy))
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.scale_factor();
    let size = monitor.size();
    let at = monitor.position();
    let area = monitor.work_area();
    let to_px = |x: i32, y: i32, w: u32, h: u32| Px {
        x,
        y,
        w: i32::try_from(w).unwrap_or(i32::MAX),
        h: i32::try_from(h).unwrap_or(i32::MAX),
    };
    let (x, y) = place(
        anchor,
        to_px(at.x, at.y, size.width, size.height),
        to_px(
            area.position.x,
            area.position.y,
            area.size.width,
            area.size.height,
        ),
        (logical.0 * scale).round() as i32,
        (logical.1 * scale).round() as i32,
    );
    if let Err(error) = window.set_position(PhysicalPosition::new(x, y)) {
        log::warn!("the tray panel could not be moved next to the icon: {error}");
    }
}

/// Destroys the panel and with it its `WebView`.
pub(crate) fn close<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(PANEL_WINDOW)
        && let Err(error) = window.close()
    {
        log::warn!("the tray panel could not be closed: {error}");
    }
}

/// Window events of the panel: it closes when it loses focus.
pub(crate) fn on_window_event<R: Runtime>(app: &AppHandle<R>, event: &WindowEvent) {
    let Some(state) = app.try_state::<PanelState>() else {
        return;
    };
    match event {
        WindowEvent::Focused(true) => state.focused.store(true, Ordering::SeqCst),
        WindowEvent::Focused(false)
            if state.focused.load(Ordering::SeqCst)
                && blur_closes(locked(&state.shown_at).map(|at| at.elapsed())) =>
        {
            close_on_blur(app);
        }
        _ => {}
    }
}

/// Closes the panel because it lost focus, and remembers when, see [`decide_toggle`].
fn close_on_blur<R: Runtime>(app: &AppHandle<R>) {
    if let Some(state) = app.try_state::<PanelState>() {
        *locked(&state.closed_on_blur) = Some(Instant::now());
    }
    close(app);
}

/// The result line under the panel's Boost button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PanelBoostLine {
    /// One sentence in the language of the app, the same words as the tray tooltip.
    pub line: String,
    /// The Boost failed.
    pub failed: bool,
}

/// Tauri command: runs Boost the way the tray menu item does (the same use case, notification
/// and tooltip) and returns the line to show. `null` when a Boost is already running. It never
/// asks for administrator rights.
#[tauri::command]
#[specta::specta]
pub(crate) async fn panel_boost(app: AppHandle) -> Result<Option<PanelBoostLine>, IpcError> {
    let worker = app.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || tray::boost_now(&worker))
        .await
        .map_err(|error| IpcError::Other {
            message: error.to_string(),
        })?;
    Ok(outcome.map(|outcome| {
        let lang = language::current(&app.state::<AppState>());
        PanelBoostLine {
            line: boost::tooltip_line(&outcome, lang),
            failed: outcome.is_err(),
        }
    }))
}

/// Tauri command: closes the panel (Escape).
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "every command returns Result<_, IpcError> so the UI handles one error shape, and Tauri hands a command its AppHandle by value"
)]
pub(crate) fn close_tray_panel(app: AppHandle) -> Result<(), IpcError> {
    close(&app);
    Ok(())
}

/// Tauri command: opens (or focuses) the main window and closes the panel.
#[tauri::command]
#[specta::specta]
#[allow(
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value,
    reason = "every command returns Result<_, IpcError> so the UI handles one error shape, and Tauri hands a command its AppHandle by value"
)]
pub(crate) fn open_main_from_panel(app: AppHandle) -> Result<(), IpcError> {
    window::open_in_background(&app, |_, _| {});
    close(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn px(x: i32, y: i32, w: i32, h: i32) -> Px {
        Px { x, y, w, h }
    }

    fn anchor(icon: Px) -> Anchor {
        Anchor {
            icon,
            cursor: (0.0, 0.0),
        }
    }

    /// 1920 x 1080 with a 48 px taskbar on the chosen edge.
    const MONITOR: Px = px(0, 0, 1920, 1080);
    const W: i32 = 300;
    const H: i32 = 340;

    #[test]
    fn a_bottom_taskbar_puts_the_panel_above_it_centred_on_the_icon() {
        let work = px(0, 0, 1920, 1032);
        let icon = px(1700, 1040, 24, 24);
        let (x, y) = place(anchor(icon), MONITOR, work, W, H);
        assert_eq!(x, 1712 - W / 2);
        assert_eq!(y, 1032 - H - GAP);
    }

    #[test]
    fn a_top_taskbar_puts_the_panel_below_it() {
        let work = px(0, 48, 1920, 1032);
        let icon = px(1700, 12, 24, 24);
        let (_, y) = place(anchor(icon), MONITOR, work, W, H);
        assert_eq!(y, 48 + GAP);
    }

    #[test]
    fn a_left_taskbar_puts_the_panel_beside_it() {
        let work = px(64, 0, 1856, 1080);
        let icon = px(20, 500, 24, 24);
        let (x, y) = place(anchor(icon), MONITOR, work, W, H);
        assert_eq!(x, 64 + GAP);
        assert_eq!(y, 512 - H / 2);
    }

    #[test]
    fn a_right_taskbar_puts_the_panel_beside_it() {
        let work = px(0, 0, 1856, 1080);
        let icon = px(1880, 900, 24, 24);
        let (x, _) = place(anchor(icon), MONITOR, work, W, H);
        assert_eq!(x, 1856 - W - GAP);
    }

    #[test]
    fn the_panel_stays_inside_the_work_area_near_a_corner() {
        let work = px(0, 0, 1920, 1032);
        let icon = px(1900, 1040, 24, 24);
        let (x, y) = place(anchor(icon), MONITOR, work, W, H);
        assert_eq!(x, 1920 - W - GAP);
        assert!(y >= GAP);
        let far = px(0, 1040, 24, 24);
        assert_eq!(place(anchor(far), MONITOR, work, W, H).0, GAP);
    }

    #[test]
    fn a_second_monitor_uses_its_own_work_area() {
        let monitor = px(1920, 0, 2560, 1440);
        let work = px(1920, 0, 2560, 1392);
        let icon = px(4000, 1400, 24, 24);
        let (x, y) = place(anchor(icon), monitor, work, W, H);
        assert_eq!(x, 4012 - W / 2);
        assert_eq!(y, 1392 - H - GAP);
    }

    #[test]
    fn a_monitor_left_of_the_primary_has_negative_coordinates() {
        let monitor = px(-1920, 0, 1920, 1080);
        let work = px(-1920, 0, 1920, 1032);
        let icon = px(-300, 1040, 24, 24);
        let (x, y) = place(anchor(icon), monitor, work, W, H);
        assert_eq!(x, -288 - W / 2);
        assert_eq!(y, 1032 - H - GAP);
    }

    #[test]
    fn an_icon_without_a_size_falls_back_to_the_cursor() {
        let work = px(0, 0, 1920, 1032);
        let at = Anchor {
            icon: px(0, 0, 0, 0),
            cursor: (1000.0, 1050.0),
        };
        let (x, y) = place(at, MONITOR, work, W, H);
        assert_eq!(x, 1000 - W / 2);
        assert_eq!(y, 1032 - H - GAP);
    }

    #[test]
    fn a_panel_larger_than_the_area_starts_at_its_corner() {
        let work = px(0, 0, 200, 200);
        let icon = px(100, 190, 24, 24);
        let (x, y) = place(anchor(icon), px(0, 0, 200, 240), work, W, H);
        assert_eq!((x, y), (GAP, GAP));
    }

    #[test]
    fn a_click_opens_the_panel_and_a_second_click_closes_it() {
        assert_eq!(decide_toggle(false, None), ToggleAction::Open);
        assert_eq!(decide_toggle(true, None), ToggleAction::Close);
    }

    #[test]
    fn a_focus_loss_right_after_showing_does_not_close_the_panel() {
        assert!(!blur_closes(Some(Duration::from_millis(100))));
        assert!(blur_closes(Some(Duration::from_secs(2))));
        assert!(blur_closes(None));
    }

    #[test]
    fn the_click_that_took_the_focus_does_not_open_it_again() {
        let soon = Some(Duration::from_millis(50));
        let late = Some(Duration::from_secs(5));
        assert_eq!(decide_toggle(false, soon), ToggleAction::Ignore);
        assert_eq!(decide_toggle(false, late), ToggleAction::Open);
    }
}
