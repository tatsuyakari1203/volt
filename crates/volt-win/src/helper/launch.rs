//! Starting the helper: whether it may be started, and the one call that starts it.
//!
//! # Availability
//!
//! [`status_for`] answers "may a shield action start the helper?" WITHOUT starting anything. It is
//! `Available` only when
//!
//! - this executable and the helper sit directly in the Program Files install folder and pass the
//!   install checks ([`verify_install`] on each, so the UI's own ACL is covered as well as the
//!   helper's);
//! - the user is an administrator who gets a consent prompt (or no prompt at all, by their own
//!   policy), not a standard user who would be asked for another account's credentials.
//!
//! Each way to fail is its own [`UnavailableReason`]. The decision is the pure function [`decide`]
//! over [`Probes`], so every case is tested without a real install.
//!
//! # Launching
//!
//! [`ShellLauncher`] calls `ShellExecuteExW` with the `runas` verb on the sibling helper image and
//! keeps the process handle (`SEE_MASK_NOCLOSEPROCESS`), so the UI can later check that the pipe is
//! served by that very process. The path is derived from this process's own image path, never from
//! settings, the environment or the working directory. A cancelled prompt is
//! [`LaunchError::Declined`].
//!
//! The launch call sits behind the private [`Launcher`] trait so the session code can be tested
//! with the call replaced; no public constructor accepts another launcher.

use std::path::{Path, PathBuf};

use volt_core::{ElevationStatus, UnavailableReason};
use windows::Win32::Foundation::{ERROR_CANCELLED, HANDLE};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::core::PCWSTR;

use super::identity::{ProcessHandle, TokenKind, current_token_kind};
use super::install::{InstallRefusal, verify_install};
use super::{failed, wide};

/// File name of the helper, a sibling of the UI executable.
pub const HELPER_IMAGE_NAME: &str = "volt-helper.exe";

/// What the availability decision reads. Each probe is called only when the decision needs it, in
/// the order of the checks.
pub(crate) struct Probes<'a> {
    /// The install check of one executable (location, ACLs, reparse points).
    pub install: &'a dyn Fn(&Path) -> Result<(), InstallRefusal>,
    /// Whether a file exists.
    pub exists: &'a dyn Fn(&Path) -> bool,
    /// The elevation facts of the current token.
    pub token: &'a dyn Fn() -> Result<TokenKind, String>,
}

/// The reason an install check refused, as the typed reason.
fn install_reason(refusal: &InstallRefusal) -> UnavailableReason {
    match refusal {
        InstallRefusal::NotInProgramFiles { .. } => UnavailableReason::NotAllUsersInstall,
        other => UnavailableReason::InstallNotProtected {
            detail: other.to_string(),
        },
    }
}

/// Decides whether administrator features may be used, for the UI executable at `ui_image`. The
/// helper is the file of that name next to it. Pure over the probes; starts nothing.
pub(crate) fn decide(ui_image: &Path, probes: &Probes<'_>) -> ElevationStatus {
    let unavailable = |reason| ElevationStatus::Unavailable { reason };
    if let Err(refusal) = (probes.install)(ui_image) {
        return unavailable(install_reason(&refusal));
    }
    let helper = ui_image.with_file_name(HELPER_IMAGE_NAME);
    if !(probes.exists)(&helper) {
        return unavailable(UnavailableReason::HelperMissing);
    }
    if let Err(refusal) = (probes.install)(&helper) {
        return unavailable(install_reason(&refusal));
    }
    match (probes.token)() {
        Ok(TokenKind::Elevated | TokenKind::Limited) => ElevationStatus::Available,
        Ok(TokenKind::Standard) => unavailable(UnavailableReason::StandardUser),
        Err(detail) => unavailable(UnavailableReason::Unknown { detail }),
    }
}

/// Whether administrator features may be used, for the UI executable at `ui_image`, read from the
/// real machine. Reads only.
#[must_use]
pub fn status_for(ui_image: &Path) -> ElevationStatus {
    decide(
        ui_image,
        &Probes {
            install: &|image| verify_install(image).map(|_| ()),
            exists: &|path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()),
            token: &current_token_kind,
        },
    )
}

/// Why the launch call did not give a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchError {
    /// The user cancelled the administrator prompt.
    Declined,
    /// Windows could not start the helper (a policy that blocks elevation, a missing file, ...).
    Failed(String),
}

/// The call that starts the helper elevated. Private to the crate so nothing outside can supply
/// another one: the real [`ShellLauncher`] is the only one in a release build.
pub(crate) trait Launcher: Send + Sync {
    /// Starts `image` with `args` (the words after the program name) and returns its process.
    fn launch(&self, image: &Path, args: &[String]) -> Result<ProcessHandle, LaunchError>;
}

/// The real launcher: `ShellExecuteExW` with the `runas` verb.
#[derive(Debug, Default)]
pub(crate) struct ShellLauncher;

/// COM initialised for the call (the shell asks for it), balanced on drop.
struct ComScope {
    uninitialize: bool,
}

impl ComScope {
    fn enter() -> Self {
        // SAFETY: plain COM initialisation of the calling thread, balanced by `CoUninitialize` in
        // `Drop` when (and only when) it succeeded. A thread already in another mode is used as
        // it is.
        let result =
            unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Self {
            uninitialize: result.is_ok(),
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful `CoInitializeEx` in `enter`, on the same thread (the
            // guard lives on one stack frame).
            unsafe { CoUninitialize() };
        }
    }
}

/// The parameter string: the words joined by a space. They come from `HelperArgs::render` (a
/// fixed set of flags, a hex name and two decimals), so there is nothing to quote; anything that
/// would need quoting is refused rather than escaped.
fn parameter_string(args: &[String]) -> Result<String, LaunchError> {
    if args
        .iter()
        .any(|arg| arg.is_empty() || arg.chars().any(|c| c.is_whitespace() || c == '"'))
    {
        return Err(LaunchError::Failed(
            "the helper arguments are not plain words".to_owned(),
        ));
    }
    Ok(args.join(" "))
}

impl Launcher for ShellLauncher {
    fn launch(&self, image: &Path, args: &[String]) -> Result<ProcessHandle, LaunchError> {
        let parameters = wide(&parameter_string(args)?);
        let file = wide(&image.to_string_lossy());
        // The helper's own folder (admin-only), not whatever this process's working directory is.
        let directory: Option<Vec<u16>> = image
            .parent()
            .map(PathBuf::from)
            .map(|dir| wide(&dir.to_string_lossy()));
        let verb = wide("runas");
        let _com = ComScope::enter();
        let mut info = SHELLEXECUTEINFOW {
            cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(parameters.as_ptr()),
            lpDirectory: directory
                .as_ref()
                .map_or(PCWSTR::null(), |dir| PCWSTR(dir.as_ptr())),
            // SW_HIDE: the helper has no window.
            nShow: 0,
            ..SHELLEXECUTEINFOW::default()
        };
        // SAFETY: `info` is a fully initialised SHELLEXECUTEINFOW whose string pointers
        // (`verb`, `file`, `parameters`, `directory`) are NUL-terminated and outlive the call. The
        // call may show the administrator prompt and blocks until it is answered.
        let started = unsafe { ShellExecuteExW(&raw mut info) };
        match started {
            Ok(()) => {
                let handle: HANDLE = info.hProcess;
                ProcessHandle::from_launch(handle).ok_or_else(|| {
                    LaunchError::Failed("Windows gave no process handle for the helper".to_owned())
                })
            }
            Err(error) if error.code() == ERROR_CANCELLED.to_hresult() => {
                Err(LaunchError::Declined)
            }
            Err(error) => Err(LaunchError::Failed(failed("ShellExecuteExW", &error))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const UI: &str = r"C:\Program Files\Volt\volt-desktop.exe";
    const HELPER: &str = r"C:\Program Files\Volt\volt-helper.exe";

    fn refusal() -> InstallRefusal {
        InstallRefusal::NotInProgramFiles {
            expected: PathBuf::from(r"C:\Program Files\Volt"),
            actual: PathBuf::from(r"C:\Users\me\Volt"),
        }
    }

    fn weak() -> InstallRefusal {
        InstallRefusal::Unreadable {
            path: PathBuf::from(r"C:\Program Files\Volt"),
            reason: "denied".to_owned(),
        }
    }

    /// Runs the decision with a table of results: which paths fail the install check, whether the
    /// helper exists and what the token is.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "a table of cases reads best with the values written in the call"
    )]
    fn run(
        ui_fails: Option<InstallRefusal>,
        helper_exists: bool,
        helper_fails: Option<InstallRefusal>,
        token: Result<TokenKind, String>,
    ) -> (ElevationStatus, u32) {
        let token_reads = Cell::new(0_u32);
        let status = decide(
            Path::new(UI),
            &Probes {
                install: &|path| {
                    let fails = if path == Path::new(HELPER) {
                        &helper_fails
                    } else {
                        &ui_fails
                    };
                    fails.clone().map_or(Ok(()), Err)
                },
                exists: &|path| path == Path::new(HELPER) && helper_exists,
                token: &|| {
                    token_reads.set(token_reads.get() + 1);
                    token.clone()
                },
            },
        );
        (status, token_reads.get())
    }

    fn reason(status: &ElevationStatus) -> &UnavailableReason {
        match status {
            ElevationStatus::Unavailable { reason } => reason,
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn a_protected_install_and_an_administrator_is_available() {
        for kind in [TokenKind::Limited, TokenKind::Elevated] {
            let (status, _) = run(None, true, None, Ok(kind));
            assert_eq!(status, ElevationStatus::Available, "{kind:?}");
        }
    }

    #[test]
    fn a_copy_outside_program_files_is_its_own_reason() {
        let (status, token_reads) = run(Some(refusal()), true, None, Ok(TokenKind::Limited));
        assert_eq!(reason(&status), &UnavailableReason::NotAllUsersInstall);
        assert_eq!(
            token_reads, 0,
            "nothing else is looked at once the folder is refused"
        );
    }

    #[test]
    fn a_writable_or_unreadable_ui_executable_or_folder_is_refused_with_the_detail() {
        let (status, _) = run(Some(weak()), true, None, Ok(TokenKind::Limited));
        match reason(&status) {
            UnavailableReason::InstallNotProtected { detail } => {
                assert!(detail.contains("cannot be checked"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_helper_is_its_own_reason() {
        let (status, _) = run(None, false, None, Ok(TokenKind::Limited));
        assert_eq!(reason(&status), &UnavailableReason::HelperMissing);
    }

    #[test]
    fn a_writable_helper_is_refused_even_when_the_ui_is_fine() {
        let (status, _) = run(None, true, Some(weak()), Ok(TokenKind::Limited));
        assert!(matches!(
            reason(&status),
            UnavailableReason::InstallNotProtected { .. }
        ));
    }

    #[test]
    fn a_standard_user_is_told_to_ask_an_administrator_and_an_unreadable_token_is_unknown() {
        let (status, _) = run(None, true, None, Ok(TokenKind::Standard));
        assert_eq!(reason(&status), &UnavailableReason::StandardUser);
        let (status, _) = run(None, true, None, Err("no token".to_owned()));
        assert_eq!(
            reason(&status),
            &UnavailableReason::Unknown {
                detail: "no token".to_owned()
            }
        );
    }

    #[test]
    fn the_status_of_a_build_folder_copy_is_not_an_all_users_install() {
        // The real checks, on the real machine, for this test binary: it is not in Program Files.
        // Reads only; nothing is started.
        let exe = std::env::current_exe().expect("exe");
        assert_eq!(
            reason(&status_for(&exe)),
            &UnavailableReason::NotAllUsersInstall
        );
    }

    #[test]
    fn only_plain_words_are_passed_to_the_launch_call() {
        let ok = [
            "serve".to_owned(),
            "--pipe".to_owned(),
            "volt-ab".to_owned(),
        ];
        assert_eq!(parameter_string(&ok).as_deref(), Ok("serve --pipe volt-ab"));
        for bad in ["a b", "a\"b", "", "a\tb"] {
            assert!(parameter_string(&[bad.to_owned()]).is_err(), "{bad:?}");
        }
    }
}
