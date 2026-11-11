use std::sync::Arc;

use super::fakes::{FakePackages, FakeUninstaller, add_entry};
use super::*;
use crate::activity::ActivityRecord;
use crate::fakes::{FakeActivityLog, FakeRegistry, ManualClock};
use crate::registry::{RegistryHive, RegistryKeyRef, RegistryValue, RegistryView};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

/// (key and display name, extra values, expected reason, whether it can be removed)
type Case = (
    &'static str,
    Vec<(&'static str, RegistryValue)>,
    Option<HiddenReason>,
    bool,
);

fn sz(text: &str) -> RegistryValue {
    RegistryValue::Sz(text.to_owned())
}

struct Fixture {
    registry: Arc<FakeRegistry>,
    uninstaller: Arc<FakeUninstaller>,
    packages: Arc<FakePackages>,
    activity: Arc<FakeActivityLog>,
    manager: AppManager,
}

fn fixture() -> Fixture {
    let registry = Arc::new(FakeRegistry::new());
    let uninstaller = Arc::new(FakeUninstaller::new());
    let packages = Arc::new(FakePackages::new());
    let activity = Arc::new(FakeActivityLog::new());
    let manager = AppManager::new(
        registry.clone(),
        uninstaller.clone(),
        Arc::new(ManualClock::new(1_000)),
    )
    .with_packages(packages.clone())
    .with_activity_log(activity.clone())
    .with_own_install(OwnInstall::new(Some(r"C:\Program Files\Volt".to_owned())));
    Fixture {
        registry,
        uninstaller,
        packages,
        activity,
        manager,
    }
}

fn key(hive: RegistryHive, view: RegistryView, name: &str) -> RegistryKeyRef {
    RegistryKeyRef::new(hive, format!(r"{UNINSTALL}\{}", name.to_lowercase()), view)
}

fn machine64(name: &str) -> RegistryKeyRef {
    key(RegistryHive::LocalMachine, RegistryView::Bit64, name)
}

impl Fixture {
    /// An ordinary app in the 64-bit machine location with a registered command.
    fn add(&self, key_name: &str, display: &str, command: &str) -> RegistryKeyRef {
        let at = machine64(key_name);
        add_entry(
            &self.registry,
            &at,
            &[
                ("DisplayName", sz(display)),
                ("UninstallString", sz(command)),
            ],
        );
        at
    }

    fn find(&self, id: &str) -> InstalledApp {
        self.manager
            .list()
            .apps
            .into_iter()
            .find(|app| app.id == id)
            .unwrap_or_else(|| panic!("{id} is listed"))
    }

    fn records(&self) -> Vec<ActivityRecord> {
        self.activity.records()
    }
}

#[test]
fn apps_of_the_three_locations_are_listed_with_their_details_sorted_by_name() {
    let f = fixture();
    let vendor = machine64("{GUID-1}");
    add_entry(
        &f.registry,
        &vendor,
        &[
            ("DisplayName", sz("Zeta Editor")),
            ("Publisher", sz("Zeta Ltd")),
            ("DisplayVersion", sz("2.5.1")),
            ("EstimatedSize", RegistryValue::Dword(2048)),
            ("InstallDate", sz("20240131")),
            ("UninstallString", sz(r#""C:\Zeta\unins000.exe""#)),
        ],
    );
    add_entry(
        &f.registry,
        &key(RegistryHive::LocalMachine, RegistryView::Bit32, "Alpha"),
        &[
            ("DisplayName", sz("alpha Tool")),
            ("InstallDate", RegistryValue::Dword(20_230_405)),
            ("UninstallString", sz("MsiExec.exe /X{ALPHA}")),
        ],
    );
    add_entry(
        &f.registry,
        &key(RegistryHive::CurrentUser, RegistryView::Native, "Mine"),
        &[
            ("DisplayName", sz("Mine")),
            (
                "UninstallString",
                sz(r"C:\Users\me\AppData\Local\Mine\u.exe"),
            ),
        ],
    );

    let listing = f.manager.list();
    let names: Vec<&str> = listing.apps.iter().map(|app| app.name.as_str()).collect();
    assert_eq!(names, ["alpha Tool", "Mine", "Zeta Editor"]);
    assert!(listing.problems.is_empty());

    let zeta = f.find("win32:m64:{guid-1}");
    assert_eq!(zeta.publisher.as_deref(), Some("Zeta Ltd"));
    assert_eq!(zeta.version.as_deref(), Some("2.5.1"));
    assert_eq!(zeta.size_bytes, Some(2048 * 1024));
    assert_eq!(
        zeta.installed.map(|d| d.to_string()),
        Some("2024-01-31".to_owned())
    );
    assert_eq!(zeta.source, AppSource::Machine64);
    assert_eq!(zeta.hidden, None);
    assert!(zeta.uninstallable());

    let alpha = f.find("win32:m32:alpha");
    assert_eq!(alpha.source, AppSource::Machine32);
    assert_eq!(
        alpha.installed.map(|d| d.to_string()),
        Some("2023-04-05".to_owned())
    );
    assert_eq!(alpha.size_bytes, None);
    assert_eq!(f.find("win32:user:mine").source, AppSource::CurrentUser);
}

#[test]
fn system_components_updates_and_entries_without_a_command_are_marked_hidden() {
    let f = fixture();
    let cases: [Case; 9] = [
        ("Plain", vec![], None, true),
        (
            "System",
            vec![("SystemComponent", RegistryValue::Dword(1))],
            Some(HiddenReason::SystemComponent),
            true,
        ),
        (
            "Patch",
            vec![("ParentKeyName", sz("OperatingSystem"))],
            Some(HiddenReason::Update),
            true,
        ),
        (
            "Hotfix",
            vec![("ReleaseType", sz("Security Update"))],
            Some(HiddenReason::Update),
            true,
        ),
        ("KB5001234", vec![], Some(HiddenReason::Update), true),
        (
            "Locked",
            vec![("NoRemove", RegistryValue::Dword(1))],
            Some(HiddenReason::NotRemovable),
            false,
        ),
        (
            "Quiet",
            vec![("UninstallString", sz(""))],
            Some(HiddenReason::NoUninstallCommand),
            false,
        ),
        (
            "Script",
            vec![("UninstallString", sz(r"C:\x\remove.bat /s"))],
            Some(HiddenReason::NoUninstallCommand),
            false,
        ),
        (
            "Network",
            vec![("UninstallString", sz(r"\\server\share\u.exe"))],
            Some(HiddenReason::NoUninstallCommand),
            false,
        ),
    ];
    for (name, extra, _, _) in &cases {
        let at = machine64(name);
        let mut values = vec![
            ("DisplayName", sz(name)),
            ("UninstallString", sz(r"C:\Vendor\u.exe")),
        ];
        values.extend(extra.iter().cloned());
        add_entry(&f.registry, &at, &values);
    }
    // Only a quiet command is registered: never used.
    let only_quiet = machine64("OnlyQuiet");
    add_entry(
        &f.registry,
        &only_quiet,
        &[
            ("DisplayName", sz("OnlyQuiet")),
            ("QuietUninstallString", sz(r"C:\Vendor\u.exe /S")),
        ],
    );
    // No display name: not an app.
    add_entry(
        &f.registry,
        &machine64("Nameless"),
        &[("UninstallString", sz(r"C:\Vendor\u.exe"))],
    );

    for (name, _, hidden, uninstallable) in &cases {
        let app = f.find(&format!("win32:m64:{}", name.to_lowercase()));
        assert_eq!(app.hidden, *hidden, "{name}");
        assert_eq!(app.uninstallable(), *uninstallable, "{name}");
    }
    let quiet = f.find("win32:m64:onlyquiet");
    assert_eq!(quiet.hidden, Some(HiddenReason::NoUninstallCommand));
    assert!(!quiet.uninstallable());
    assert!(
        f.manager
            .list()
            .apps
            .iter()
            .all(|app| app.id != "win32:m64:nameless")
    );
}

#[test]
fn volt_itself_is_never_listed() {
    let f = fixture();
    // By name.
    f.add("Volt", "Volt", r"C:\Elsewhere\u.exe");
    // By the folder of its uninstaller, whatever it is called.
    f.add(
        "Renamed",
        "Something Else",
        r#""C:\Program Files\Volt\uninstall.exe""#,
    );
    // By its install location.
    add_entry(
        &f.registry,
        &machine64("Located"),
        &[
            ("DisplayName", sz("Located")),
            ("InstallLocation", sz(r"c:\program files\volt\")),
            ("UninstallString", sz("MsiExec.exe /X{VOLT}")),
        ],
    );
    // A neighbour with a similar name and folder is not Volt.
    f.add(
        "Neighbour",
        "Volt Notes",
        r"C:\Program Files\Volt Notes\u.exe",
    );

    let names: Vec<String> = f.manager.list().apps.into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Volt Notes"]);
}

#[test]
fn packaged_apps_are_listed_and_frameworks_system_packages_and_resources_are_handled() {
    let f = fixture();
    f.packages.add("Contoso.Notes_1.0_x64__abc", "Notes");
    f.packages.add_package(PackagedApp {
        full_name: "Microsoft.VCLibs_14_x64__8we".to_owned(),
        name: "VC Libs".to_owned(),
        publisher: None,
        version: None,
        installed: None,
        size_bytes: None,
        framework: true,
        resource: false,
        system: false,
    });
    f.packages.add_package(PackagedApp {
        full_name: "Microsoft.Settings_1_x64__8we".to_owned(),
        name: "Settings".to_owned(),
        publisher: Some("Microsoft".to_owned()),
        version: None,
        installed: None,
        size_bytes: None,
        framework: false,
        resource: false,
        system: true,
    });
    f.packages.add_package(PackagedApp {
        full_name: "Contoso.Notes.Language_1_x64__abc".to_owned(),
        name: "Notes language".to_owned(),
        publisher: None,
        version: None,
        installed: None,
        size_bytes: None,
        framework: false,
        resource: true,
        system: false,
    });

    let listing = f.manager.list();
    let names: Vec<&str> = listing.apps.iter().map(|app| app.name.as_str()).collect();
    assert_eq!(names, ["Notes", "Settings", "VC Libs"]);

    let notes = f.find("pkg:Contoso.Notes_1.0_x64__abc");
    assert_eq!(notes.source, AppSource::Packaged);
    assert_eq!(notes.publisher.as_deref(), Some("Contoso"));
    assert_eq!(
        notes.installed.map(|d| d.to_string()),
        Some("2023-11-14".to_owned())
    );
    assert_eq!(notes.hidden, None);
    assert!(notes.uninstallable());

    let libs = f.find("pkg:Microsoft.VCLibs_14_x64__8we");
    assert_eq!(libs.hidden, Some(HiddenReason::Framework));
    assert!(!libs.uninstallable());
    let settings = f.find("pkg:Microsoft.Settings_1_x64__8we");
    assert_eq!(settings.hidden, Some(HiddenReason::NotRemovable));
    assert!(!settings.uninstallable());
}

#[test]
fn a_source_that_cannot_be_read_is_a_problem_and_the_rest_of_the_list_stays() {
    let f = fixture();
    f.add("One", "One", r"C:\Vendor\u.exe");
    f.packages
        .fail_list(PortError::AccessDenied("secret detail".into()));
    let listing = f.manager.list();
    assert_eq!(listing.apps.len(), 1);
    assert_eq!(
        listing.problems,
        [AppProblem {
            source: AppSource::Packaged,
            unreadable: true,
            skipped: 0
        }]
    );
}

#[test]
fn uninstalling_runs_the_registered_command_as_written_and_reports_removed() {
    let f = fixture();
    let at = f.add(
        "{GUID-9}",
        "Vendor App",
        r#""C:\Program Files\Vendor\unins000.exe" /LOG="C:\a b.log""#,
    );
    f.uninstaller.removes_key_when_run(
        r"C:\Program Files\Vendor\unins000.exe",
        f.registry.clone(),
        at,
    );

    let report = f
        .manager
        .uninstall("win32:m64:{guid-9}", Source::Manual)
        .expect("started");
    assert_eq!(report.outcome, UninstallOutcome::Removed);
    assert_eq!(report.exit_code, Some(0));
    assert_eq!(report.name, "Vendor App");
    assert!(report.listing.apps.is_empty());

    let runs = f.uninstaller.runs();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].program(), r"C:\Program Files\Vendor\unins000.exe");
    assert_eq!(runs[0].arguments(), r#"/LOG="C:\a b.log""#);

    let records = f.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome, ActivityOutcome::Applied);
    assert_eq!(records[0].label.as_deref(), Some(UNINSTALL_LABEL));
    assert_eq!(
        records[0].summary,
        "Uninstalled Vendor App (cannot be undone by Volt)"
    );
    assert!(!report.activity_write_failed);
}

#[test]
fn an_uninstaller_that_leaves_the_app_listed_is_reported_as_still_installed() {
    let f = fixture();
    f.add("Stubborn", "Stubborn App", r"C:\Vendor\u.exe /x");
    f.uninstaller.exit_with(r"C:\Vendor\u.exe", 1602);
    let report = f
        .manager
        .uninstall("win32:m64:stubborn", Source::Manual)
        .expect("started");
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
    assert_eq!(report.exit_code, Some(1602));
    assert_eq!(report.listing.apps.len(), 1);
    let records = f.records();
    assert_eq!(records[0].outcome, ActivityOutcome::Failed);
    assert_eq!(
        records[0].summary,
        "Ran the uninstaller for Stubborn App; it is still installed"
    );
}

#[test]
fn an_uninstaller_that_cannot_start_is_reported_without_a_message_from_the_port() {
    let f = fixture();
    f.add("Gone", "Gone App", r"C:\Vendor\missing.exe");
    f.uninstaller.fail_on(
        r"C:\Vendor\missing.exe",
        PortError::NotFound("C:\\Vendor\\missing.exe: secret path".into()),
    );
    let report = f
        .manager
        .uninstall("win32:m64:gone", Source::Manual)
        .expect("tried");
    assert_eq!(report.outcome, UninstallOutcome::Failed("not found"));
    assert_eq!(report.exit_code, None);
    assert!(f.uninstaller.runs().is_empty());
    let summary = &f.records()[0].summary;
    assert_eq!(summary, "Could not uninstall Gone App: not found");
    assert!(!summary.contains("secret"));
}

#[test]
fn a_packaged_app_is_removed_through_the_package_manager() {
    let f = fixture();
    f.packages.add("Contoso.Notes_1.0_x64__abc", "Notes");
    f.packages.add("Contoso.Keep_1.0_x64__abc", "Keep");
    f.packages.add("Contoso.Stuck_1.0_x64__abc", "Stuck");
    f.packages.keep_after_remove("Contoso.Stuck_1.0_x64__abc");
    f.packages.add("Contoso.Denied_1.0_x64__abc", "Denied");
    f.packages.fail_remove(
        "Contoso.Denied_1.0_x64__abc",
        PortError::AccessDenied("detail".into()),
    );

    let removed = f
        .manager
        .uninstall("pkg:Contoso.Notes_1.0_x64__abc", Source::Manual)
        .expect("started");
    assert_eq!(removed.outcome, UninstallOutcome::Removed);
    assert_eq!(removed.exit_code, None);
    assert_eq!(f.packages.removed(), ["Contoso.Notes_1.0_x64__abc"]);
    assert!(removed.listing.apps.iter().all(|app| app.name != "Notes"));

    let stuck = f
        .manager
        .uninstall("pkg:Contoso.Stuck_1.0_x64__abc", Source::Manual)
        .expect("started");
    assert_eq!(stuck.outcome, UninstallOutcome::StillInstalled);

    let denied = f
        .manager
        .uninstall("pkg:Contoso.Denied_1.0_x64__abc", Source::Manual)
        .expect("tried");
    assert_eq!(denied.outcome, UninstallOutcome::Failed("access denied"));

    assert!(f.uninstaller.runs().is_empty());
    assert_eq!(f.records().len(), 3);
}

#[test]
fn nothing_is_started_for_an_unknown_or_unremovable_entry() {
    let f = fixture();
    f.add("Script", "Script App", r"C:\x\remove.bat");
    add_entry(
        &f.registry,
        &machine64("Locked"),
        &[
            ("DisplayName", sz("Locked")),
            ("NoRemove", RegistryValue::Dword(1)),
            ("UninstallString", sz(r"C:\Vendor\u.exe")),
        ],
    );
    f.packages.add_package(PackagedApp {
        full_name: "Microsoft.VCLibs_14_x64__8we".to_owned(),
        name: "VC Libs".to_owned(),
        publisher: None,
        version: None,
        installed: None,
        size_bytes: None,
        framework: true,
        resource: false,
        system: false,
    });

    for id in [
        "win32:m64:script",
        "win32:m64:locked",
        "pkg:Microsoft.VCLibs_14_x64__8we",
    ] {
        assert_eq!(
            f.manager.uninstall(id, Source::Manual).unwrap_err(),
            UninstallError::NotUninstallable,
            "{id}"
        );
    }
    // An id is looked up, never run: a command in its place finds nothing.
    for id in ["win32:m64:nope", r"C:\Windows\System32\cmd.exe /c calc", ""] {
        assert_eq!(
            f.manager.uninstall(id, Source::Manual).unwrap_err(),
            UninstallError::NotFound,
            "{id}"
        );
    }
    assert!(f.uninstaller.runs().is_empty());
    assert!(f.packages.removed().is_empty());
    assert!(f.records().is_empty());
}

#[test]
fn a_system_component_or_an_update_is_removed_only_when_the_caller_asks_for_system_entries() {
    let f = fixture();
    let core = machine64("Core");
    add_entry(
        &f.registry,
        &core,
        &[
            ("DisplayName", sz("Core Runtime")),
            ("SystemComponent", RegistryValue::Dword(1)),
            ("UninstallString", sz(r"C:\Vendor\core.exe")),
        ],
    );
    add_entry(
        &f.registry,
        &machine64("KB5001234"),
        &[
            ("DisplayName", sz("Security Update")),
            ("UninstallString", sz(r"C:\Vendor\kb.exe")),
        ],
    );
    for id in ["win32:m64:core", "win32:m64:kb5001234"] {
        assert_eq!(
            f.manager.uninstall(id, Source::Manual).unwrap_err(),
            UninstallError::HiddenByDefault,
            "{id}"
        );
    }
    assert!(f.uninstaller.runs().is_empty());
    assert!(f.records().is_empty());

    f.uninstaller
        .removes_key_when_run(r"C:\Vendor\core.exe", f.registry.clone(), core);
    let report = f
        .manager
        .uninstall_including_system("win32:m64:core", Source::Manual)
        .expect("started");
    assert_eq!(report.outcome, UninstallOutcome::Removed);
    assert_eq!(f.uninstaller.runs().len(), 1);
}

#[test]
fn a_second_removal_while_one_runs_is_refused() {
    let f = fixture();
    f.add("A", "A", r"C:\Vendor\u.exe");
    let _running = f.manager.running.lock().expect("lock");
    assert_eq!(
        f.manager
            .uninstall("win32:m64:a", Source::Manual)
            .unwrap_err(),
        UninstallError::Busy
    );
    assert!(f.uninstaller.runs().is_empty());
}

#[test]
fn a_failing_activity_log_never_blocks_a_removal_and_is_reported() {
    let f = fixture();
    let at = f.add("A", "A", r"C:\Vendor\u.exe");
    f.uninstaller
        .removes_key_when_run(r"C:\Vendor\u.exe", f.registry.clone(), at);
    f.activity
        .fail_records_with(Some(PortError::Other("disk full".into())));
    let report = f
        .manager
        .uninstall("win32:m64:a", Source::Manual)
        .expect("started");
    assert_eq!(report.outcome, UninstallOutcome::Removed);
    assert!(report.activity_write_failed);
}

#[test]
fn dates_are_read_from_the_compact_text_and_impossible_ones_are_dropped() {
    assert_eq!(
        AppDate::parse_compact("20240131").map(|d| (d.year, d.month, d.day)),
        Some((2024, 1, 31))
    );
    for text in [
        "",
        "2024-01-31",
        "20241301",
        "20240100",
        "19000101",
        "2024013",
        "abcdefgh",
    ] {
        assert_eq!(AppDate::parse_compact(text), None, "{text}");
    }
    assert!(AppDate::parse_compact("20230405") < AppDate::parse_compact("20240101"));
}

/// A fixture whose manager polls fast, so a test of the fallback wait stays short.
fn quick_fixture(limit: std::time::Duration) -> Fixture {
    let mut f = fixture();
    f.manager.poll = std::time::Duration::from_millis(10);
    f.manager.limit = limit;
    f
}

#[test]
fn an_uninstaller_that_could_not_be_followed_is_waited_for_until_the_entry_is_gone() {
    let f = quick_fixture(std::time::Duration::from_secs(30));
    let at = f.add("Elevated", "Elevated App", r"C:\Vendor\e.exe");
    f.uninstaller
        .waited_as(r"C:\Vendor\e.exe", Waited::FirstProcess);
    let started = std::time::Instant::now();
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = f.registry.delete_key(&at);
        });
        f.manager
            .uninstall("win32:m64:elevated", Source::Manual)
            .expect("started")
    });
    assert!(started.elapsed() >= std::time::Duration::from_millis(100));
    assert_eq!(report.outcome, UninstallOutcome::Removed);
    assert_eq!(f.records()[0].outcome, ActivityOutcome::Applied);
}

#[test]
fn the_wait_for_an_uninstaller_that_could_not_be_followed_gives_up_at_its_limit() {
    let f = quick_fixture(std::time::Duration::from_millis(60));
    f.add("Slow", "Slow App", r"C:\Vendor\s.exe");
    f.uninstaller
        .waited_as(r"C:\Vendor\s.exe", Waited::FirstProcess);
    let report = f
        .manager
        .uninstall("win32:m64:slow", Source::Manual)
        .expect("started");
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
}

#[test]
fn stopping_ends_the_fallback_wait_at_once() {
    let f = quick_fixture(std::time::Duration::from_secs(600));
    f.add("Stuck", "Stuck App", r"C:\Vendor\k.exe");
    f.uninstaller
        .waited_as(r"C:\Vendor\k.exe", Waited::FirstProcess);
    let started = std::time::Instant::now();
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            f.manager.stop_waiting();
        });
        f.manager
            .uninstall("win32:m64:stuck", Source::Manual)
            .expect("started")
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
}

#[test]
fn stopping_reaches_an_uninstaller_that_is_still_being_waited_for() {
    let f = fixture();
    f.add("Open", "Open App", r"C:\Vendor\o.exe");
    f.uninstaller.runs_until_stopped(r"C:\Vendor\o.exe");
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            f.manager.stop_waiting();
        });
        f.manager
            .uninstall("win32:m64:open", Source::Manual)
            .expect("started")
    });
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
    assert_eq!(report.exit_code, None);
}

#[test]
fn a_stop_that_nothing_was_waiting_for_does_not_cut_the_next_wait_short() {
    let f = fixture();
    f.add("Next", "Next App", r"C:\Vendor\n.exe");
    f.uninstaller.runs_until_stopped(r"C:\Vendor\n.exe");
    f.manager.stop_waiting();
    // The earlier request is cleared when the removal starts, so this one waits for its own.
    let report = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            f.manager.stop_waiting();
        });
        let started = std::time::Instant::now();
        let report = f
            .manager
            .uninstall("win32:m64:next", Source::Manual)
            .expect("started");
        assert!(started.elapsed() >= std::time::Duration::from_millis(80));
        report
    });
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
}
