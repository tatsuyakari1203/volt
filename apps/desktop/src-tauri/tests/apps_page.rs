//! The Apps commands over a real core: the listing and the removal path end to end.
//!
//! Nothing here touches this machine's apps. The registry the manager reads is a throwaway key
//! under `HKCU\Software\VoltTest\Apps-...` (the three locations moved there by
//! [`AppLocation::sandboxed_in_hkcu`]), the packaged apps are an in-memory fake and the
//! "uninstaller" is a double that deletes one key of that sandbox. No real uninstaller is ever
//! started. The test that reads the real machine is opt-in, read-only and prints counts only.

use std::process::Command as Process;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use volt_core::apps::fakes::FakePackages;
use volt_core::clean::Location;
use volt_core::clean::fakes::FakeKnownFolders;
use volt_core::fakes::FakeSystem;
use volt_core::{
    AppLocation, AppManager, Executor, LeftoverCleaner, PortError, Registry, RegistryHive,
    RegistryKeyRef, RegistryValue, StopWaiting, SystemClock, UninstallCommand, Uninstaller,
    UninstallerExit,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::list_activity;
use volt_desktop_lib::apps::{
    AppSourceView, ConfidenceView, HiddenReasonView, LeftoverKindView, UninstallOutcomeView,
    list_apps_of, remove_leftovers_of, scan_leftovers_of, stop_waiting_of, uninstall_app_of,
};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::wiring::{AppCore, build_core};
use volt_win::{WinFileSystem, WinRegistry};

/// A throwaway key under `HKCU\Software\VoltTest`, removed on drop.
struct Sandbox {
    root: String,
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self {
            root: format!(r"Software\VoltTest\Apps-{}-{nanos}", std::process::id()),
            dir: TempDir::new().expect("temp dir"),
        }
    }

    fn entry(&self, place: &str, key_name: &str, values: &[(&str, RegistryValue)]) {
        let key = RegistryKeyRef::new(
            RegistryHive::CurrentUser,
            format!(r"{}\{place}\{key_name}", self.root),
            volt_core::RegistryView::Native,
        );
        for (name, value) in values {
            WinRegistry::new()
                .write_value(&key.value(*name), value)
                .expect("arrange an entry");
        }
    }

    fn exists(&self, place: &str, key_name: &str) -> bool {
        WinRegistry::new()
            .key_exists(
                RegistryHive::CurrentUser,
                &format!(r"{}\{place}\{key_name}", self.root),
                volt_core::RegistryView::Native,
            )
            .expect("read")
    }

    /// A core whose apps manager reads the sandbox and can look for leftovers over the real
    /// registry and file system, with the user's folders pointed into the temp dir.
    fn core_with_leftovers(&self) -> AppCore {
        let home = self.dir.path();
        let system = FakeSystem::new();
        let core = build_core(system.ports(), &home.join("volt.db"), EMBEDDED_CATALOGUE)
            .expect("core builds");
        let mut ports = system.ports();
        ports.registry = Arc::new(WinRegistry::new());
        ports.file_system = Arc::new(WinFileSystem::new());
        let folders = Arc::new(FakeKnownFolders::new());
        folders.set(Location::UserProfile, home.join("profile"));
        folders.set(Location::LocalAppData, home.join("profile").join("Local"));
        folders.set(
            Location::RoamingAppData,
            home.join("profile").join("Roaming"),
        );
        folders.set(Location::LocalLow, home.join("profile").join("LocalLow"));
        folders.set(Location::ProgramData, home.join("ProgramData"));
        folders.set(Location::Windows, home.join("Windows"));
        folders.set_system_folders(vec![home.join("Windows"), home.join("ProgramData")]);
        let cleaner = LeftoverCleaner::new(
            ports.file_system.clone(),
            folders,
            Executor::new(ports, core.journal.clone(), Arc::new(SystemClock))
                .with_activity_log(core.activity.clone()),
            Arc::new(SystemClock),
        )
        .with_activity_log(core.activity.clone());
        let manager = AppManager::new(
            Arc::new(WinRegistry::new()),
            Arc::new(SandboxUninstaller {
                root: self.root.clone(),
            }),
            Arc::new(SystemClock),
        )
        .with_locations(AppLocation::sandboxed_in_hkcu(&self.root))
        .with_activity_log(core.activity.clone())
        .with_leftovers(cleaner);
        core.with_apps(manager)
    }

    /// A core over fakes (never the real machine) with the apps manager reading the sandbox.
    fn core(&self, packages: Arc<FakePackages>) -> AppCore {
        let system = FakeSystem::new();
        let core = build_core(
            system.ports(),
            &self.dir.path().join("volt.db"),
            EMBEDDED_CATALOGUE,
        )
        .expect("core builds");
        let manager = AppManager::new(
            Arc::new(WinRegistry::new()),
            Arc::new(SandboxUninstaller {
                root: self.root.clone(),
            }),
            Arc::new(SystemClock),
        )
        .with_locations(AppLocation::sandboxed_in_hkcu(&self.root))
        .with_packages(packages)
        .with_activity_log(core.activity.clone());
        core.with_apps(manager)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // The path is checked first, so this can only name a key below
        // `HKCU\Software\VoltTest\Apps-`.
        assert!(
            self.root.starts_with(r"Software\VoltTest\Apps-"),
            "refusing to delete {}",
            self.root
        );
        let _ = Process::new("reg")
            .args(["delete", &format!(r"HKCU\{}", self.root), "/f"])
            .output();
    }
}

/// Pretends to be a vendor's uninstaller: the program `C:\Vendor\remove-<key>.exe` deletes the
/// key `<key>` of the sandbox's user place. It starts no real program.
struct SandboxUninstaller {
    root: String,
}

impl Uninstaller for SandboxUninstaller {
    fn run(
        &self,
        command: &UninstallCommand,
        _stop: &StopWaiting,
    ) -> Result<UninstallerExit, PortError> {
        let program = command.program().to_lowercase();
        let Some(key) = program
            .strip_prefix(r"c:\vendor\remove-")
            .and_then(|rest| rest.strip_suffix(".exe"))
        else {
            return Err(PortError::NotFound("that program".to_owned()));
        };
        assert!(
            self.root.starts_with(r"Software\VoltTest\Apps-"),
            "refusing to delete {}",
            self.root
        );
        let target = format!(r"HKCU\{}\user\{key}", self.root);
        let status = Process::new("reg")
            .args(["delete", &target, "/f"])
            .output()
            .map_err(|error| PortError::Other(error.to_string()))?
            .status;
        Ok(UninstallerExit::complete(status.code()))
    }
}

fn sz(text: &str) -> RegistryValue {
    RegistryValue::Sz(text.to_owned())
}

#[test]
fn stopping_the_wait_with_nothing_running_is_harmless_and_does_not_cut_the_next_removal() {
    let sandbox = Sandbox::new();
    sandbox.entry(
        "user",
        "Mine",
        &[
            ("DisplayName", sz("Mine")),
            ("UninstallString", sz(r"C:\Vendor\remove-mine.exe")),
        ],
    );
    let core = sandbox.core(Arc::new(FakePackages::new()));
    stop_waiting_of(&core).expect("accepted");
    let result = uninstall_app_of(&core, "win32:user:Mine", false).expect("runs");
    assert_eq!(result.outcome, UninstallOutcomeView::Removed);
}

#[test]
fn the_list_comes_from_the_registry_with_hidden_reasons_and_one_app_is_removed_and_recorded() {
    let sandbox = Sandbox::new();
    assert!(sandbox.root.starts_with(r"Software\VoltTest\"));
    sandbox.entry(
        "machine64",
        "Vendor Tool",
        &[
            ("DisplayName", sz("Vendor Tool")),
            ("Publisher", sz("Vendor")),
            ("DisplayVersion", sz("3.1")),
            ("EstimatedSize", RegistryValue::Dword(1024)),
            ("InstallDate", sz("20240229")),
            (
                "UninstallString",
                sz(r#""C:\Vendor\remove-vendor-tool.exe" /x"#),
            ),
        ],
    );
    sandbox.entry(
        "machine32",
        "Core Runtime",
        &[
            ("DisplayName", sz("Core Runtime")),
            ("SystemComponent", RegistryValue::Dword(1)),
            ("UninstallString", sz("MsiExec.exe /X{CORE}")),
        ],
    );
    sandbox.entry(
        "user",
        "Mine",
        &[
            ("DisplayName", sz("Mine")),
            ("UninstallString", sz(r"C:\Vendor\remove-mine.exe")),
        ],
    );
    let packages = Arc::new(FakePackages::new());
    packages.add("Contoso.Notes_1.0_x64__abc", "Notes");
    let core = sandbox.core(packages.clone());

    let list = list_apps_of(&core).expect("lists");
    assert!(list.problems.is_empty(), "{:?}", list.problems);
    let names: Vec<&str> = list.apps.iter().map(|app| app.name.as_str()).collect();
    assert_eq!(names, ["Core Runtime", "Mine", "Notes", "Vendor Tool"]);
    let tool = list
        .apps
        .iter()
        .find(|a| a.name == "Vendor Tool")
        .expect("tool");
    assert_eq!(tool.id, "win32:m64:Vendor Tool");
    assert_eq!(tool.source, AppSourceView::Machine64);
    assert_eq!(tool.publisher.as_deref(), Some("Vendor"));
    assert_eq!(tool.version.as_deref(), Some("3.1"));
    assert_eq!(tool.size_bytes, Some(1024.0 * 1024.0));
    assert_eq!(tool.installed.as_deref(), Some("2024-02-29"));
    assert!(tool.uninstallable && tool.hidden.is_none());
    let runtime = list
        .apps
        .iter()
        .find(|a| a.name == "Core Runtime")
        .expect("runtime");
    assert_eq!(runtime.hidden, Some(HiddenReasonView::SystemComponent));
    assert_eq!(runtime.source, AppSourceView::Machine32);
    let mine = list.apps.iter().find(|a| a.name == "Mine").expect("mine");
    assert_eq!(mine.source, AppSourceView::CurrentUser);

    let done = uninstall_app_of(&core, &mine.id, false).expect("removed");
    assert_eq!(done.outcome, UninstallOutcomeView::Removed);
    assert!(done.recorded);
    assert!(done.listing.apps.iter().all(|app| app.name != "Mine"));
    assert!(!sandbox.exists("user", "Mine"));
    assert!(sandbox.exists("machine64", "Vendor Tool"));

    let page = list_activity(&core, 10, None).expect("activity");
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        page.items[0].summary,
        "Uninstalled Mine (cannot be undone by Volt)"
    );
    assert_eq!(page.items[0].label.as_deref(), Some("apps.uninstall"));
    assert!(page.items[0].entry.is_none(), "not an undoable change");

    // A packaged app goes through the package manager double.
    let notes = uninstall_app_of(&core, "pkg:Contoso.Notes_1.0_x64__abc", false).expect("notes");
    assert_eq!(notes.outcome, UninstallOutcomeView::Removed);
    assert_eq!(packages.removed(), ["Contoso.Notes_1.0_x64__abc"]);
}

#[test]
fn an_unknown_id_and_an_app_that_cannot_be_removed_are_refused_before_anything_runs() {
    let sandbox = Sandbox::new();
    sandbox.entry(
        "user",
        "Locked",
        &[
            ("DisplayName", sz("Locked")),
            ("NoRemove", RegistryValue::Dword(1)),
            ("UninstallString", sz(r"C:\Vendor\remove-locked.exe")),
        ],
    );
    sandbox.entry(
        "user",
        "Hidden",
        &[
            ("DisplayName", sz("Hidden")),
            ("SystemComponent", RegistryValue::Dword(1)),
            ("UninstallString", sz(r"C:\Vendor\remove-hidden.exe")),
        ],
    );
    let core = sandbox.core(Arc::new(FakePackages::new()));
    assert!(matches!(
        uninstall_app_of(&core, "win32:user:Nope", false),
        Err(IpcError::NotFound { .. })
    ));
    assert!(matches!(
        uninstall_app_of(&core, "win32:user:Locked", false),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(sandbox.exists("user", "Locked"));
    // A system component stays out of reach unless the caller says system entries are shown.
    assert!(matches!(
        uninstall_app_of(&core, "win32:user:Hidden", false),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(sandbox.exists("user", "Hidden"));
    assert!(
        list_activity(&core, 10, None)
            .expect("activity")
            .items
            .is_empty()
    );
}

#[test]
fn a_core_without_the_apps_manager_answers_unsupported() {
    let dir = TempDir::new().expect("temp dir");
    let core = build_core(
        FakeSystem::new().ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");
    assert!(matches!(
        list_apps_of(&core),
        Err(IpcError::Unsupported { .. })
    ));
    assert!(matches!(
        uninstall_app_of(&core, "win32:m64:x", false),
        Err(IpcError::Unsupported { .. })
    ));
}

/// The leftover scan and removal over the real registry (a throwaway key) and the real file
/// system (a temp folder), with the user's folders pointed into the temp dir. Only the registry
/// key is removed here: moving a folder would reach the real Recycle Bin.
#[test]
fn leftovers_of_a_removed_app_are_found_from_its_entry_and_a_chosen_key_is_removed_through_the_journal()
 {
    let sandbox = Sandbox::new();
    let home = sandbox.dir.path();
    let install = home.join("Programs").join("Mine");
    std::fs::create_dir_all(install.join("data")).expect("install folder");
    std::fs::write(install.join("data").join("a.bin"), [0_u8; 1500]).expect("file");
    let settings_path = format!(r"{}\data\Settings", sandbox.root);
    let settings = RegistryKeyRef::new(
        RegistryHive::CurrentUser,
        settings_path.clone(),
        volt_core::RegistryView::Native,
    );
    let registry = WinRegistry::new();
    registry
        .write_value(&settings.value("Theme"), &sz("dark"))
        .expect("arrange a key");
    sandbox.entry(
        "user",
        "Mine",
        &[
            ("DisplayName", sz("Mine")),
            ("UninstallString", sz(r"C:\Vendor\remove-mine.exe")),
            ("InstallLocation", sz(&install.display().to_string())),
            ("SettingsKey", sz(&format!(r"HKCU\{settings_path}"))),
        ],
    );

    let core = sandbox.core_with_leftovers();

    // Nothing is known about an app Volt did not remove.
    assert!(matches!(
        scan_leftovers_of(&core, "win32:user:Mine"),
        Err(IpcError::NotFound { .. })
    ));
    let result = uninstall_app_of(&core, "win32:user:Mine", false).expect("runs");
    assert_eq!(result.outcome, UninstallOutcomeView::Removed);
    assert!(result.leftovers_available);

    let scan = scan_leftovers_of(&core, "win32:user:Mine").expect("scan");
    assert_eq!(scan.app_name, "Mine");
    let folder = scan
        .items
        .iter()
        .find(|item| item.kind == LeftoverKindView::Folder)
        .expect("the install folder");
    assert_eq!(folder.confidence, ConfidenceView::High);
    assert!(folder.selected_by_default);
    assert!((folder.size_bytes - 1500.0).abs() < f64::EPSILON);
    let key = scan
        .items
        .iter()
        .find(|item| item.kind == LeftoverKindView::RegistryKey)
        .expect("the key the entry named");
    assert_eq!(key.confidence, ConfidenceView::High);
    assert!(key.locked.is_none());

    assert!(matches!(
        remove_leftovers_of(&core, "win32:user:Mine", &[]),
        Err(IpcError::InvalidInput { .. })
    ));
    let removal = remove_leftovers_of(&core, "win32:user:Mine", std::slice::from_ref(&key.id))
        .expect("removes the key");
    assert_eq!(removal.removed.len(), 1);
    assert!(removal.skipped.is_empty());
    assert!(
        !registry
            .key_exists(settings.hive, &settings.subkey, settings.view)
            .expect("read")
    );
    assert!(
        install.join("data").join("a.bin").exists(),
        "the folder was not chosen"
    );
    let batches = core
        .journal
        .batches_with_label(volt_core::LEFTOVERS_LABEL)
        .expect("journal");
    assert_eq!(
        batches.len(),
        1,
        "the removal is a batch the Undo page can revert"
    );
}

/// Reads the real machine's apps (the three registry places and the packaged apps) and prints
/// counts only, never a name. Nothing is started or removed. Opt in with `--ignored`.
#[test]
#[ignore = "reads the real machine; prints counts only"]
fn the_real_list_can_be_read_and_counts_are_plausible() {
    let manager = AppManager::new(
        Arc::new(WinRegistry::new()),
        Arc::new(NeverRuns),
        Arc::new(SystemClock),
    )
    .with_packages(Arc::new(volt_win::WinPackagedApps::new()));
    let listing = manager.list();
    let count = |wanted: fn(&volt_core::InstalledApp) -> bool| {
        listing.apps.iter().filter(|app| wanted(app)).count()
    };
    println!(
        "apps: {}, machine64: {}, machine32: {}, user: {}, packaged: {}, hidden: {}, \
         uninstallable: {}, with size: {}, with date: {}, with publisher: {}, problems: {}",
        listing.apps.len(),
        count(|a| a.source == volt_core::AppSource::Machine64),
        count(|a| a.source == volt_core::AppSource::Machine32),
        count(|a| a.source == volt_core::AppSource::CurrentUser),
        count(|a| a.source == volt_core::AppSource::Packaged),
        count(|a| a.hidden.is_some()),
        count(volt_core::InstalledApp::uninstallable),
        count(|a| a.size_bytes.is_some()),
        count(|a| a.installed.is_some()),
        count(|a| a.publisher.is_some()),
        listing.problems.len(),
    );
    assert!(!listing.apps.is_empty());
}

/// Refuses to run anything, so the opt-in test cannot start an uninstaller.
struct NeverRuns;

impl Uninstaller for NeverRuns {
    fn run(
        &self,
        _command: &UninstallCommand,
        _stop: &StopWaiting,
    ) -> Result<UninstallerExit, PortError> {
        Err(PortError::Unsupported("never runs".to_owned()))
    }
}
