//! The startup apps manager end to end, through the wiring function and the functions the Tauri
//! commands call: [`build_core_with`], [`list_startup_entries`], [`set_startup_entry_enabled`]
//! and the Undo page functions.
//!
//! Real adapters only touch a throwaway place: every Run key and approval key the manager uses
//! is redirected to `HKCU\Software\VoltTest\Startup-...` ([`StartupSources::sandboxed_in_hkcu`]),
//! the Startup folders are two folders in a temp directory and the store is a database in a temp
//! directory. The real Run and `StartupApproved` keys and the real Startup folders are never
//! read or written here. The publisher reader is the real one, asked about a program that ships
//! with Windows: it only reads.

use std::path::Path;
use std::process::Command as Process;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use volt_core::fakes::FakeSystem;
use volt_core::{
    Registry, RegistryHive, RegistryKeyRef, RegistryValue, RegistryValueRef, RegistryView,
    StartupSources,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{
    ActivityStatus, ChangeState, StartupChangeStatus, StartupSource, UndoStatus, list_activity,
    list_startup_entries, preview_undo_batch, set_startup_entry_enabled, undo_batch, undo_entry,
};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::wiring::{
    AppCore, StartupWiring, build_core, build_core_with, windows_ports,
};
use volt_win::{WinFilePublisher, WinRegistry};

const NOTEPAD: &str = r"%SystemRoot%\System32\notepad.exe";

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
        let root = format!(r"Software\VoltTest\Startup-{}-{nanos}", std::process::id());
        let sandbox = Self {
            root,
            dir: TempDir::new().expect("temp dir"),
        };
        std::fs::create_dir_all(sandbox.user_folder()).expect("user folder");
        std::fs::create_dir_all(sandbox.common_folder()).expect("common folder");
        sandbox
    }

    fn user_folder(&self) -> std::path::PathBuf {
        self.dir.path().join("UserStartup")
    }

    fn common_folder(&self) -> std::path::PathBuf {
        self.dir.path().join("CommonStartup")
    }

    fn db(&self) -> std::path::PathBuf {
        self.dir.path().join("volt.db")
    }

    fn sources(&self) -> StartupSources {
        StartupSources::sandboxed_in_hkcu(&self.root, self.user_folder(), self.common_folder())
    }

    /// A core over the real adapters, as the app builds it, with every startup place redirected.
    fn core(&self) -> AppCore {
        build_core_with(
            windows_ports(),
            &self.db(),
            EMBEDDED_CATALOGUE,
            StartupWiring {
                sources: self.sources(),
                publisher: Arc::new(WinFilePublisher::new()),
                shortcuts: None,
                logon_tasks: None,
            },
        )
        .expect("core builds")
    }

    fn value(&self, leaf: &str, name: &str) -> RegistryValueRef {
        RegistryKeyRef::new(
            RegistryHive::CurrentUser,
            format!(r"{}\{leaf}", self.root),
            RegistryView::Bit64,
        )
        .value(name)
    }

    fn set_command(&self, leaf: &str, name: &str, command: &str) {
        WinRegistry::new()
            .write_value(
                &self.value(leaf, name),
                &RegistryValue::Sz(command.to_owned()),
            )
            .expect("arrange a Run value");
    }

    fn read(&self, leaf: &str, name: &str) -> Option<RegistryValue> {
        WinRegistry::new()
            .read_value(&self.value(leaf, name))
            .expect("read")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // The shell has no unsafe code, so the unique test key is removed with the system's own
        // `reg` tool. The path is checked first, so this can only name a key below
        // `HKCU\Software\VoltTest\Startup-`.
        assert!(
            self.root.starts_with(r"Software\VoltTest\Startup-"),
            "refusing to delete {}",
            self.root
        );
        let _ = Process::new("reg")
            .args(["delete", &format!(r"HKCU\{}", self.root), "/f"])
            .output();
    }
}

fn write_file(path: &Path) {
    std::fs::write(path, "placeholder shortcut").expect("write a file");
}

#[test]
fn list_disable_read_back_undo_and_restart_on_a_sandbox() {
    let sandbox = Sandbox::new();
    // Guard: nothing below may point at a real startup key.
    assert!(sandbox.root.starts_with(r"Software\VoltTest\"));
    assert!(!sandbox.root.to_lowercase().contains("currentversion"));
    sandbox.set_command(
        "Run",
        "Sample Notepad",
        &format!("\"{NOTEPAD}\" --background"),
    );
    sandbox.set_command("Run", "No Publisher", r"C:\this\does\not\exist\tool.exe /x");
    sandbox.set_command("RunMachine", "Vendor Updater", NOTEPAD);
    sandbox.set_command("RunMachine32", "Old Helper", r"C:\Legacy\helper.exe");
    write_file(&sandbox.user_folder().join("My Notes.lnk"));
    write_file(&sandbox.user_folder().join("desktop.ini"));
    write_file(&sandbox.common_folder().join("Shared.lnk"));

    let core = sandbox.core();
    check_listing(&core);
    check_disable(&core, &sandbox);

    // The restart: only the database and the registry survive.
    drop(core);
    let core = sandbox.core();
    check_state_after_restart(&core);
    check_undo_after_restart(&core, &sandbox);
}

fn check_listing(core: &AppCore) {
    let list = list_startup_entries(core).expect("lists");
    assert!(list.problems.is_empty(), "{:?}", list.problems);
    let ids: Vec<_> = list.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "user-folder/My Notes.lnk",
            "user-run/No Publisher",
            "machine-run-32/Old Helper",
            "user-run/Sample Notepad",
            "common-folder/Shared.lnk",
            "machine-run/Vendor Updater",
        ]
    );
    let entry = |id: &str| {
        list.entries
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("entry {id}"))
    };
    let notepad = entry("user-run/Sample Notepad");
    assert_eq!(notepad.source, StartupSource::UserRun);
    assert_eq!(notepad.command, format!("\"{NOTEPAD}\" --background"));
    assert!(notepad.enabled && !notepad.needs_admin);
    assert!(
        notepad
            .publisher
            .as_deref()
            .is_some_and(|p| p.contains("Microsoft")),
        "{:?}",
        notepad.publisher
    );
    assert_eq!(entry("user-run/No Publisher").publisher, None);
    assert!(entry("machine-run/Vendor Updater").needs_admin);
    assert!(entry("common-folder/Shared.lnk").needs_admin);
    assert_eq!(entry("user-folder/My Notes.lnk").name, "My Notes");
}

fn check_disable(core: &AppCore, sandbox: &Sandbox) {
    // ---- disable ----
    let changed =
        set_startup_entry_enabled(core, "user-run/Sample Notepad", false).expect("disables");
    assert_eq!(changed.status, StartupChangeStatus::Changed);
    assert_eq!(changed.enabled, Some(false));
    // The state is the Windows record: 12 bytes, flags 3, a time.
    let Some(RegistryValue::Binary(record)) =
        sandbox.read(r"Explorer\StartupApproved\Run", "Sample Notepad")
    else {
        panic!("no approval record was written");
    };
    assert_eq!(record.len(), 12);
    assert_eq!(record[..4], [3, 0, 0, 0]);
    assert_ne!(record[4..], [0; 8]);
    // The Run value is untouched, and so is every other entry.
    assert_eq!(
        sandbox.read("Run", "Sample Notepad"),
        Some(RegistryValue::Sz(format!("\"{NOTEPAD}\" --background")))
    );
    let after = list_startup_entries(core).expect("lists again");
    let state = |id: &str| after.entries.iter().find(|e| e.id == id).map(|e| e.enabled);
    assert_eq!(state("user-run/Sample Notepad"), Some(false));
    assert_eq!(state("user-run/No Publisher"), Some(true));
    // Disabling again is a no-op.
    let again = set_startup_entry_enabled(core, "user-run/Sample Notepad", false).expect("fine");
    assert_eq!(again.status, StartupChangeStatus::AlreadyInState);

    // ---- the Undo page reads well ----
    let page = list_activity(core, 50, None).expect("activity");
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    assert_eq!(item.summary, "Disabled startup entry: Sample Notepad");
    assert_eq!(item.status, ActivityStatus::Applied);
    assert_eq!(item.state, ChangeState::CanUndo);
    assert_eq!(item.title.as_deref(), Some("Startup apps"));
    assert_eq!(item.previous.as_deref(), Some("enabled (it had no record)"));
    // The command line is not in the record (redaction rule).
    assert!(!item.summary.contains("notepad"));
}

fn check_state_after_restart(core: &AppCore) {
    // ---- restart: only the database and the registry survive ----
    assert!(
        core.startup
            .reconcile
            .as_ref()
            .is_ok_and(|report| report.outcomes.is_empty()),
        "{:?}",
        core.startup.reconcile
    );
    let restarted = list_startup_entries(core).expect("lists after the restart");
    assert_eq!(
        restarted
            .entries
            .iter()
            .find(|e| e.id == "user-run/Sample Notepad")
            .map(|e| e.enabled),
        Some(false),
        "the state reads disabled after a restart"
    );
}

fn check_undo_after_restart(core: &AppCore, sandbox: &Sandbox) {
    // ---- undo, after the restart ----
    let page = list_activity(core, 50, None).expect("activity");
    let entry_id = page.items[0].entry.clone().expect("linked to the journal");
    let batch_id = page.items[0].batch.clone().expect("in a batch");
    let preview = preview_undo_batch(core, &batch_id).expect("preview");
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.items[0].what, "Startup entry Sample Notepad");
    assert_eq!(
        preview.items[0].restore,
        "set back to enabled (the record Volt wrote is removed)"
    );
    let undone = undo_entry(core, &entry_id, &[]).expect("undoes");
    assert_eq!(undone.status, UndoStatus::Restored);
    assert_eq!(
        sandbox.read(r"Explorer\StartupApproved\Run", "Sample Notepad"),
        None,
        "a record that did not exist before is gone again"
    );
    let enabled_again = list_startup_entries(core).expect("lists");
    assert_eq!(
        enabled_again
            .entries
            .iter()
            .find(|e| e.id == "user-run/Sample Notepad")
            .map(|e| e.enabled),
        Some(true)
    );
    let page = list_activity(core, 50, None).expect("activity");
    assert_eq!(
        page.items[0].summary,
        "Reverted startup entry: Sample Notepad is enabled again"
    );
    assert_eq!(page.items[0].status, ActivityStatus::Reverted);
    assert_eq!(page.items[1].state, ChangeState::Undone);
    // Nothing is left to undo.
    assert_eq!(
        undo_entry(core, &entry_id, &[]).expect("again").status,
        UndoStatus::NothingToUndo
    );
}

#[test]
fn a_startup_folder_item_is_approved_under_startup_folder_and_undone_by_batch() {
    let sandbox = Sandbox::new();
    write_file(&sandbox.user_folder().join("My Notes.lnk"));
    let core = sandbox.core();

    let changed =
        set_startup_entry_enabled(&core, "user-folder/My Notes.lnk", false).expect("disables");
    assert_eq!(changed.name, "My Notes.lnk");
    assert_eq!(changed.enabled, Some(false));
    assert!(matches!(
        sandbox.read(r"Explorer\StartupApproved\StartupFolder", "My Notes.lnk"),
        Some(RegistryValue::Binary(_))
    ));
    // Enabling writes the enabled record (kept, not deleted), as Task Manager does.
    let enabled =
        set_startup_entry_enabled(&core, "user-folder/My Notes.lnk", true).expect("enables");
    assert_eq!(enabled.enabled, Some(true));
    assert_eq!(
        sandbox.read(r"Explorer\StartupApproved\StartupFolder", "My Notes.lnk"),
        Some(RegistryValue::Binary(vec![
            2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        ]))
    );

    let page = list_activity(&core, 50, None).expect("activity");
    let summaries: Vec<_> = page.items.iter().map(|i| i.summary.as_str()).collect();
    assert_eq!(
        summaries,
        [
            "Enabled startup entry: My Notes.lnk",
            "Disabled startup entry: My Notes.lnk"
        ]
    );
    // The earlier value of the enable was the disabled record: the Undo page says so in words.
    assert_eq!(page.items[0].previous.as_deref(), Some("disabled"));
    // Undo all puts everything back, newest first: disabled, then no record.
    let batches: std::collections::BTreeSet<_> =
        page.items.iter().filter_map(|i| i.batch.clone()).collect();
    for batch in batches.iter().rev() {
        assert_eq!(
            undo_batch(&core, batch, &[]).expect("undoes").status,
            UndoStatus::Restored
        );
    }
    assert_eq!(
        sandbox.read(r"Explorer\StartupApproved\StartupFolder", "My Notes.lnk"),
        None
    );
}

#[test]
fn machine_wide_entries_are_listed_but_refused_without_touching_anything() {
    let sandbox = Sandbox::new();
    sandbox.set_command("RunMachine", "Vendor Updater", r"C:\v.exe");
    sandbox.set_command("RunMachine32", "Old Helper", r"C:\o.exe");
    write_file(&sandbox.common_folder().join("Shared.lnk"));
    let core = sandbox.core();

    for id in [
        "machine-run/Vendor Updater",
        "machine-run-32/Old Helper",
        "common-folder/Shared.lnk",
    ] {
        let error = set_startup_entry_enabled(&core, id, false).expect_err("refused");
        assert!(
            matches!(&error, IpcError::AccessDenied { message, .. } if message.contains("administrator rights")),
            "{id}: {error:?}"
        );
    }
    for (leaf, name) in [
        (r"Explorer\StartupApprovedMachine\Run", "Vendor Updater"),
        (r"Explorer\StartupApprovedMachine\Run32", "Old Helper"),
        (
            r"Explorer\StartupApprovedMachine\StartupFolder",
            "Shared.lnk",
        ),
    ] {
        assert_eq!(sandbox.read(leaf, name), None);
    }
    assert!(
        list_activity(&core, 10, None)
            .expect("activity")
            .items
            .is_empty()
    );
    let list = list_startup_entries(&core).expect("lists");
    assert_eq!(list.entries.len(), 3);
    assert!(list.entries.iter().all(|e| e.needs_admin && e.enabled));
}

#[test]
fn stale_and_malformed_ids_are_typed_errors() {
    let sandbox = Sandbox::new();
    sandbox.set_command("Run", "Real", r"C:\r.exe");
    let core = sandbox.core();

    assert!(matches!(
        set_startup_entry_enabled(&core, "user-run/Gone", false),
        Err(IpcError::NotFound { .. })
    ));
    assert!(matches!(
        set_startup_entry_enabled(&core, "nonsense", false),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        set_startup_entry_enabled(&core, "user-folder/..\\..\\evil.lnk", false),
        Err(IpcError::InvalidInput { .. })
    ));
}

#[test]
fn a_core_built_without_the_startup_wiring_answers_unsupported() {
    let system = FakeSystem::new();
    let dir = TempDir::new().expect("temp dir");
    let core = build_core(
        system.ports(),
        &dir.path().join("volt.db"),
        EMBEDDED_CATALOGUE,
    )
    .expect("core builds");

    assert!(matches!(
        list_startup_entries(&core),
        Err(IpcError::Unsupported { .. })
    ));
    assert!(matches!(
        set_startup_entry_enabled(&core, "user-run/X", false),
        Err(IpcError::Unsupported { .. })
    ));
}
