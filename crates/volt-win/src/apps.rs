//! Adapters for installed apps: the packaged (Store) apps of the current user and starting a
//! vendor's uninstaller.
//!
//! [`WinPackagedApps`] lists the current user's packages through the `WinRT` package manager (no
//! `PowerShell`) and removes one for the current user only. [`WinUninstaller`] starts the command an
//! app registered, exactly as registered, through `ShellExecuteExW` (so a program that asks for
//! administrator rights shows Windows' own prompt) and waits for the process to exit. Neither
//! runs a shell, and neither builds a command from anything but what the core passes in.

use std::path::{Component, Path, PathBuf};

use volt_core::{PackagedApp, PackagedApps, PortError, UninstallCommand, Uninstaller};
use windows::ApplicationModel::{Package, PackageSignatureKind};
use windows::Management::Deployment::PackageManager;
use windows::Win32::Foundation::{CloseHandle, HANDLE, RPC_E_CHANGED_MODE, WAIT_OBJECT_0};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    ShellExecuteExW,
};
use windows::core::{HSTRING, PCWSTR};

use crate::error::from_hresult;
use crate::version::expand_environment;
use crate::wide::to_wide_nul;

/// `SW_SHOWNORMAL`: the uninstaller's window opens the way its author meant.
const SW_SHOWNORMAL: i32 = 1;
/// Milliseconds from 1601-01-01 (the Windows epoch) to 1970-01-01.
const WINDOWS_TO_UNIX_MILLIS: i64 = 11_644_473_600_000;

/// Keeps COM initialized on the calling thread for as long as it lives.
struct ComScope {
    uninitialize: bool,
}

impl ComScope {
    fn enter() -> Result<Self, PortError> {
        // SAFETY: plain COM initialization of the calling thread; balanced by `CoUninitialize` in
        // `Drop` when (and only when) it succeeded.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if result.is_ok() {
            Ok(Self { uninitialize: true })
        } else if result == RPC_E_CHANGED_MODE {
            // The thread already runs COM in another mode: use it as it is.
            Ok(Self {
                uninitialize: false,
            })
        } else {
            Err(from_hresult(result.0, "initializing COM"))
        }
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: balances the successful `CoInitializeEx` of `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

fn winrt_error(error: &windows::core::Error, context: &str) -> PortError {
    from_hresult(error.code().0, context)
}

/// The packaged apps of the current user, through the `WinRT` package manager.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinPackagedApps;

impl WinPackagedApps {
    /// A packaged-apps adapter.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn text(value: windows::core::Result<HSTRING>) -> Option<String> {
    value
        .ok()
        .map(|text| text.to_string())
        .filter(|text| !text.trim().is_empty())
}

fn unix_millis(universal_time: i64) -> Option<i64> {
    // 100 ns ticks since 1601-01-01; zero or less is "never".
    (universal_time > 0).then(|| universal_time / 10_000 - WINDOWS_TO_UNIX_MILLIS)
}

/// What the package manager says about one package, or `None` when it has no identity.
fn describe(package: &Package) -> Option<PackagedApp> {
    let id = package.Id().ok()?;
    let full_name = text(id.FullName())?;
    let identity_name = text(id.Name())?;
    // A display name that is an unresolved resource reference (`ms-resource:...`) is no name.
    let name = text(package.DisplayName())
        .filter(|name| !name.starts_with("ms-resource:"))
        .unwrap_or(identity_name);
    Some(PackagedApp {
        full_name,
        name,
        publisher: text(package.PublisherDisplayName()),
        version: id
            .Version()
            .ok()
            .map(|v| format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision)),
        installed: package
            .InstalledDate()
            .ok()
            .and_then(|date| unix_millis(date.UniversalTime))
            .map(volt_core::Timestamp::from_unix_millis),
        size_bytes: None,
        framework: package.IsFramework().unwrap_or(false),
        resource: package.IsResourcePackage().unwrap_or(false)
            || package.IsBundle().unwrap_or(false),
        system: package
            .SignatureKind()
            .is_ok_and(|kind| kind == PackageSignatureKind::System),
    })
}

impl PackagedApps for WinPackagedApps {
    fn list(&self) -> Result<Vec<PackagedApp>, PortError> {
        let _com = ComScope::enter()?;
        let manager = PackageManager::new()
            .map_err(|error| winrt_error(&error, "opening the package manager"))?;
        // An empty user id is the current user, which needs no administrator rights.
        let packages = manager
            .FindPackagesByUserSecurityId(&HSTRING::new())
            .map_err(|error| winrt_error(&error, "listing the packaged apps"))?;
        Ok(packages
            .into_iter()
            .filter_map(|package| describe(&package))
            .collect())
    }

    fn remove(&self, full_name: &str) -> Result<(), PortError> {
        let _com = ComScope::enter()?;
        let context = "removing a packaged app";
        let manager = PackageManager::new()
            .map_err(|error| winrt_error(&error, "opening the package manager"))?;
        // `RemovePackageAsync` removes the package for the current user only.
        let operation = manager
            .RemovePackageAsync(&HSTRING::from(full_name))
            .map_err(|error| winrt_error(&error, context))?;
        let result = operation
            .join()
            .map_err(|error| winrt_error(&error, context))?;
        match result.ExtendedErrorCode() {
            Ok(code) if code.is_err() => Err(from_hresult(code.0, context)),
            _ => Ok(()),
        }
    }
}

/// Starts a vendor's registered uninstall command and waits for it.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinUninstaller;

impl WinUninstaller {
    /// An uninstaller launcher.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// The file to start for the program of a registered command: environment references expanded,
/// a missing `.exe` added, and a bare name looked up in the Windows system folder only. Anything
/// that is not an absolute local path afterwards is refused.
fn resolve_program(
    program: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
    system_folder: &Path,
) -> Result<PathBuf, PortError> {
    let expanded = expand_environment(program.trim(), lookup);
    let refuse = |why: &str| PortError::InvalidInput(format!("the uninstall program {why}"));
    if expanded.contains('%') {
        return Err(refuse("refers to an environment variable that is not set"));
    }
    if expanded.starts_with(r"\\") || expanded.starts_with("//") {
        return Err(refuse("is on a network path"));
    }
    let mut path = PathBuf::from(&expanded);
    if path.components().any(|part| part == Component::ParentDir) {
        return Err(refuse("has a parent folder reference"));
    }
    if path.extension().is_none() {
        path.set_extension("exe");
    }
    if path.is_absolute() {
        return Ok(path);
    }
    if path.components().count() == 1 {
        return Ok(system_folder.join(path));
    }
    Err(refuse("is not an absolute path"))
}

impl Uninstaller for WinUninstaller {
    fn run(&self, command: &UninstallCommand) -> Result<Option<i32>, PortError> {
        let system_folder = std::env::var_os("SystemRoot")
            .map(|root| Path::new(&root).join("System32"))
            .ok_or_else(|| PortError::NotFound("the Windows folder".to_owned()))?;
        let program = resolve_program(
            command.program(),
            &|name| std::env::var(name).ok(),
            &system_folder,
        )?;
        if !program.is_file() {
            return Err(PortError::NotFound("the uninstall program".to_owned()));
        }
        let file = to_wide_nul(&program.to_string_lossy(), "the uninstall program")?;
        let arguments = (!command.arguments().is_empty())
            .then(|| to_wide_nul(command.arguments(), "the uninstall arguments"))
            .transpose()?;
        let _com = ComScope::enter()?;
        let mut info = SHELLEXECUTEINFOW {
            cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>()).unwrap_or(0),
            fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: arguments
                .as_ref()
                .map_or(PCWSTR::null(), |wide| PCWSTR(wide.as_ptr())),
            nShow: SW_SHOWNORMAL,
            ..SHELLEXECUTEINFOW::default()
        };
        // SAFETY: `info` is fully initialised with its real size; `file` and `arguments` are
        // NUL-terminated UTF-16 buffers that live until the call returns, and the call is made
        // with no verb, so the file is opened as an ordinary program. The process handle it
        // returns is owned by this function and closed below.
        unsafe { ShellExecuteExW(&raw mut info) }
            .map_err(|error| winrt_error(&error, "starting the uninstaller"))?;
        wait_for_exit(info.hProcess)
    }
}

/// A process handle, closed on drop.
struct Process(HANDLE);

impl Drop for Process {
    fn drop(&mut self) {
        // SAFETY: the handle came from `ShellExecuteExW` with `SEE_MASK_NOCLOSEPROCESS`, is
        // owned exclusively by this value and is closed once, here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Waits for the process to exit and returns its exit code. An invalid handle (Windows handed the
/// file to a process that was already running) cannot be waited for.
fn wait_for_exit(process: HANDLE) -> Result<Option<i32>, PortError> {
    if process.is_invalid() {
        return Ok(None);
    }
    let process = Process(process);
    // SAFETY: `process.0` is a valid process handle owned by `process` for the whole call.
    let waited = unsafe { WaitForSingleObject(process.0, INFINITE) };
    if waited != WAIT_OBJECT_0 {
        return Err(PortError::Other(
            "waiting for the uninstaller failed".to_owned(),
        ));
    }
    let mut code = 0_u32;
    // SAFETY: the handle is valid and `code` is a live, writable u32.
    unsafe { GetExitCodeProcess(process.0, &raw mut code) }
        .map_err(|error| winrt_error(&error, "reading the uninstaller's exit code"))?;
    Ok(Some(code.cast_signed()))
}
#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(name: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "programfiles" => Some(r"C:\Program Files".to_owned()),
            _ => None,
        }
    }

    fn resolved(program: &str) -> Result<PathBuf, PortError> {
        resolve_program(program, &lookup, Path::new(r"C:\Windows\System32"))
    }

    #[test]
    fn environment_references_are_expanded_and_a_missing_extension_is_added() {
        assert_eq!(
            resolved(r"%ProgramFiles%\Vendor\remove").expect("resolves"),
            PathBuf::from(r"C:\Program Files\Vendor\remove.exe")
        );
        assert_eq!(
            resolved(r"D:\Apps\Tool\unins000.exe").expect("resolves"),
            PathBuf::from(r"D:\Apps\Tool\unins000.exe")
        );
    }

    #[test]
    fn a_bare_name_is_looked_up_in_the_system_folder_only() {
        assert_eq!(
            resolved("MsiExec.exe").expect("resolves"),
            PathBuf::from(r"C:\Windows\System32\MsiExec.exe")
        );
        assert_eq!(
            resolved("msiexec").expect("resolves"),
            PathBuf::from(r"C:\Windows\System32\msiexec.exe")
        );
    }

    #[test]
    fn anything_that_is_not_an_absolute_local_path_is_refused() {
        for program in [
            r"%Unset%\x.exe",
            r"\\server\share\x.exe",
            r"sub\x.exe",
            r"C:\a\..\b\x.exe",
        ] {
            assert!(
                matches!(resolved(program), Err(PortError::InvalidInput(_))),
                "{program}"
            );
        }
    }

    /// A harmless system program, not an uninstaller: proves the program is started as given and
    /// that its exit code comes back.
    #[test]
    fn a_started_program_is_waited_for_and_its_exit_code_returned() {
        let command = UninstallCommand::parse("cmd.exe /c exit 3").expect("accepted");
        assert_eq!(WinUninstaller::new().run(&command), Ok(Some(3)));
    }

    #[test]
    fn a_program_that_is_not_there_is_not_found_and_nothing_runs() {
        let command =
            UninstallCommand::parse(r"C:\VoltTest\does-not-exist\remove.exe /S").expect("accepted");
        assert!(matches!(
            WinUninstaller::new().run(&command),
            Err(PortError::NotFound(_))
        ));
    }

    /// Reads the real machine's packaged apps (read-only) and prints only a count. Opt in with
    /// `--ignored`.
    #[test]
    #[ignore = "reads the real machine; prints counts only"]
    fn the_real_package_list_can_be_read() {
        let packages = WinPackagedApps::new().list().expect("lists the packages");
        let apps = packages.iter().filter(|p| !p.resource).count();
        let frameworks = packages.iter().filter(|p| p.framework).count();
        let system = packages.iter().filter(|p| p.system).count();
        println!(
            "packages: {}, apps: {apps}, frameworks: {frameworks}, system: {system}",
            packages.len()
        );
        assert!(!packages.is_empty());
    }
}
