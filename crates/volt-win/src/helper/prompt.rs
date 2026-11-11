//! The helper's confirmation, drawn on a private desktop.
//!
//! A change that persists is made only after a person confirmed it on a surface that no program of
//! the user's can click (see the decision on the helper). This module is that surface:
//!
//! 1. [`PrivateDesktop`] makes a desktop with an ACL that grants access to Administrators only. The
//!    UI's token holds Administrators as a deny-only group, so a program running as the user at
//!    medium integrity cannot open the desktop, attach a thread to it, hook it, enumerate its
//!    windows or post a message to them. (Windows does not deliver messages across desktops at
//!    all.)
//! 2. [`ask`] puts one window on it (a heading, the question, **Cancel as the default button**, and
//!    an accept button), makes it the input desktop for the duration of the question, and always
//!    switches back. Both buttons are disabled for [`BUTTON_DELAY`] so a key press that was meant
//!    for something else cannot answer it. An unanswered question ends at the timeout and counts as
//!    not accepted.
//! 3. It fails closed: when the desktop cannot be created, cannot be switched to, or the current
//!    input desktop cannot be read (the session is locked, say), the answer is
//!    [`PromptAnswer::Unavailable`] and nothing is shown.
//! 4. The person is never left on the empty private desktop: the switch back is retried
//!    ([`SWITCH_BACK_TRIES`] times) and then tried once more to the session's `Default` desktop; when
//!    even that fails the answer is [`PromptAnswer::SwitchBackFailed`] (whatever was clicked), and the
//!    caller must end the process, which destroys the desktop and returns the input to `Default`.
//!    A window thread that is not ready in time, or does not end, is handed to a reaper that closes
//!    the desktop handle once the thread is gone (a desktop with a thread on it cannot be closed),
//!    so the handle is never left open for the life of the process.
//!
//! The only way to get [`PromptAnswer::Accepted`] is a click on the accept button, or Enter or
//! Space with that button focused, after [`BUTTON_DELAY`]. There is no argument, environment
//! variable or setting that answers for the person.
//!
//! Everything about the text lives in the caller ([`PromptText`]); nothing here reads anything the
//! client sent.

use std::cell::{Cell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, HANDLE, HINSTANCE, HLOCAL, HWND, LPARAM, LRESULT, LocalFree, RECT,
    WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateFontIndirectW, DT_CALCRECT, DT_LEFT, DT_WORDBREAK, DeleteObject, DrawTextW, GetDC,
    HBRUSH, HFONT, HGDIOBJ, ReleaseDC, SelectObject,
};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    DACL_SECURITY_INFORMATION, GetUserObjectSecurity, LABEL_SECURITY_INFORMATION,
    OBJECT_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS,
    DESKTOP_SWITCHDESKTOP, GetUserObjectInformationW, HDESK, OpenDesktopW, OpenInputDesktop,
    SetThreadDesktop, SwitchDesktop, UOI_NAME,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_SYSTEM_AWARE, GetDpiForSystem, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics, HMENU, HWND_TOPMOST, IDC_ARROW,
    IsDialogMessageW, KillTimer, LoadCursorW, MSG, NONCLIENTMETRICSW, PostQuitMessage,
    RegisterClassExW, SM_CXSCREEN, SM_CYSCREEN, SPI_GETNONCLIENTMETRICS, SW_SHOW, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageW, SetTimer,
    SetWindowPos, ShowWindow, SystemParametersInfoW, TranslateMessage, UnregisterClassW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WM_TIMER,
    WNDCLASSEXW, WS_CAPTION, WS_CHILD, WS_DISABLED, WS_EX_DLGMODALFRAME, WS_EX_TOPMOST, WS_POPUP,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, PWSTR};

use super::state::Descriptor;
use super::{failed, wide};

/// The class of the prompt window. The harness of the elevated CI job looks for it from the
/// default desktop and must not find it.
pub const WINDOW_CLASS: &str = "VoltConfirmWindow";

/// How long both buttons stay disabled after the window appears.
pub const BUTTON_DELAY: Duration = Duration::from_millis(1500);

/// The ACL of the private desktop: full access for Administrators only, nothing inherited, and a
/// High integrity label so a lower-integrity program has no write access either.
const DESKTOP_SDDL: &str = "D:P(A;;GA;;;BA)S:(ML;;NW;;;HI)";

/// Everything the window shows, already in the language of the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptText {
    /// The title bar.
    pub title: String,
    /// The line that says what is about to happen, naming the tweak.
    pub heading: String,
    /// What the change means, in plain words.
    pub body: String,
    /// A small line under the buttons (that nobody answering counts as no).
    pub footer: String,
    /// The accept button.
    pub accept: String,
    /// The cancel button (the default one).
    pub cancel: String,
}

/// How a question ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PromptAnswer {
    /// A person pressed the accept button.
    Accepted,
    /// A person pressed Cancel, Escape or closed the window.
    Declined,
    /// Nobody answered in time.
    TimedOut,
    /// The question could not be shown at all, for this reason. Nothing was shown.
    Unavailable(String),
    /// The question was shown, but the input could not be put back on the person's desktop (not
    /// even on `Default`). Whatever was clicked does not count. The caller ends the process: that
    /// destroys the private desktop and Windows returns the input to the default one.
    SwitchBackFailed(String),
}

// ---------------------------------------------------------------------------------------------
// The desktop

/// A desktop that only Administrators can open, destroyed when this value is dropped.
#[derive(Debug)]
pub struct PrivateDesktop {
    handle: HDESK,
    name: String,
}

// SAFETY: a desktop handle is a process-wide kernel handle; the kernel serialises operations on it.
unsafe impl Send for PrivateDesktop {}
// SAFETY: as above; the handle is only read (copied) through a shared reference.
unsafe impl Sync for PrivateDesktop {}

impl PrivateDesktop {
    /// Creates a desktop in this process's window station, named at random.
    ///
    /// # Errors
    /// A message when the descriptor, the name or the desktop cannot be made.
    pub fn create() -> Result<Self, String> {
        let name = format!("VoltConfirm-{}", random_suffix()?);
        let descriptor = Descriptor::parse(DESKTOP_SDDL)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: descriptor.raw().0,
            bInheritHandle: false.into(),
        };
        let wide_name = wide(&name);
        // SAFETY: the name is NUL-terminated and the attributes point at the descriptor above; both
        // outlive the call, which copies the descriptor. The device is null (not used).
        let handle = unsafe {
            CreateDesktopW(
                PCWSTR(wide_name.as_ptr()),
                PCWSTR::null(),
                None,
                DESKTOP_CONTROL_FLAGS(0),
                0x1000_0000, // GENERIC_ALL
                Some(&raw const attributes),
            )
        }
        .map_err(|e| failed("CreateDesktopW", &e))?;
        Ok(Self { handle, name })
    }

    /// The desktop's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The desktop's DACL and label as SDDL text, as Windows reports them now.
    ///
    /// # Errors
    /// A message when the descriptor cannot be read.
    pub fn security_sddl(&self) -> Result<String, String> {
        let wanted = (DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION).0;
        let mut needed = 0_u32;
        // SAFETY: a size query: no buffer is given and `needed` is a valid out pointer. The call
        // is expected to fail with "insufficient buffer" while filling `needed`.
        let _ = unsafe {
            GetUserObjectSecurity(
                HANDLE(self.handle.0),
                &raw const wanted,
                None,
                0,
                &raw mut needed,
            )
        };
        if needed == 0 {
            return Err("GetUserObjectSecurity gave no size".to_owned());
        }
        let mut buffer = vec![0_u8; needed as usize];
        // SAFETY: the buffer has the size the first call asked for and `needed` is a valid out
        // pointer.
        unsafe {
            GetUserObjectSecurity(
                HANDLE(self.handle.0),
                &raw const wanted,
                Some(PSECURITY_DESCRIPTOR(buffer.as_mut_ptr().cast())),
                needed,
                &raw mut needed,
            )
        }
        .map_err(|e| failed("GetUserObjectSecurity", &e))?;
        let mut text = PWSTR::null();
        // SAFETY: the buffer holds a self-relative descriptor that the call just wrote; `text`
        // is a valid out pointer, and the string it receives is freed below.
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                PSECURITY_DESCRIPTOR(buffer.as_mut_ptr().cast()),
                SDDL_REVISION_1,
                OBJECT_SECURITY_INFORMATION(wanted),
                &raw mut text,
                None,
            )
        }
        .map_err(|e| failed("ConvertSecurityDescriptorToStringSecurityDescriptorW", &e))?;
        // SAFETY: `text` is a NUL-terminated string allocated by the call above.
        let result = unsafe { text.to_string() }.map_err(|e| e.to_string());
        // SAFETY: the string was allocated with LocalAlloc by the conversion call and is freed once.
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        result
    }

    fn handle(&self) -> HDESK {
        self.handle
    }
}

impl Drop for PrivateDesktop {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateDesktopW and is closed once, here. A failure leaves
        // nothing more to do (the desktop goes when its last user does).
        let _ = unsafe { CloseDesktop(self.handle) };
    }
}

/// Whether a desktop of this name exists in this window station and can be opened for reading.
#[must_use]
pub fn desktop_exists(name: &str) -> bool {
    let wide_name = wide(name);
    // SAFETY: the name is NUL-terminated and outlives the call.
    match unsafe {
        OpenDesktopW(
            PCWSTR(wide_name.as_ptr()),
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_READOBJECTS.0,
        )
    } {
        Ok(handle) => {
            // SAFETY: the handle was just opened and is closed once.
            let _ = unsafe { CloseDesktop(handle) };
            true
        }
        Err(_) => false,
    }
}

/// The name of the desktop that receives input now (`Default` in a normal session).
///
/// # Errors
/// A message when it cannot be read (the session is locked, or this is not an interactive window
/// station).
pub fn input_desktop_name() -> Result<String, String> {
    let desktop = InputDesktop::open()?;
    desktop.name()
}

/// A handle to the desktop that receives input, closed on drop.
struct InputDesktop(HDESK);

impl InputDesktop {
    fn open() -> Result<Self, String> {
        // SAFETY: no pointers are passed.
        unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_SWITCHDESKTOP) }
            .map(Self)
            .map_err(|e| failed("OpenInputDesktop", &e))
    }

    fn name(&self) -> Result<String, String> {
        let mut buffer = [0_u16; 256];
        let mut needed = 0_u32;
        // SAFETY: the buffer is valid for the stated byte length and `needed` is a valid out
        // pointer.
        unsafe {
            GetUserObjectInformationW(
                HANDLE(self.0.0),
                UOI_NAME,
                Some(buffer.as_mut_ptr().cast()),
                u32::try_from(std::mem::size_of_val(&buffer)).unwrap_or(0),
                Some(&raw mut needed),
            )
        }
        .map_err(|e| failed("GetUserObjectInformationW", &e))?;
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        Ok(String::from_utf16_lossy(&buffer[..length]))
    }
}

impl Drop for InputDesktop {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenInputDesktop and is closed once, here.
        let _ = unsafe { CloseDesktop(self.0) };
    }
}

/// How many times the switch back to the person's desktop is tried before the fallback.
pub const SWITCH_BACK_TRIES: u32 = 5;
/// The pause between two tries.
const SWITCH_BACK_PAUSE: Duration = Duration::from_millis(300);
/// The name of the session's ordinary desktop, the last place to try.
const DEFAULT_DESKTOP: &str = "Default";

/// Calls `attempt` until it succeeds or `tries` calls have failed, waiting `pause` between two
/// calls with `sleep`. Returns the last error.
fn retry<S>(
    tries: u32,
    pause: Duration,
    mut attempt: impl FnMut() -> Result<(), String>,
    sleep: S,
) -> Result<(), String>
where
    S: Fn(Duration),
{
    let mut last = Err("no attempt was made".to_owned());
    for index in 0..tries {
        last = attempt();
        if last.is_ok() {
            return last;
        }
        if index + 1 < tries {
            sleep(pause);
        }
    }
    last
}

/// Puts the input back where the person was: the desktop that was the input desktop before, tried
/// [`SWITCH_BACK_TRIES`] times, then the `Default` desktop by name, tried the same way.
fn switch_back(back: &InputDesktop) -> Result<(), String> {
    let first = retry(
        SWITCH_BACK_TRIES,
        SWITCH_BACK_PAUSE,
        || {
            // SAFETY: `back` is a valid handle to the desktop that was the input desktop before.
            unsafe { SwitchDesktop(back.0) }.map_err(|e| failed("SwitchDesktop", &e))
        },
        std::thread::sleep,
    );
    let Err(first_error) = first else {
        return Ok(());
    };
    log::error!("could not switch back to the user's desktop: {first_error}; trying `Default`");
    retry(
        SWITCH_BACK_TRIES,
        SWITCH_BACK_PAUSE,
        || {
            let name = wide(DEFAULT_DESKTOP);
            // SAFETY: the name is NUL-terminated and outlives the call.
            let desktop = unsafe {
                OpenDesktopW(
                    PCWSTR(name.as_ptr()),
                    DESKTOP_CONTROL_FLAGS(0),
                    false,
                    DESKTOP_SWITCHDESKTOP.0,
                )
            }
            .map_err(|e| failed("OpenDesktopW", &e))?;
            // SAFETY: the handle was just opened with switch access.
            let switched =
                unsafe { SwitchDesktop(desktop) }.map_err(|e| failed("SwitchDesktop", &e));
            // SAFETY: the handle was just opened and is closed once.
            let _ = unsafe { CloseDesktop(desktop) };
            switched
        },
        std::thread::sleep,
    )
    .map_err(|second_error| format!("{first_error}; {second_error}"))
}

/// Makes `to` the input desktop and puts the input back on `back` when it ends: through
/// [`Switched::restore`], which says whether it worked, or on drop as a last resort.
struct Switched<'a> {
    back: &'a InputDesktop,
    restored: bool,
}

impl<'a> Switched<'a> {
    fn to(desktop: &PrivateDesktop, back: &'a InputDesktop) -> Result<Self, String> {
        // SAFETY: the handle is a valid desktop handle with switch access (created with full
        // access).
        unsafe { SwitchDesktop(desktop.handle()) }.map_err(|e| failed("SwitchDesktop", &e))?;
        Ok(Self {
            back,
            restored: false,
        })
    }

    /// Puts the input back on the person's desktop, with the retries of [`switch_back`].
    fn restore(&mut self) -> Result<(), String> {
        let result = switch_back(self.back);
        if result.is_ok() {
            self.restored = true;
        }
        result
    }
}

impl Drop for Switched<'_> {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = switch_back(self.back)
        {
            log::error!("could not switch back to the user's desktop: {error}");
        }
    }
}

/// Hands the window thread and the desktop to a thread that closes the desktop handle once the
/// window thread has ended. A desktop that has a thread attached cannot be closed, so closing it
/// here would leave the handle open for the life of the process.
fn reap(worker: std::thread::JoinHandle<()>, desktop: PrivateDesktop) {
    let reaper = std::thread::Builder::new()
        .name("volt-confirm-reaper".to_owned())
        .spawn(move || {
            let _ = worker.join();
            drop(desktop);
        });
    if let Err(error) = reaper {
        log::error!("the prompt desktop could not be handed to a reaper: {error}");
    }
}

fn random_suffix() -> Result<String, String> {
    let name = super::args::PipeName::random().map_err(|e| format!("{e:?}"))?;
    // The random part of a pipe name (`volt-` and 32 hex digits).
    Ok(name
        .as_str()
        .trim_start_matches("volt-")
        .chars()
        .take(16)
        .collect())
}

// ---------------------------------------------------------------------------------------------
// The window

/// `SS_LEFT`: static text, left aligned.
const SS_LEFT: u32 = 0;
const ID_ACCEPT: i32 = 1001;
const ID_CANCEL: i32 = 2;
const TIMER_ENABLE: usize = 1;
const TIMER_EXPIRE: usize = 2;
const TIMER_POLL: usize = 3;

/// What the window procedure of this thread needs.
struct Ui {
    accept: HWND,
    cancel: HWND,
    shown_at: Instant,
    abort: Arc<AtomicBool>,
}

thread_local! {
    /// The window of this thread (a thread has at most one prompt).
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    /// How the question ended, set by the window procedure.
    static ANSWER: Cell<Option<PromptAnswerKind>> = const { Cell::new(None) };
}

/// The answers a window can end with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptAnswerKind {
    Accepted,
    Declined,
    TimedOut,
    Aborted,
}

fn finish(window: HWND, kind: PromptAnswerKind) {
    // The first answer stands: a second event (a timer firing as a click arrives) changes nothing.
    ANSWER.with(|answer| {
        if answer.get().is_none() {
            answer.set(Some(kind));
        }
    });
    // SAFETY: `window` is the window of this thread, which is destroyed by the thread that
    // owns it.
    let _ = unsafe { DestroyWindow(window) };
}

/// The window procedure of the prompt.
///
/// # Safety
/// Called by Windows on the thread that owns the window, with the message's own parameters.
unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_TIMER => {
            match wparam.0 {
                TIMER_ENABLE => {
                    // SAFETY: the timer id was set on this window.
                    let _ = unsafe { KillTimer(Some(window), TIMER_ENABLE) };
                    let buttons =
                        UI.with(|ui| ui.borrow().as_ref().map(|ui| (ui.accept, ui.cancel)));
                    if let Some((accept, cancel)) = buttons {
                        // SAFETY: both are child windows of this window, owned by this thread.
                        unsafe {
                            let _ = EnableWindow(accept, true);
                            let _ = EnableWindow(cancel, true);
                            let _ = SetFocus(Some(cancel));
                        }
                    }
                }
                TIMER_EXPIRE => finish(window, PromptAnswerKind::TimedOut),
                TIMER_POLL => {
                    let aborted = UI.with(|ui| {
                        ui.borrow()
                            .as_ref()
                            .is_some_and(|ui| ui.abort.load(Ordering::SeqCst))
                    });
                    if aborted {
                        finish(window, PromptAnswerKind::Aborted);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = i32::try_from(wparam.0 & 0xFFFF).unwrap_or(0);
            let notification = (wparam.0 >> 16) & 0xFFFF;
            // BN_CLICKED is 0. Only a click counts, from the button that is still enabled, and
            // only after the delay: belt and braces over the disabled state.
            let from = HWND(lparam.0 as *mut _);
            let accepted = UI.with(|ui| {
                ui.borrow().as_ref().is_some_and(|ui| {
                    from == ui.accept
                        // SAFETY: the button is a child of this window, owned by this thread.
                        && unsafe { IsWindowEnabled(ui.accept) }.as_bool()
                        && ui.shown_at.elapsed() >= BUTTON_DELAY
                })
            });
            if notification == 0 && id == ID_ACCEPT && accepted {
                finish(window, PromptAnswerKind::Accepted);
            } else if id == ID_CANCEL {
                finish(window, PromptAnswerKind::Declined);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            finish(window, PromptAnswerKind::Declined);
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends the message loop of this thread.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: the default handling of a message this procedure does not handle.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

/// A desktop handle that may move to the window thread.
struct SendDesktop(HDESK);
// SAFETY: see `PrivateDesktop`: a desktop handle is process-wide.
unsafe impl Send for SendDesktop {}

fn font_from(face: &windows::Win32::Graphics::Gdi::LOGFONTW) -> Option<HFONT> {
    // SAFETY: the face is a valid LOGFONTW.
    let font = unsafe { CreateFontIndirectW(face) };
    (!font.is_invalid()).then_some(font)
}

/// Text height in pixels for `text` wrapped to `width` pixels in `font`.
fn measure(font: HFONT, text: &str, width: i32) -> i32 {
    let mut units: Vec<u16> = text.encode_utf16().collect();
    if units.is_empty() {
        return 0;
    }
    // SAFETY: the screen DC is released below; the font is selected and the old object restored.
    unsafe {
        let dc = GetDC(None);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: 0,
        };
        DrawTextW(
            dc,
            &mut units,
            &raw mut rect,
            DT_CALCRECT | DT_WORDBREAK | DT_LEFT,
        );
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        rect.bottom - rect.top
    }
}

/// The three fonts of the prompt, deleted on drop.
struct Fonts {
    body: HFONT,
    heading: HFONT,
    small: HFONT,
}

impl Fonts {
    /// The system's message font, in three sizes.
    fn create() -> Result<Self, String> {
        let mut metrics = NONCLIENTMETRICSW {
            cbSize: u32::try_from(std::mem::size_of::<NONCLIENTMETRICSW>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `metrics` has its size set and is valid for the stated size.
        unsafe {
            SystemParametersInfoW(
                SPI_GETNONCLIENTMETRICS,
                metrics.cbSize,
                Some((&raw mut metrics).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .map_err(|e| failed("SystemParametersInfoW", &e))?;
        let body_face = metrics.lfMessageFont;
        let mut heading_face = body_face;
        heading_face.lfWeight = 700;
        heading_face.lfHeight = heading_face.lfHeight * 12 / 10;
        let mut small_face = body_face;
        small_face.lfHeight = small_face.lfHeight * 9 / 10;
        match (
            font_from(&body_face),
            font_from(&heading_face),
            font_from(&small_face),
        ) {
            (Some(body), Some(heading), Some(small)) => Ok(Self {
                body,
                heading,
                small,
            }),
            _ => Err("the system font could not be made".to_owned()),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for font in [self.body, self.heading, self.small] {
            // SAFETY: the fonts were made by this value and nothing uses them any more.
            let _ = unsafe { DeleteObject(HGDIOBJ(font.0)) };
        }
    }
}

/// Where everything goes, in pixels at the system DPI.
struct Layout {
    margin: i32,
    content: i32,
    button_w: i32,
    button_h: i32,
    gap: i32,
    heading_h: i32,
    body_h: i32,
    footer_h: i32,
    client_w: i32,
    client_h: i32,
}

impl Layout {
    fn new(fonts: &Fonts, text: &PromptText) -> Self {
        // SAFETY: no pointers.
        let dpi = i32::try_from(unsafe { GetDpiForSystem() })
            .unwrap_or(96)
            .max(96);
        let scale = |value: i32| value * dpi / 96;
        let (margin, content, gap) = (scale(16), scale(420), scale(10));
        let (button_w, button_h) = (scale(120), scale(30));
        let heading_h = measure(fonts.heading, &text.heading, content);
        let body_h = measure(fonts.body, &text.body, content);
        let footer_h = measure(fonts.small, &text.footer, content);
        Self {
            margin,
            content,
            button_w,
            button_h,
            gap,
            heading_h,
            body_h,
            footer_h,
            client_w: content + 2 * margin,
            client_h: margin + heading_h + gap + body_h + gap + button_h + gap + footer_h + margin,
        }
    }
}

const FRAME: WINDOW_STYLE = WINDOW_STYLE(WS_POPUP.0 | WS_CAPTION.0 | WS_SYSMENU.0);
const FRAME_EX: WINDOW_EX_STYLE = WINDOW_EX_STYLE(WS_EX_TOPMOST.0 | WS_EX_DLGMODALFRAME.0);

/// Registers the class and makes the top-level window, centred in the upper third of the screen.
fn make_window(
    instance: HINSTANCE,
    class: &[u16],
    title: &str,
    layout: &Layout,
) -> Result<HWND, String> {
    // SAFETY: the stock arrow cursor.
    let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default();
    let class_def = WNDCLASSEXW {
        cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(0),
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        // `COLOR_WINDOW + 1` is the system's window colour as a brush.
        hbrBackground: HBRUSH(6 as *mut _),
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    // SAFETY: the class definition points at the NUL-terminated name, which outlives the call, and
    // at `window_proc`, which lives for the whole process.
    if unsafe { RegisterClassExW(&raw const class_def) } == 0 {
        let error = windows::core::Error::from_thread();
        if error.code() != ERROR_CLASS_ALREADY_EXISTS.to_hresult() {
            return Err(failed("RegisterClassExW", &error));
        }
    }
    let mut frame = RECT {
        left: 0,
        top: 0,
        right: layout.client_w,
        bottom: layout.client_h,
    };
    // SAFETY: `frame` is a valid rectangle.
    let _ = unsafe { AdjustWindowRectEx(&raw mut frame, FRAME, false, FRAME_EX) };
    let (width, height) = (frame.right - frame.left, frame.bottom - frame.top);
    // SAFETY: no pointers.
    let (screen_w, screen_h) =
        unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let title = wide(title);
    // SAFETY: the class and title are NUL-terminated and outlive the call.
    unsafe {
        CreateWindowExW(
            FRAME_EX,
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            FRAME,
            ((screen_w - width) / 2).max(0),
            ((screen_h - height) / 3).max(0),
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|e| failed("CreateWindowExW", &e))
}

/// What a child control is: its class, text, style, id, place and font.
struct Control<'a> {
    class: &'a str,
    label: &'a str,
    style: u32,
    id: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    font: HFONT,
}

/// Makes one child control of `window`.
fn add_control(window: HWND, instance: HINSTANCE, control: &Control<'_>) -> HWND {
    let (class, label) = (wide(control.class), wide(control.label));
    let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(control.style);
    // The control id travels as the menu handle, as the API specifies.
    let id = HMENU(control.id.unsigned_abs() as usize as *mut _);
    // SAFETY: the class and label are NUL-terminated and outlive the call; the parent is the window
    // made by the caller.
    let made = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR(label.as_ptr()),
            style,
            control.x,
            control.y,
            control.w,
            control.h,
            Some(window),
            Some(id),
            Some(instance),
            None,
        )
    }
    .unwrap_or_default();
    if !made.is_invalid() {
        // SAFETY: the font outlives the window; the control is redrawn with it.
        unsafe {
            SendMessageW(
                made,
                WM_SETFONT,
                Some(WPARAM(control.font.0 as usize)),
                Some(LPARAM(1)),
            );
        }
    }
    made
}

/// The texts and the two buttons. The buttons start disabled; Cancel is the default one. Returns
/// (accept, cancel).
fn add_controls(
    window: HWND,
    instance: HINSTANCE,
    text: &PromptText,
    fonts: &Fonts,
    layout: &Layout,
) -> (HWND, HWND) {
    let label = |label: &str, font: HFONT, y: i32, h: i32| {
        add_control(
            window,
            instance,
            &Control {
                class: "STATIC",
                label,
                style: SS_LEFT,
                id: 0,
                x: layout.margin,
                y,
                w: layout.content,
                h,
                font,
            },
        );
    };
    let mut y = layout.margin;
    label(&text.heading, fonts.heading, y, layout.heading_h);
    y += layout.heading_h + layout.gap;
    label(&text.body, fonts.body, y, layout.body_h);
    y += layout.body_h + layout.gap;
    let button = |label: &str, style: u32, id: i32, x: i32| {
        add_control(
            window,
            instance,
            &Control {
                class: "BUTTON",
                label,
                style: style | WS_TABSTOP.0 | WS_DISABLED.0,
                id,
                x,
                y,
                w: layout.button_w,
                h: layout.button_h,
                font: fonts.body,
            },
        )
    };
    let right = layout.margin + layout.content;
    let cancel = button(
        &text.cancel,
        BS_DEFPUSHBUTTON as u32,
        ID_CANCEL,
        right - 2 * layout.button_w - layout.gap,
    );
    let accept = button(
        &text.accept,
        BS_PUSHBUTTON as u32,
        ID_ACCEPT,
        right - layout.button_w,
    );
    y += layout.button_h + layout.gap;
    label(&text.footer, fonts.small, y, layout.footer_h);
    (accept, cancel)
}

/// Starts the three timers, shows the window and runs the message loop until it is destroyed.
fn run_window(window: HWND, timeout: Duration) {
    let millis = |duration: Duration| u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: timers on this thread's own window.
    unsafe {
        SetTimer(Some(window), TIMER_ENABLE, millis(BUTTON_DELAY), None);
        SetTimer(Some(window), TIMER_EXPIRE, millis(timeout), None);
        SetTimer(Some(window), TIMER_POLL, 100, None);
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetWindowPos(
            window,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
    let mut message = MSG::default();
    // SAFETY: the standard message loop of the thread that owns the window; `IsDialogMessageW`
    // gives Tab, Enter and Escape their dialog meaning.
    unsafe {
        while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
            if !IsDialogMessageW(window, &raw const message).as_bool() {
                let _ = TranslateMessage(&raw const message);
                DispatchMessageW(&raw const message);
            }
        }
    }
}

/// Builds the window on this thread, tells `ready`, and runs it. Everything it makes is released
/// when it returns.
fn build_and_run(
    text: &PromptText,
    timeout: Duration,
    abort: Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    let fonts = Fonts::create()?;
    let layout = Layout::new(&fonts, text);
    // SAFETY: a null module name gives this executable's handle.
    let instance = HINSTANCE(
        unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|e| failed("GetModuleHandleW", &e))?
            .0,
    );
    let class = wide(WINDOW_CLASS);
    let window = make_window(instance, &class, &text.title, &layout)?;
    let (accept, cancel) = add_controls(window, instance, text, &fonts, &layout);
    UI.with(|ui| {
        *ui.borrow_mut() = Some(Ui {
            accept,
            cancel,
            shown_at: Instant::now(),
            abort,
        });
    });
    ANSWER.with(|answer| answer.set(None));
    let _ = ready.send(Ok(()));
    run_window(window, timeout);
    UI.with(|ui| *ui.borrow_mut() = None);
    // SAFETY: the class was registered by this thread and its window is destroyed.
    let _ = unsafe { UnregisterClassW(PCWSTR(class.as_ptr()), Some(instance)) };
    Ok(())
}

/// Runs the window on `desktop` and returns how the question ended. Runs on its own thread.
fn window_thread(
    desktop: &SendDesktop,
    text: &PromptText,
    timeout: Duration,
    abort: Arc<AtomicBool>,
    ready: &mpsc::Sender<Result<(), String>>,
) -> PromptAnswerKind {
    // The thread must be attached to the private desktop before it makes any window.
    // SAFETY: the thread has no window or hook yet; the handle is a valid desktop handle.
    if let Err(error) = unsafe { SetThreadDesktop(desktop.0) } {
        let _ = ready.send(Err(failed("SetThreadDesktop", &error)));
        return PromptAnswerKind::Aborted;
    }
    // SAFETY: no pointers; only this thread is affected.
    let _ = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_SYSTEM_AWARE) };
    if let Err(error) = build_and_run(text, timeout, abort, ready) {
        let _ = ready.send(Err(error));
        return PromptAnswerKind::Aborted;
    }
    // Without an answer (the loop ended for another reason) nothing was accepted.
    ANSWER.with(Cell::get).unwrap_or(PromptAnswerKind::Declined)
}

/// Asks the person the question in `text` on a private desktop and returns how it ended. Waits at
/// most `timeout` (plus a little to wind the window down); an unanswered question is
/// [`PromptAnswer::TimedOut`].
#[must_use]
pub fn ask(text: &PromptText, timeout: Duration) -> PromptAnswer {
    let desktop = match PrivateDesktop::create() {
        Ok(desktop) => desktop,
        Err(error) => return PromptAnswer::Unavailable(error),
    };
    // Where to come back to: read before anything is shown, so a session that cannot be switched
    // back (locked, no interactive desktop) never gets a prompt.
    let original = match InputDesktop::open() {
        Ok(original) => original,
        Err(error) => return PromptAnswer::Unavailable(error),
    };
    let abort = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let thread_desktop = SendDesktop(desktop.handle());
    let thread_text = text.clone();
    let thread_abort = Arc::clone(&abort);
    let worker = std::thread::Builder::new()
        .name("volt-confirm".to_owned())
        .spawn(move || {
            let kind = window_thread(
                &thread_desktop,
                &thread_text,
                timeout,
                thread_abort,
                &ready_tx,
            );
            let _ = done_tx.send(kind);
        });
    let Ok(worker) = worker else {
        return PromptAnswer::Unavailable("the prompt thread could not be started".to_owned());
    };

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = worker.join();
            return PromptAnswer::Unavailable(error);
        }
        Err(_) => {
            // The thread may still be building its window: stop it as soon as it can be, and close
            // the desktop only after it has ended.
            abort.store(true, Ordering::SeqCst);
            reap(worker, desktop);
            return PromptAnswer::Unavailable("the prompt window did not appear".to_owned());
        }
    }

    // The window exists on the private desktop. Make it the one the person sees.
    let mut switched = match Switched::to(&desktop, &original) {
        Ok(switched) => switched,
        Err(error) => {
            abort.store(true, Ordering::SeqCst);
            if done_rx.recv_timeout(Duration::from_secs(5)).is_ok() {
                let _ = worker.join();
                drop(desktop);
            } else {
                reap(worker, desktop);
            }
            return PromptAnswer::Unavailable(error);
        }
    };
    let kind = done_rx.recv_timeout(timeout + Duration::from_secs(10));
    if kind.is_err() {
        abort.store(true, Ordering::SeqCst);
    }
    // Back to the person's desktop before anything else, then tidy up.
    let restored = switched.restore();
    drop(switched);
    if kind.is_ok() {
        let _ = worker.join();
        drop(desktop);
    } else {
        reap(worker, desktop);
    }
    if let Err(error) = restored {
        return PromptAnswer::SwitchBackFailed(error);
    }
    match kind {
        Ok(PromptAnswerKind::Accepted) => PromptAnswer::Accepted,
        Ok(PromptAnswerKind::Declined) => PromptAnswer::Declined,
        Ok(PromptAnswerKind::TimedOut) | Err(_) => PromptAnswer::TimedOut,
        Ok(PromptAnswerKind::Aborted) => {
            PromptAnswer::Unavailable("the prompt was cancelled".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_acl_grants_administrators_only() {
        // Only Administrators, nothing inherited (P), and a High label: no other principal.
        assert!(DESKTOP_SDDL.starts_with("D:P(A;;GA;;;BA)"));
        assert_eq!(DESKTOP_SDDL.matches("(A;").count(), 1);
    }

    #[test]
    fn a_failed_switch_back_is_retried_and_stops_at_the_first_success() {
        let calls = Cell::new(0_u32);
        let pauses = Cell::new(0_u32);
        let result = retry(
            5,
            Duration::from_millis(1),
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    Err(format!("busy {}", calls.get()))
                } else {
                    Ok(())
                }
            },
            |_| pauses.set(pauses.get() + 1),
        );
        assert_eq!(result, Ok(()));
        assert_eq!(calls.get(), 3);
        assert_eq!(
            pauses.get(),
            2,
            "it waits between tries, not after the last"
        );
    }

    #[test]
    fn a_switch_back_that_never_works_gives_the_last_error_after_every_try() {
        let calls = Cell::new(0_u32);
        let result = retry(
            SWITCH_BACK_TRIES,
            Duration::ZERO,
            || {
                calls.set(calls.get() + 1);
                Err(format!("denied {}", calls.get()))
            },
            |_| {},
        );
        assert_eq!(result, Err(format!("denied {SWITCH_BACK_TRIES}")));
        assert_eq!(calls.get(), SWITCH_BACK_TRIES);
        assert!(retry(0, Duration::ZERO, || Ok(()), |_| {}).is_err());
    }

    #[test]
    fn the_timers_and_ids_are_distinct() {
        assert_ne!(TIMER_ENABLE, TIMER_EXPIRE);
        assert_ne!(TIMER_EXPIRE, TIMER_POLL);
        assert_ne!(ID_ACCEPT, ID_CANCEL);
    }
}
