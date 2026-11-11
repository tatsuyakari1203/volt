//! The leftover scan and its removal, over in-memory fakes. Nothing here touches the machine.

use std::path::PathBuf;
use std::sync::Arc;

use super::fakes::{FakeUninstaller, add_entry};
use super::*;
use crate::clean::Location;
use crate::clean::fakes::FakeKnownFolders;
use crate::fakes::{FakeSystem, ManualClock};
use crate::registry::{RegistryHive, RegistryKeyRef, RegistryValue, RegistryView};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
const PROFILE: &str = r"C:\Users\me";
const LOCAL: &str = r"C:\Users\me\AppData\Local";
const ROAMING: &str = r"C:\Users\me\AppData\Roaming";

fn sz(text: &str) -> RegistryValue {
    RegistryValue::Sz(text.to_owned())
}

struct Fixture {
    system: FakeSystem,
    uninstaller: Arc<FakeUninstaller>,
    manager: AppManager,
    activity: Arc<crate::fakes::FakeActivityLog>,
}

fn fixture() -> Fixture {
    let system = FakeSystem::new();
    let folders = Arc::new(FakeKnownFolders::new());
    folders.set(Location::UserProfile, PROFILE);
    folders.set(Location::LocalAppData, LOCAL);
    folders.set(Location::RoamingAppData, ROAMING);
    folders.set(Location::LocalLow, r"C:\Users\me\AppData\LocalLow");
    folders.set(Location::ProgramData, r"C:\ProgramData");
    folders.set(Location::Windows, r"C:\Windows");
    folders.set_system_folders(vec![
        PathBuf::from(r"C:\Windows"),
        PathBuf::from(r"C:\Program Files"),
        PathBuf::from(r"C:\Program Files (x86)"),
        PathBuf::from(r"C:\ProgramData"),
    ]);
    folders.protect(PROFILE);
    folders.set_env("ProgramFiles", r"C:\Program Files");
    let activity = system.activity.clone();
    let uninstaller = Arc::new(FakeUninstaller::new());
    let cleaner = LeftoverCleaner::new(
        system.file_system.clone(),
        folders,
        system.executor(),
        Arc::new(ManualClock::new(5_000)),
    )
    .with_activity_log(activity.clone());
    let manager = AppManager::new(
        system.registry.clone(),
        uninstaller.clone(),
        Arc::new(ManualClock::new(1_000)),
    )
    .with_leftovers(cleaner)
    .with_own_install(OwnInstall::new(Some(r"C:\Program Files\Volt".to_owned())));
    Fixture {
        system,
        uninstaller,
        manager,
        activity,
    }
}

fn entry_key(hive: RegistryHive, view: RegistryView, name: &str) -> RegistryKeyRef {
    RegistryKeyRef::new(hive, format!(r"{UNINSTALL}\{}", name.to_lowercase()), view)
}

impl Fixture {
    /// An installed app, with `extra` values on its entry. Its uninstaller removes the entry.
    fn install(&self, key_name: &str, name: &str, extra: &[(&str, RegistryValue)]) -> String {
        let at = entry_key(RegistryHive::LocalMachine, RegistryView::Bit64, key_name);
        let program = format!(r"C:\Vendors\{key_name}\remove.exe");
        let mut values = vec![("DisplayName", sz(name)), ("UninstallString", sz(&program))];
        values.extend(extra.iter().cloned());
        add_entry(&self.system.registry, &at, &values);
        self.uninstaller
            .removes_key_when_run(&program, self.system.registry.clone(), at);
        format!("win32:m64:{}", key_name.to_lowercase())
    }

    fn uninstall(&self, id: &str) -> UninstallReport {
        self.manager
            .uninstall(id, Source::Manual)
            .expect("the uninstall starts")
    }

    fn scan(&self, id: &str) -> LeftoverScan {
        self.manager.scan_leftovers(id).expect("a scan")
    }
}

fn find<'a>(scan: &'a LeftoverScan, location: &str) -> &'a Leftover {
    scan.items
        .iter()
        .find(|item| item.location.eq_ignore_ascii_case(location))
        .unwrap_or_else(|| panic!("{location} is offered: {:?}", scan.items))
}

fn offered(scan: &LeftoverScan, location: &str) -> bool {
    scan.items
        .iter()
        .any(|item| item.location.eq_ignore_ascii_case(location))
}

#[test]
fn the_recorded_install_folder_is_high_selected_by_default_and_sized() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("InstallLocation", sz(r"C:\Program Files\Contoso Editor"))],
    );
    f.system
        .file_system
        .add_file(r"C:\Program Files\Contoso Editor\data\a.bin", 1000);
    f.system
        .file_system
        .add_file(r"C:\Program Files\Contoso Editor\b.cfg", 24);
    assert!(f.uninstall(&id).leftovers_available);

    let scan = f.scan(&id);
    let folder = find(&scan, r"C:\Program Files\Contoso Editor");
    assert_eq!(folder.confidence, Confidence::High);
    assert!(folder.selected_by_default());
    assert_eq!(folder.size_bytes, 1024);
    assert_eq!(folder.items, 2);
    assert_eq!(folder.kind, LeftoverKind::Folder);
    assert_eq!(folder.evidence[0].code, "leftover.why.installLocation");
    assert!(!scan.incomplete);
}

#[test]
fn nothing_is_offered_for_an_app_that_was_not_removed_by_volt_or_without_the_tools() {
    let f = fixture();
    let id = f.install("contoso", "Contoso Editor", &[]);
    assert_eq!(
        f.manager.scan_leftovers(&id).expect_err("no record"),
        LeftoverError::NoRecord
    );
    let bare = AppManager::new(
        f.system.registry.clone(),
        f.uninstaller.clone(),
        Arc::new(ManualClock::new(1)),
    );
    assert_eq!(
        bare.scan_leftovers(&id).expect_err("no tools"),
        LeftoverError::Unavailable
    );
}

#[test]
fn an_app_that_is_still_installed_has_no_leftover_record() {
    let f = fixture();
    let at = entry_key(RegistryHive::LocalMachine, RegistryView::Bit64, "stay");
    add_entry(
        &f.system.registry,
        &at,
        &[
            ("DisplayName", sz("Stay App")),
            ("UninstallString", sz(r"C:\Vendors\stay\remove.exe")),
        ],
    );
    let report = f.uninstall("win32:m64:stay");
    assert_eq!(report.outcome, UninstallOutcome::StillInstalled);
    assert!(!report.leftovers_available);
    assert_eq!(
        f.manager
            .scan_leftovers("win32:m64:stay")
            .expect_err("no record"),
        LeftoverError::NoRecord
    );
}

#[test]
fn per_user_data_named_exactly_like_the_app_is_medium_and_never_selected() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("Publisher", sz("Contoso Ltd"))],
    );
    f.system
        .file_system
        .add_file(format!(r"{LOCAL}\Contoso Editor\cache.db"), 10);
    f.system.file_system.add_file(
        format!(r"{ROAMING}\Contoso Ltd\Contoso Editor\prefs.json"),
        5,
    );
    f.system
        .file_system
        .add_file(format!(r"{ROAMING}\Contoso Ltd\other\x"), 5);
    f.system
        .file_system
        .add_file(format!(r"{LOCAL}\Contoso Editors\unrelated"), 5);
    f.uninstall(&id);

    let scan = f.scan(&id);
    let same = find(&scan, &format!(r"{LOCAL}\Contoso Editor"));
    assert_eq!(same.confidence, Confidence::Medium);
    assert!(!same.selected_by_default());
    let both = find(&scan, &format!(r"{ROAMING}\Contoso Ltd\Contoso Editor"));
    assert_eq!(both.confidence, Confidence::Medium);
    assert_eq!(both.evidence[0].code, "leftover.why.publisherAndName");
    let vendor = find(&scan, &format!(r"{ROAMING}\Contoso Ltd"));
    assert_eq!(vendor.confidence, Confidence::Low);
    // A near miss is never matched.
    assert!(!offered(&scan, &format!(r"{LOCAL}\Contoso Editors")));
    assert!(scan.items.iter().all(|item| !item.selected_by_default()));
    // The most certain come first.
    assert!(
        scan.items
            .windows(2)
            .all(|pair| pair[0].confidence >= pair[1].confidence)
    );
}

#[test]
fn the_folder_of_the_uninstaller_and_of_the_icon_are_medium() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("DisplayIcon", sz(r#""C:\Tools\Contoso\bin\app.exe",0"#))],
    );
    f.system
        .file_system
        .add_file(r"C:\Tools\Contoso\bin\app.exe", 5);
    f.system
        .file_system
        .add_file(r"C:\Vendors\contoso\remove.exe", 5);
    f.uninstall(&id);
    let scan = f.scan(&id);
    assert_eq!(
        find(&scan, r"C:\Tools\Contoso\bin").confidence,
        Confidence::Medium
    );
    assert_eq!(
        find(&scan, r"C:\Vendors\contoso").evidence[0].code,
        "leftover.why.uninstallerFolder"
    );
    assert!(scan.items.iter().all(|item| !item.selected_by_default()));
}

#[test]
fn a_folder_shared_with_another_app_of_the_publisher_is_never_high() {
    let f = fixture();
    f.install(
        "second",
        "Contoso Viewer",
        &[
            ("Publisher", sz("Contoso")),
            ("InstallLocation", sz(r"C:\Program Files\Contoso\Viewer")),
        ],
    );
    f.system
        .file_system
        .add_file(r"C:\Program Files\Contoso\Viewer\v.exe", 5);
    let id = f.install(
        "first",
        "Contoso Editor",
        &[
            ("Publisher", sz("Contoso")),
            ("InstallLocation", sz(r"C:\Program Files\Contoso")),
        ],
    );
    f.system
        .file_system
        .add_file(r"C:\Program Files\Contoso\e.exe", 5);
    f.uninstall(&id);
    // The vendor folder holds the other app's install folder: not offered at all.
    let scan = f.scan(&id);
    assert!(!offered(&scan, r"C:\Program Files\Contoso"));
    assert!(!offered(&scan, r"C:\Program Files\Contoso\Viewer"));

    // When the other app lives elsewhere, a vendor-named folder is shared: low, with the reason.
    let g = fixture();
    g.install(
        "second",
        "Contoso Viewer",
        &[
            ("Publisher", sz("Contoso")),
            ("InstallLocation", sz(r"D:\Viewer")),
        ],
    );
    let id = g.install(
        "first",
        "Contoso Editor",
        &[
            ("Publisher", sz("Contoso")),
            ("InstallLocation", sz(r"C:\Program Files\Contoso")),
        ],
    );
    g.system
        .file_system
        .add_file(r"C:\Program Files\Contoso\e.exe", 5);
    g.system.file_system.add_file(r"D:\Viewer\v.exe", 5);
    g.uninstall(&id);
    let scan = g.scan(&id);
    let shared = find(&scan, r"C:\Program Files\Contoso");
    assert_eq!(shared.confidence, Confidence::Low);
    assert!(!shared.selected_by_default());
    assert!(
        shared
            .evidence
            .iter()
            .any(|phrase| phrase.code == "leftover.why.shared")
    );
}

#[test]
fn system_places_the_roots_links_and_other_apps_are_never_offered() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("InstallLocation", sz(r"C:\Program Files"))],
    );
    f.uninstall(&id);
    assert!(f.scan(&id).items.is_empty());

    for place in [
        r"C:\Windows\System32\Contoso",
        r"C:\Program Files (x86)",
        r"C:\ProgramData",
        r"C:\Users\me",
        r"C:\Users\me\AppData\Local",
        r"C:\Users\me\AppData",
        r"C:\Users",
        r"..\evil",
        r"\\server\share\app",
        r"C:\Program Files\..\Windows",
        r"C:\PROGRA~1",
        r"C:\Program Files.",
        "relative",
    ] {
        let f = fixture();
        f.system.file_system.add_dir(r"C:\PROGRA~1", None);
        f.system.file_system.add_dir(r"C:\Program Files.", None);
        f.system
            .file_system
            .add_dir(r"C:\Windows\System32\Contoso", None);
        let id = f.install(
            "contoso",
            "Contoso Editor",
            &[("InstallLocation", sz(place))],
        );
        f.uninstall(&id);
        assert!(f.scan(&id).items.is_empty(), "{place} must not be offered");
    }

    // A link, or a folder behind one.
    let f = fixture();
    f.system.file_system.add_link(r"C:\Linked");
    f.system.file_system.add_file(r"C:\Linked\inside\f", 5);
    f.system.file_system.add_link(r"C:\Real\Junction");
    for (n, place) in [r"C:inked", r"c:inkedinside", r"c:realjunction"]
        .into_iter()
        .enumerate()
    {
        let id = f.install(
            &format!("a{n}"),
            "Contoso Editor",
            &[("InstallLocation", sz(place))],
        );
        f.uninstall(&id);
        assert!(f.scan(&id).items.is_empty(), "{place} is behind a link");
    }

    // A folder another installed app's entry points at, or that holds it.
    let f = fixture();
    f.install(
        "other",
        "Fabrikam Tool",
        &[("InstallLocation", sz(r"C:\Suites\Fab\Tool"))],
    );
    f.system.file_system.add_file(r"C:\Suites\Fab\Tool\t", 5);
    f.system.file_system.add_file(r"C:\Suites\Fab\own", 5);
    for place in [
        r"C:\Suites\Fab",
        r"C:\Suites\Fab\Tool",
        r"C:\Suites\Fab\Tool\x",
    ] {
        let id = f.install(
            &format!("b{}", place.len()),
            "Contoso Editor",
            &[("InstallLocation", sz(place))],
        );
        f.system.file_system.add_dir(place, None);
        f.uninstall(&id);
        assert!(
            f.scan(&id).items.is_empty(),
            "{place} belongs to another app"
        );
    }
}

#[test]
fn a_recorded_path_starting_with_an_environment_variable_is_expanded() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("InstallLocation", sz(r"%ProgramFiles%\Contoso Editor"))],
    );
    f.system
        .file_system
        .add_file(r"C:\Program Files\Contoso Editor\a", 7);
    f.uninstall(&id);
    let scan = f.scan(&id);
    assert_eq!(
        find(&scan, r"C:\Program Files\Contoso Editor").confidence,
        Confidence::High
    );
}

fn registry_fixture() -> (Fixture, String) {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[
            ("Publisher", sz("Contoso Ltd")),
            (
                "SettingsKey",
                sz(r"HKCU\Software\Contoso Ltd\Editor Settings"),
            ),
            (
                "MachineKey",
                sz(r"HKEY_LOCAL_MACHINE\Software\Contoso Ltd\Editor Machine"),
            ),
        ],
    );
    let user =
        |path: &str| RegistryKeyRef::new(RegistryHive::CurrentUser, path, RegistryView::Native);
    let settings = user(r"Software\Contoso Ltd\Editor Settings");
    f.system.registry.set(&settings.value("Theme"), sz("dark"));
    f.system.registry.set(
        &user(r"Software\Contoso Ltd\Editor Settings\Recent").value("Count"),
        RegistryValue::Dword(3),
    );
    f.system
        .registry
        .set(&user(r"Software\Contoso Editor").value("Lang"), sz("en"));
    f.system.registry.set(
        &user(r"Software\Microsoft\Contoso Editor").value("x"),
        sz("never"),
    );
    f.system.registry.add_key(
        RegistryHive::LocalMachine,
        r"Software\Contoso Ltd\Editor Machine",
        RegistryView::Native,
    );
    f.system.registry.add_key(
        RegistryHive::LocalMachine,
        r"Software\Contoso Editor",
        RegistryView::Bit64,
    );
    (f, id)
}

#[test]
fn a_key_the_entry_named_is_high_machine_wide_keys_are_locked_and_microsoft_keys_never_offered() {
    let (f, id) = registry_fixture();
    f.uninstall(&id);
    let scan = f.scan(&id);

    let settings = find(&scan, r"HKCU\Software\Contoso Ltd\Editor Settings");
    assert_eq!(settings.confidence, Confidence::High);
    assert!(settings.selected_by_default());
    assert_eq!(settings.items, 2, "the key and its subkey");
    assert_eq!(settings.evidence[0].code, "leftover.why.pointedKey");

    let named = find(&scan, r"HKCU\Software\Contoso Editor");
    assert_eq!(named.confidence, Confidence::Medium);

    let machine = find(&scan, r"HKLM\Software\Contoso Ltd\Editor Machine");
    assert_eq!(machine.locked, Some(LeftoverLock::NeedsAdministrator));
    assert!(!machine.selected_by_default());
    assert_eq!(machine.confidence, Confidence::High);
    assert_eq!(
        find(&scan, r"HKLM\Software\Contoso Editor [64-bit]").locked,
        Some(LeftoverLock::NeedsAdministrator)
    );

    assert!(
        !scan
            .items
            .iter()
            .any(|item| item.location.to_lowercase().contains("microsoft"))
    );
}

#[test]
fn a_whole_vendor_key_named_by_an_entry_or_a_neighbour_key_is_not_offered() {
    let f = fixture();
    f.install(
        "other",
        "Fabrikam Tool",
        &[("Key", sz(r"HKCU\Software\Shared\Tool"))],
    );
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[
            ("A", sz(r"HKCU\Software\Vendor")),
            ("B", sz(r"HKCU\Software\Shared")),
            ("C", sz(r"HKCU\Software\Shared\Tool\Deep")),
            ("D", sz(r"HKCU\Software\Microsoft\Office")),
            ("E", sz(r"HKCU\Other\Place")),
        ],
    );
    for path in [
        r"Software\Vendor",
        r"Software\Shared",
        r"Software\Shared\Tool",
        r"Software\Shared\Tool\Deep",
        r"Software\Microsoft\Office",
        r"Other\Place",
    ] {
        f.system
            .registry
            .add_key(RegistryHive::CurrentUser, path, RegistryView::Native);
    }
    f.uninstall(&id);
    assert!(f.scan(&id).items.is_empty(), "{:?}", f.scan(&id).items);
}

#[test]
fn a_big_key_tree_is_shown_but_locked() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("K", sz(r"HKCU\Software\Contoso\Big"))],
    );
    for n in 0..=MAX_KEYS {
        f.system.registry.add_key(
            RegistryHive::CurrentUser,
            &format!(r"Software\Contoso\Big\k{n}"),
            RegistryView::Native,
        );
    }
    f.uninstall(&id);
    let scan = f.scan(&id);
    let big = find(&scan, r"HKCU\Software\Contoso\Big");
    assert_eq!(big.locked, Some(LeftoverLock::CannotRecord));
    assert!(!big.selected_by_default());
}

#[test]
fn removing_moves_folders_to_the_bin_and_journals_registry_keys_so_they_can_be_undone() {
    let g = fixture();
    let id = g.install(
        "contoso",
        "Contoso Editor",
        &[
            ("InstallLocation", sz(r"C:\Program Files\Contoso Editor")),
            (
                "SettingsKey",
                sz(r"HKCU\Software\Contoso Ltd\Editor Settings"),
            ),
        ],
    );
    g.system
        .file_system
        .add_file(r"C:\Program Files\Contoso Editor\a.bin", 2048);
    let settings = RegistryKeyRef::new(
        RegistryHive::CurrentUser,
        r"Software\Contoso Ltd\Editor Settings",
        RegistryView::Native,
    );
    g.system.registry.set(&settings.value("Theme"), sz("dark"));
    let recent = RegistryKeyRef::new(
        RegistryHive::CurrentUser,
        r"Software\Contoso Ltd\Editor Settings\Recent",
        RegistryView::Native,
    );
    g.system
        .registry
        .set(&recent.value("Count"), RegistryValue::Dword(3));
    g.uninstall(&id);

    let scan = g.scan(&id);
    let chosen: Vec<String> = scan
        .items
        .iter()
        .filter(|item| item.selected_by_default())
        .map(|item| item.id.clone())
        .collect();
    assert_eq!(chosen.len(), 2, "{:?}", scan.items);

    let removal = g
        .manager
        .remove_leftovers(&id, &chosen, Source::Manual)
        .expect("removal");
    assert_eq!(removal.removed.len(), 2, "{:?}", removal.skipped);
    assert!(removal.skipped.is_empty());
    assert!(!removal.activity_write_failed);
    assert_eq!(
        g.system.file_system.trashed(),
        [PathBuf::from(r"C:\Program Files\Contoso Editor")]
    );
    assert!(
        !g.system
            .registry
            .key_exists(settings.hive, &settings.subkey, settings.view)
            .expect("read")
    );
    assert_eq!(removal.batches.len(), 1);

    // The activity record lists what was removed.
    let records = g.activity.records();
    let record = records
        .iter()
        .find(|record| record.label.as_deref() == Some(LEFTOVERS_LABEL))
        .expect("a record");
    assert_eq!(record.outcome, ActivityOutcome::Applied);
    assert!(
        record.summary.contains(r"C:\Program Files\Contoso Editor"),
        "{}",
        record.summary
    );
    assert!(
        record.summary.contains("Editor Settings"),
        "{}",
        record.summary
    );

    // Undo puts the key and its subkey back, with their values.
    let report = g
        .system
        .executor()
        .revert_batch(removal.batches[0], Source::Manual, None)
        .expect("revert");
    assert!(report.is_complete());
    assert_eq!(
        g.system.registry.get(&settings.value("Theme")),
        Some(sz("dark"))
    );
    assert_eq!(
        g.system.registry.get(&recent.value("Count")),
        Some(RegistryValue::Dword(3))
    );
}

#[test]
fn locked_and_unknown_ids_are_skipped_and_nothing_below_high_is_removed_unless_chosen() {
    let (f, id) = registry_fixture();
    f.uninstall(&id);
    let scan = f.scan(&id);
    let machine = find(&scan, r"HKLM\Software\Contoso Ltd\Editor Machine")
        .id
        .clone();
    let medium = find(&scan, r"HKCU\Software\Contoso Editor").id.clone();
    let removal = f
        .manager
        .remove_leftovers(
            &id,
            &[
                machine.clone(),
                "folder:C:\\Windows".to_owned(),
                medium.clone(),
            ],
            Source::Manual,
        )
        .expect("removal");
    assert_eq!(
        removal.skipped,
        [
            (machine, LeftoverSkip::Locked),
            ("folder:C:\\Windows".to_owned(), LeftoverSkip::NotOffered),
        ]
    );
    // Only what was chosen went: the high key is still there.
    assert_eq!(removal.removed.len(), 1);
    assert_eq!(removal.removed[0].id, medium);
    assert!(
        f.system
            .registry
            .key_exists(
                RegistryHive::CurrentUser,
                r"Software\Contoso Ltd\Editor Settings",
                RegistryView::Native
            )
            .expect("read")
    );
    assert!(f.system.file_system.trashed().is_empty());
}

#[test]
fn a_folder_that_changed_into_a_link_since_the_scan_is_left_alone() {
    let f = fixture();
    let id = f.install(
        "contoso",
        "Contoso Editor",
        &[("InstallLocation", sz(r"C:\Apps\Contoso"))],
    );
    f.system.file_system.add_file(r"C:\Apps\Contoso\a", 5);
    f.uninstall(&id);
    let scan = f.scan(&id);
    let chosen = vec![scan.items[0].id.clone()];
    f.system.file_system.add_link(r"C:\Apps\Contoso");
    let removal = f
        .manager
        .remove_leftovers(&id, &chosen, Source::Manual)
        .expect("removal");
    assert!(removal.removed.is_empty());
    assert_eq!(removal.skipped.len(), 1);
    assert!(f.system.file_system.trashed().is_empty());
    let records = f.activity.records();
    assert_eq!(records[0].outcome, ActivityOutcome::Failed);
}

#[test]
fn when_windows_cannot_name_its_system_folders_no_folder_is_offered() {
    let system = FakeSystem::new();
    let folders = Arc::new(FakeKnownFolders::new());
    let cleaner = LeftoverCleaner::new(
        system.file_system.clone(),
        folders,
        system.executor(),
        Arc::new(ManualClock::new(1)),
    );
    let uninstaller = Arc::new(FakeUninstaller::new());
    let manager = AppManager::new(
        system.registry.clone(),
        uninstaller.clone(),
        Arc::new(ManualClock::new(1)),
    )
    .with_leftovers(cleaner);
    let at = entry_key(RegistryHive::LocalMachine, RegistryView::Bit64, "c");
    add_entry(
        &system.registry,
        &at,
        &[
            ("DisplayName", sz("Contoso Editor")),
            ("UninstallString", sz(r"C:\V\u.exe")),
            ("InstallLocation", sz(r"C:\Apps\Contoso")),
        ],
    );
    uninstaller.removes_key_when_run(r"C:\V\u.exe", system.registry.clone(), at);
    system.file_system.add_file(r"C:\Apps\Contoso\a", 1);
    manager
        .uninstall("win32:m64:c", Source::Manual)
        .expect("run");
    let scan = manager.scan_leftovers("win32:m64:c").expect("scan");
    assert!(scan.items.is_empty());
    assert!(scan.incomplete);
}
