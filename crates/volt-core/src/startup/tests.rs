//! The startup manager against the in-memory fakes. Nothing here touches the
//! real system.

use std::path::PathBuf;
use std::sync::Arc;

use super::fakes::FakePublisher;
use super::*;
use crate::activity::{redacted_prior_for, restore_words_for};
use crate::clean::Location;
use crate::clean::fakes::FakeKnownFolders;
use crate::error::{ExecError, PortError};
use crate::fakes::{Event, FakeSystem};
use crate::journal::{EntryState, Journal, Source};
use crate::operation::{Operation, PriorState};
use crate::registry::{RegistryHive, RegistryValueRef, RegistryView};
use crate::time::Timestamp;

const ROOT: &str = r"Software\VoltTest\startup";
const USER_FOLDER: &str = "C:/Users/Sample/Startup";
const COMMON_FOLDER: &str = "C:/ProgramData/Startup";

struct Fixture {
    system: FakeSystem,
    publisher: Arc<FakePublisher>,
    manager: StartupManager,
}

fn sandbox() -> StartupSources {
    StartupSources::sandboxed_in_hkcu(
        ROOT,
        PathBuf::from(USER_FOLDER),
        PathBuf::from(COMMON_FOLDER),
    )
}

fn fixture_with(sources: StartupSources) -> Fixture {
    let system = FakeSystem::new();
    let publisher = Arc::new(FakePublisher::new());
    let manager = StartupManager::new(
        system.registry.clone(),
        system.file_system.clone(),
        publisher.clone(),
        system.executor_with_activity(),
        system.clock.clone(),
        sources,
    );
    Fixture {
        system,
        publisher,
        manager,
    }
}

fn fixture() -> Fixture {
    fixture_with(sandbox())
}

fn key(subkey: &str) -> RegistryKeyRef {
    RegistryKeyRef::new(RegistryHive::CurrentUser, subkey, RegistryView::Bit64)
}

fn run_value(subkey: &str, name: &str) -> RegistryValueRef {
    key(subkey).value(name)
}

fn user_run(name: &str) -> RegistryValueRef {
    run_value(&format!(r"{ROOT}\Run"), name)
}

fn user_approval(name: &str) -> RegistryValueRef {
    run_value(&format!(r"{ROOT}\Explorer\StartupApproved\Run"), name)
}

fn add_run(f: &Fixture, name: &str, command: &str) {
    f.system
        .registry
        .set(&user_run(name), RegistryValue::Sz(command.to_owned()));
}

fn find<'a>(listing: &'a StartupListing, id: &str) -> &'a StartupEntry {
    listing
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("no entry {id} in {:?}", listing.entries))
}

fn summaries(f: &Fixture) -> Vec<String> {
    f.system
        .activity
        .records()
        .into_iter()
        .map(|record| record.summary)
        .collect()
}

// ---- listing --------------------------------------------------------------------------------

#[test]
fn run_entries_of_every_registry_source_are_listed_with_command_origin_and_publisher() {
    let f = fixture();
    let registry = &f.system.registry;
    registry.set(
        &user_run("Sample Sync"),
        RegistryValue::Sz(r#""C:\Program Files\Sample\sync.exe" --background"#.into()),
    );
    registry.set(
        &run_value(&format!(r"{ROOT}\RunMachine"), "Vendor Updater"),
        RegistryValue::ExpandSz(r"%ProgramFiles%\Vendor\update.exe /check".into()),
    );
    registry.set(
        &run_value(&format!(r"{ROOT}\RunMachine32"), "Old Helper"),
        RegistryValue::Sz(r"C:\Legacy\helper.exe".into()),
    );
    f.publisher
        .set(r"C:\Program Files\Sample\sync.exe", "Sample Software Ltd");
    f.publisher
        .set(r"%ProgramFiles%\Vendor\update.exe", "Vendor Inc.");

    let listing = f.manager.list();

    assert!(listing.problems.is_empty(), "{:?}", listing.problems);
    assert_eq!(listing.entries.len(), 3);
    let sync = find(&listing, "user-run/Sample Sync");
    assert_eq!(sync.name, "Sample Sync");
    assert_eq!(sync.origin, StartupOrigin::UserRun);
    assert_eq!(
        sync.command,
        r#""C:\Program Files\Sample\sync.exe" --background"#
    );
    assert_eq!(
        sync.program.as_deref(),
        Some(r"C:\Program Files\Sample\sync.exe")
    );
    assert_eq!(sync.publisher.as_deref(), Some("Sample Software Ltd"));
    assert_eq!(sync.state, StartupState::Enabled);
    let updater = find(&listing, "machine-run/Vendor Updater");
    // The environment variable is passed on unexpanded.
    assert_eq!(
        updater.program.as_deref(),
        Some(r"%ProgramFiles%\Vendor\update.exe")
    );
    assert_eq!(updater.publisher.as_deref(), Some("Vendor Inc."));
    let old = find(&listing, "machine-run-32/Old Helper");
    assert_eq!(old.origin, StartupOrigin::MachineRun32);
    assert_eq!(old.publisher, None);
}

#[test]
fn startup_folder_files_are_listed_and_decoration_and_directories_are_not() {
    let f = fixture();
    let fs = &f.system.file_system;
    fs.add_file(format!("{USER_FOLDER}/Notes.lnk"), 1200);
    fs.add_file(format!("{USER_FOLDER}/tool.exe"), 5000);
    fs.add_file(format!("{USER_FOLDER}/desktop.ini"), 100);
    fs.add_dir(format!("{USER_FOLDER}/A Folder"), None);
    fs.add_link(format!("{USER_FOLDER}/Linked"));
    fs.add_file(format!("{COMMON_FOLDER}/Shared.lnk"), 900);
    // The program path is the folder joined with the file name, as the platform writes it.
    let tool_path = PathBuf::from(USER_FOLDER)
        .join("tool.exe")
        .to_string_lossy()
        .into_owned();
    f.publisher.set(&tool_path, "Tool Makers");

    let listing = f.manager.list();

    let names: Vec<_> = listing
        .entries
        .iter()
        .map(|e| (e.id.as_str(), e.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("user-folder/Notes.lnk", "Notes"),
            ("common-folder/Shared.lnk", "Shared"),
            ("user-folder/tool.exe", "tool.exe"),
        ]
    );
    // A shortcut has no version information of its own; a program does.
    let notes = find(&listing, "user-folder/Notes.lnk");
    assert_eq!(notes.publisher, None);
    assert_eq!(notes.program, None);
    assert!(notes.command.ends_with("Notes.lnk"));
    let tool = find(&listing, "user-folder/tool.exe");
    assert_eq!(tool.publisher.as_deref(), Some("Tool Makers"));
    assert_eq!(f.publisher.asked(), [tool_path]);
}

#[test]
fn the_state_comes_from_the_approval_records_whatever_the_case_of_the_name() {
    let f = fixture();
    for name in ["On", "Off", "Plain", "Odd"] {
        add_run(&f, name, r"C:\x\y.exe");
    }
    let registry = &f.system.registry;
    registry.set(
        &user_approval("on"),
        encode_approval(StartupState::Enabled, Timestamp::from_unix_millis(0)),
    );
    registry.set(
        &user_approval("OFF"),
        encode_approval(StartupState::Disabled, Timestamp::from_unix_millis(5)),
    );
    // A record this layout does not describe reads as enabled.
    registry.set(&user_approval("Odd"), RegistryValue::Binary(vec![1; 12]));
    f.system
        .file_system
        .add_file(format!("{USER_FOLDER}/Folder Item.lnk"), 1);
    registry.set(
        &run_value(
            &format!(r"{ROOT}\Explorer\StartupApproved\StartupFolder"),
            "Folder Item.lnk",
        ),
        encode_approval(StartupState::Disabled, Timestamp::from_unix_millis(5)),
    );

    let listing = f.manager.list();

    let state = |id: &str| find(&listing, id).state;
    assert_eq!(state("user-run/On"), StartupState::Enabled);
    assert_eq!(state("user-run/Off"), StartupState::Disabled);
    assert_eq!(state("user-run/Plain"), StartupState::Enabled);
    assert_eq!(state("user-run/Odd"), StartupState::Enabled);
    assert_eq!(state("user-folder/Folder Item.lnk"), StartupState::Disabled);
}

#[test]
fn values_that_are_not_strings_are_not_entries_and_the_list_is_sorted_by_name() {
    let f = fixture();
    add_run(&f, "zeta", r"C:\z.exe");
    add_run(&f, "Alpha", r"C:\a.exe");
    f.system
        .registry
        .set(&user_run("Not a command"), RegistryValue::Dword(1));
    f.system
        .registry
        .set(&user_run("Binary"), RegistryValue::Binary(vec![1, 2]));

    let names: Vec<_> = f
        .manager
        .list()
        .entries
        .into_iter()
        .map(|e| e.name)
        .collect();

    assert_eq!(names, ["Alpha", "zeta"]);
}

#[test]
fn a_missing_startup_folder_is_empty_and_an_unreadable_source_is_a_problem_not_a_failure() {
    let f = fixture();
    add_run(&f, "Kept", r"C:\k.exe");
    // The user's folder is missing entirely (never created); the common folder
    // cannot be read.
    f.system.file_system.add_dir(COMMON_FOLDER, None);
    f.system.file_system.fail_read_dir_of(
        COMMON_FOLDER,
        PortError::AccessDenied("secret detail".into()),
    );

    let listing = f.manager.list();

    assert_eq!(listing.entries.len(), 1);
    assert_eq!(
        listing.problems,
        [SourceProblem {
            origin: StartupOrigin::CommonFolder,
            reason: "access denied"
        }]
    );
}

#[test]
fn a_publisher_that_cannot_be_read_or_is_blank_is_none_never_an_error() {
    let f = fixture();
    add_run(&f, "Broken", r"C:\broken.exe");
    add_run(&f, "Blank", r"C:\blank.exe");
    add_run(&f, "NoProgram", "");
    f.publisher.fail_on(
        r"C:\broken.exe",
        PortError::Other("version.dll said no".into()),
    );
    f.publisher.set(r"C:\blank.exe", "   ");

    let listing = f.manager.list();

    assert_eq!(listing.entries.len(), 3);
    assert!(listing.problems.is_empty());
    assert!(listing.entries.iter().all(|e| e.publisher.is_none()));
    // An empty command has no program to ask about.
    assert_eq!(find(&listing, "user-run/NoProgram").program, None);
}

#[test]
fn a_registry_without_enumeration_makes_the_run_sources_problems() {
    struct NoEnumeration;
    impl Registry for NoEnumeration {
        fn read_value(&self, _: &RegistryValueRef) -> Result<Option<RegistryValue>, PortError> {
            Ok(None)
        }
        fn write_value(&self, _: &RegistryValueRef, _: &RegistryValue) -> Result<(), PortError> {
            Ok(())
        }
        fn delete_value(&self, _: &RegistryValueRef) -> Result<(), PortError> {
            Ok(())
        }
        fn key_exists(
            &self,
            _: crate::RegistryHive,
            _: &str,
            _: crate::RegistryView,
        ) -> Result<bool, PortError> {
            Ok(false)
        }
    }
    let system = FakeSystem::new();
    let manager = StartupManager::new(
        Arc::new(NoEnumeration),
        system.file_system.clone(),
        Arc::new(NoPublisher),
        system.executor(),
        system.clock.clone(),
        sandbox(),
    );

    let listing = manager.list_entries();

    assert!(listing.entries.is_empty());
    assert_eq!(listing.problems.len(), 3);
    assert!(listing.problems.iter().all(|p| p.reason == "unsupported"));
    // The full listing also reports the three RunOnce sources.
    assert_eq!(manager.list().problems.len(), 6);
}

#[test]
fn the_real_sources_are_read_and_approved_where_task_manager_does() {
    let folders = FakeKnownFolders::new();
    folders.set(Location::RoamingAppData, "C:/Users/Sample/AppData/Roaming");
    folders.set(Location::ProgramData, "C:/ProgramData");
    let f = fixture_with(StartupSources::windows(&folders));
    let run = RegistryKeyRef::new(
        RegistryHive::CurrentUser,
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        RegistryView::Bit64,
    );
    f.system
        .registry
        .set(&run.value("Sample"), RegistryValue::Sz(r"C:\s.exe".into()));
    let startup = "C:/Users/Sample/AppData/Roaming/Microsoft/Windows/Start Menu/Programs/Startup";
    f.system
        .file_system
        .add_file(format!("{startup}/Shortcut.lnk"), 3);

    let listing = f.manager.list();
    assert!(find(&listing, "user-run/Sample").state == StartupState::Enabled);
    f.manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("disabled");
    f.manager
        .set_enabled("user-folder/Shortcut.lnk", false, Source::Manual)
        .expect("disabled");

    let approved = |kind: &str, name: &str| {
        f.system.registry.get(
            &RegistryValueRef::new(
                RegistryHive::CurrentUser,
                format!(
                    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{kind}"
                ),
                name,
            )
            .with_view(RegistryView::Bit64),
        )
    };
    assert_eq!(
        approved("Run", "Sample").as_ref().and_then(decode_approval),
        Some(StartupState::Disabled)
    );
    assert_eq!(
        approved("StartupFolder", "Shortcut.lnk")
            .as_ref()
            .and_then(decode_approval),
        Some(StartupState::Disabled)
    );
}

// ---- switching ------------------------------------------------------------------------------

#[test]
fn disabling_writes_the_windows_record_through_the_journal_and_reads_back_disabled() {
    let f = fixture();
    add_run(&f, "Sample Sync", r"C:\s.exe");

    let change = f
        .manager
        .set_enabled("user-run/Sample Sync", false, Source::Manual)
        .expect("disables");

    assert_eq!(change.state, Some(StartupState::Disabled));
    assert_eq!(change.name, "Sample Sync");
    let ChangeOutcome::Changed { batch, entry } = change.outcome else {
        panic!("expected a change, got {:?}", change.outcome);
    };
    // The record is the 12-byte Windows layout carrying the clock's time.
    let Some(RegistryValue::Binary(bytes)) = f.system.registry.get(&user_approval("Sample Sync"))
    else {
        panic!("no approval record");
    };
    assert_eq!(bytes.len(), 12);
    assert_eq!(bytes[..4], [3, 0, 0, 0]);
    assert_ne!(bytes[4..], [0; 8]);
    // The Run value itself is untouched.
    assert_eq!(
        f.system.registry.get(&user_run("Sample Sync")),
        Some(RegistryValue::Sz(r"C:\s.exe".into()))
    );
    // The journal holds the previous value, "there was none" (and that the approval key did not
    // exist either, so undoing removes the empty keys), under the label.
    let recorded = f.system.journal.entry(entry).expect("read").expect("entry");
    assert_eq!(recorded.state, EntryState::Applied);
    assert_eq!(
        recorded.prior,
        PriorState::RegistryValueInNewKey { missing_levels: 3 }
    );
    assert_eq!(
        f.system
            .journal
            .batch_label(batch)
            .expect("label")
            .as_deref(),
        Some(STARTUP_LABEL)
    );
    // The listing agrees.
    assert_eq!(
        find(&f.manager.list(), "user-run/Sample Sync").state,
        StartupState::Disabled
    );
}

#[test]
fn the_journal_entry_is_written_before_the_record_is_changed() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");

    f.manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("disables");

    let events = f.system.log.events();
    let pending = events
        .iter()
        .position(|e| matches!(e, Event::JournalPending(_)))
        .expect("pending entry");
    let write = events
        .iter()
        .position(|e| matches!(e, Event::RegistryWrite(_)))
        .expect("registry write");
    assert!(pending < write, "{events:?}");
}

#[test]
fn enabling_writes_the_enabled_record_and_undo_puts_the_disabled_one_back() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");
    f.manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("disables");
    let disabled = f.system.registry.get(&user_approval("Sample"));

    let change = f
        .manager
        .set_enabled("user-run/Sample", true, Source::Manual)
        .expect("enables");

    assert_eq!(change.state, Some(StartupState::Enabled));
    // Like Task Manager: the value stays, with the enabled flags and zeros.
    assert_eq!(
        f.system.registry.get(&user_approval("Sample")),
        Some(RegistryValue::Binary(vec![
            2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
        ]))
    );
    let ChangeOutcome::Changed { entry, .. } = change.outcome else {
        panic!("expected a change");
    };
    f.manager
        .executor
        .revert_entry(entry, Source::Manual, None)
        .expect("undoes the enable");
    assert_eq!(f.system.registry.get(&user_approval("Sample")), disabled);
}

#[test]
fn undoing_the_first_disable_removes_the_record_it_created() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");
    let change = f
        .manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("disables");
    let ChangeOutcome::Changed { entry, .. } = change.outcome else {
        panic!("expected a change");
    };

    f.manager
        .executor
        .revert_entry(entry, Source::Manual, None)
        .expect("undoes");

    assert_eq!(f.system.registry.get(&user_approval("Sample")), None);
    assert_eq!(
        find(&f.manager.list(), "user-run/Sample").state,
        StartupState::Enabled
    );
}

#[test]
fn a_change_to_the_state_it_already_has_is_left_alone() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");

    let change = f
        .manager
        .set_enabled("user-run/Sample", true, Source::Manual)
        .expect("fine");

    assert_eq!(change.outcome, ChangeOutcome::AlreadyInState);
    assert_eq!(change.state, Some(StartupState::Enabled));
    assert!(f.system.journal.entries().is_empty());
    assert!(f.system.activity.records().is_empty());
    assert!(f.system.log.events().is_empty());

    f.manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("disables");
    let again = f
        .manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect("fine");
    assert_eq!(again.outcome, ChangeOutcome::AlreadyInState);
    assert_eq!(f.system.journal.entries().len(), 1);
}

#[test]
fn the_activity_records_say_in_plain_words_what_happened() {
    let f = fixture();
    add_run(
        &f,
        "Sample Sync",
        r"C:\very\secret\path\s.exe --token=hunter2",
    );
    f.system
        .file_system
        .add_file(format!("{USER_FOLDER}/Notes.lnk"), 1);

    let disable = f
        .manager
        .set_enabled("user-run/Sample Sync", false, Source::Manual)
        .expect("disables");
    f.manager
        .set_enabled("user-folder/Notes.lnk", false, Source::Manual)
        .expect("disables the shortcut");
    let ChangeOutcome::Changed { entry, .. } = disable.outcome else {
        panic!("expected a change");
    };
    f.manager
        .executor
        .revert_entry(entry, Source::Manual, None)
        .expect("undoes");
    f.manager
        .set_enabled("user-folder/Notes.lnk", true, Source::Manual)
        .expect("enables the shortcut");

    assert_eq!(
        summaries(&f),
        [
            "Disabled startup entry: Sample Sync",
            "Disabled startup entry: Notes.lnk",
            "Reverted startup entry: Sample Sync is enabled again",
            "Enabled startup entry: Notes.lnk",
        ]
    );
    // The command line is not part of any record (redaction rule).
    assert!(
        summaries(&f)
            .iter()
            .all(|s| !s.contains("secret") && !s.contains("hunter2"))
    );
    // Every record links to its journal entry and carries the label.
    assert!(
        f.system
            .activity
            .records()
            .iter()
            .all(|r| r.entry.is_some() && r.label.as_deref() == Some(STARTUP_LABEL))
    );
}

#[test]
fn undoing_a_disable_that_replaced_a_disabled_record_says_it_is_disabled_again() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");
    f.system.registry.set(
        &user_approval("Sample"),
        encode_approval(StartupState::Disabled, Timestamp::from_unix_millis(5)),
    );
    // Enable (previous: disabled), then undo it.
    let change = f
        .manager
        .set_enabled("user-run/Sample", true, Source::Manual)
        .expect("enables");
    let ChangeOutcome::Changed { entry, .. } = change.outcome else {
        panic!("expected a change");
    };
    f.manager
        .executor
        .revert_entry(entry, Source::Manual, None)
        .expect("undoes");

    assert_eq!(
        summaries(&f),
        [
            "Enabled startup entry: Sample",
            "Reverted startup entry: Sample is disabled again",
        ]
    );
}

#[test]
fn the_undo_page_wording_for_a_startup_record_is_enabled_or_disabled() {
    let operation = Operation::SetRegistryValue {
        target: user_approval("Sample"),
        value: encode_approval(StartupState::Disabled, Timestamp::from_unix_millis(5)),
    };
    let none = PriorState::RegistryValue(None);
    let enabled = PriorState::RegistryValue(Some(encode_approval(
        StartupState::Enabled,
        Timestamp::from_unix_millis(0),
    )));
    let disabled = PriorState::RegistryValue(Some(encode_approval(
        StartupState::Disabled,
        Timestamp::from_unix_millis(5),
    )));
    let odd = PriorState::RegistryValue(Some(RegistryValue::Binary(vec![1; 12])));

    assert_eq!(
        redacted_prior_for(&operation, &none),
        "enabled (it had no record)"
    );
    assert_eq!(redacted_prior_for(&operation, &enabled), "enabled");
    assert_eq!(redacted_prior_for(&operation, &disabled), "disabled");
    assert_eq!(
        redacted_prior_for(&operation, &odd),
        "REG_BINARY (12 bytes)"
    );
    assert_eq!(
        restore_words_for(&operation, &disabled),
        "set back to disabled"
    );
    assert_eq!(
        restore_words_for(&operation, &none),
        "set back to enabled (the record Volt wrote is removed)"
    );
    // Any other registry change keeps its old wording.
    let other = Operation::SetRegistryValue {
        target: RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\X", "Flag"),
        value: RegistryValue::Dword(1),
    };
    assert_eq!(redacted_prior_for(&other, &none), "(not set)");
    assert_eq!(
        restore_words_for(&other, &none),
        "removed (it did not exist before)"
    );
}

#[test]
fn machine_wide_entries_need_administrator_rights_and_nothing_is_touched() {
    let f = fixture();
    f.system.registry.set(
        &run_value(&format!(r"{ROOT}\RunMachine"), "Vendor Updater"),
        RegistryValue::Sz(r"C:\v.exe".into()),
    );
    f.system.registry.set(
        &run_value(&format!(r"{ROOT}\RunMachine32"), "Old"),
        RegistryValue::Sz(r"C:\o.exe".into()),
    );
    f.system
        .file_system
        .add_file(format!("{COMMON_FOLDER}/Shared.lnk"), 1);

    for id in [
        "machine-run/Vendor Updater",
        "machine-run-32/Old",
        "common-folder/Shared.lnk",
    ] {
        for enabled in [false, true] {
            let error = f
                .manager
                .set_enabled(id, enabled, Source::Manual)
                .expect_err("refused");
            assert!(
                matches!(&error, StartupError::NeedsAdministrator { .. }),
                "{id}: {error:?}"
            );
            assert_eq!(f.manager.plan(id, enabled), Err(error));
        }
    }

    assert!(f.system.log.events().is_empty());
    assert!(f.system.journal.entries().is_empty());
    assert!(f.system.activity.records().is_empty());
    // They are still listed, with their origin.
    let listing = f.manager.list();
    assert_eq!(listing.entries.len(), 3);
    assert!(
        listing
            .entries
            .iter()
            .all(|e| e.origin.scope() == StartupScope::Machine)
    );
}

#[test]
fn unknown_stale_and_malformed_ids_are_refused_before_anything_changes() {
    let f = fixture();
    add_run(&f, "Real", r"C:\r.exe");
    f.system
        .registry
        .set(&user_run("Number"), RegistryValue::Dword(1));
    f.system
        .file_system
        .add_dir(format!("{USER_FOLDER}/Sub"), None);

    for id in ["", "nope", "bogus-origin/Real", "user-run/"] {
        assert!(
            matches!(
                f.manager.set_enabled(id, false, Source::Manual),
                Err(StartupError::InvalidId(_))
            ),
            "{id:?}"
        );
    }
    for id in [
        "user-run/Gone",
        "user-run/Number",
        "user-folder/Missing.lnk",
        "user-folder/Sub",
    ] {
        assert!(
            matches!(
                f.manager.set_enabled(id, false, Source::Manual),
                Err(StartupError::NotFound(_))
            ),
            "{id:?}"
        );
    }
    // A folder name is one path component: nothing can reach outside the folder.
    for id in [
        "user-folder/../secret.lnk",
        "user-folder/..",
        "user-folder/a/b.lnk",
        "user-folder/a\\b.lnk",
        "user-folder/C:/x.lnk",
    ] {
        assert!(
            matches!(
                f.manager.set_enabled(id, false, Source::Manual),
                Err(StartupError::InvalidId(_))
            ),
            "{id:?}"
        );
    }
    assert!(f.system.log.events().is_empty());
    assert!(f.system.journal.entries().is_empty());
}

#[test]
fn the_dry_run_reads_only_and_shows_the_record_it_would_write() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");

    let plan = f
        .manager
        .plan("user-run/Sample", false)
        .expect("plans")
        .expect("something to do");

    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.label.as_deref(), Some(STARTUP_LABEL));
    assert_eq!(plan.risk(), Risk::Safe);
    assert_eq!(plan.items[0].preview.current, "(not set)");
    assert!(
        plan.items[0]
            .preview
            .target
            .starts_with("REG_BINARY 03000000")
    );
    assert!(f.system.log.events().is_empty());
    assert!(f.system.journal.entries().is_empty());
    assert_eq!(f.manager.plan("user-run/Sample", true), Ok(None));
}

#[test]
fn a_failed_write_is_reported_as_failed_and_leaves_the_journal_entry_failed() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");
    f.system.registry.fail_on(
        &user_approval("Sample"),
        PortError::AccessDenied("no".into()),
    );

    let error = f
        .manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect_err("fails");

    assert!(matches!(
        &error,
        StartupError::Change(ExecError::Apply {
            error: PortError::AccessDenied(_),
            ..
        })
    ));
    assert_eq!(f.system.registry.get(&user_approval("Sample")), None);
    let entries = f.system.journal.entries();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].state, EntryState::Failed);
    let summary = summaries(&f);
    assert_eq!(summary.len(), 1);
    assert_eq!(
        summary[0],
        "Failed to apply startup entry Sample: the change failed (access denied)"
    );
}

#[test]
fn a_journal_that_cannot_be_written_blocks_the_change() {
    let f = fixture();
    add_run(&f, "Sample", r"C:\s.exe");
    f.system
        .journal
        .fail_next_record(PortError::Other("disk full".into()));

    let error = f
        .manager
        .set_enabled("user-run/Sample", false, Source::Manual)
        .expect_err("blocked");

    assert!(matches!(error, StartupError::Change(ExecError::Journal(_))));
    assert_eq!(f.system.registry.get(&user_approval("Sample")), None);
}

#[test]
fn a_state_that_cannot_be_read_is_an_error_not_a_guess() {
    struct BrokenReads(Arc<crate::fakes::FakeRegistry>);
    impl Registry for BrokenReads {
        fn list_values(&self, key: &RegistryKeyRef) -> Result<Vec<crate::NamedValue>, PortError> {
            self.0.list_values(key)
        }
        fn read_value(
            &self,
            target: &RegistryValueRef,
        ) -> Result<Option<RegistryValue>, PortError> {
            if target.subkey.contains("StartupApproved") {
                Err(PortError::AccessDenied("approved".into()))
            } else {
                self.0.read_value(target)
            }
        }
        fn write_value(&self, t: &RegistryValueRef, v: &RegistryValue) -> Result<(), PortError> {
            self.0.write_value(t, v)
        }
        fn delete_value(&self, t: &RegistryValueRef) -> Result<(), PortError> {
            self.0.delete_value(t)
        }
        fn key_exists(
            &self,
            h: crate::RegistryHive,
            s: &str,
            v: crate::RegistryView,
        ) -> Result<bool, PortError> {
            self.0.key_exists(h, s, v)
        }
    }
    let system = FakeSystem::new();
    let registry = Arc::new(BrokenReads(system.registry.clone()));
    system
        .registry
        .set(&user_run("Sample"), RegistryValue::Sz("x".into()));
    let manager = StartupManager::new(
        registry,
        system.file_system.clone(),
        Arc::new(NoPublisher),
        system.executor(),
        system.clock.clone(),
        sandbox(),
    );

    // The user Run source cannot be read: a problem, and no entry with a made-up state.
    let listing = manager.list();
    assert!(listing.entries.is_empty());
    assert!(
        listing
            .problems
            .iter()
            .any(|p| p.origin == StartupOrigin::UserRun && p.reason == "access denied")
    );
    assert!(matches!(
        manager.set_enabled("user-run/Sample", false, Source::Manual),
        Err(StartupError::Read(PortError::AccessDenied(_)))
    ));
    assert!(system.journal.entries().is_empty());
}

// ---- shortcut targets and logon tasks ------------------------------------------------------

use super::fakes::{FakeLogonTasks, FakeShortcuts};

fn logon_task(path: &str, program: &str, arguments: &str, enabled: bool) -> LogonTask {
    LogonTask {
        path: path.to_owned(),
        program: program.to_owned(),
        arguments: arguments.to_owned(),
        enabled,
        user_id: String::new(),
        elevated: false,
    }
}

#[test]
fn a_shortcut_shows_its_target_command_and_the_publisher_of_the_target() {
    let f = fixture();
    let shortcuts = Arc::new(FakeShortcuts::new());
    shortcuts.set(
        "Notes.lnk",
        r"C:\Program Files\Notes\notes.exe",
        "--minimized",
    );
    shortcuts.fail_on("Broken.lnk", PortError::AccessDenied("x".into()));
    f.publisher
        .set(r"C:\Program Files\Notes\notes.exe", "Notes Inc.");
    for name in ["Notes.lnk", "Broken.lnk", "Folder.lnk"] {
        f.system
            .file_system
            .add_file(format!("{USER_FOLDER}/{name}"), 1);
    }
    let manager = f.manager.with_shortcut_resolver(shortcuts);

    let listing = manager.list();
    let notes = find(&listing, "user-folder/Notes.lnk");
    assert_eq!(
        notes.command,
        r#""C:\Program Files\Notes\notes.exe" --minimized"#
    );
    assert_eq!(
        notes.program.as_deref(),
        Some(r"C:\Program Files\Notes\notes.exe")
    );
    assert_eq!(notes.publisher.as_deref(), Some("Notes Inc."));
    // A shortcut that cannot be read, or has no file target, keeps its file path.
    for id in ["user-folder/Broken.lnk", "user-folder/Folder.lnk"] {
        let entry = find(&listing, id);
        assert!(
            std::path::Path::new(&entry.command)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("lnk")),
            "{}",
            entry.command
        );
        assert_eq!(entry.program, None);
    }
}

#[test]
fn logon_tasks_are_listed_read_only_with_publisher_and_origin() {
    let f = fixture();
    f.publisher.set(r"C:\Tools\sync.exe", "Sync Ltd");
    let tasks = FakeLogonTasks::new(vec![
        logon_task(r"\Sync Updater", r"C:\Tools\sync.exe", "/quiet", true),
        logon_task(
            r"\Microsoft\Windows\Foo\Bar",
            r"%windir%\system32\bar.exe",
            "",
            false,
        ),
        logon_task(r"\Odd", "", "", true),
    ]);
    let manager = f.manager.with_logon_tasks(Arc::new(tasks));

    let listing = manager.list();
    assert_eq!(listing.tasks_problem, None);
    let names: Vec<_> = listing.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Bar", "Odd", "Sync Updater"]);
    let bar = &listing.tasks[0];
    assert!(bar.microsoft && !bar.enabled);
    assert_eq!(bar.folder, r"\Microsoft\Windows\Foo");
    let sync = &listing.tasks[2];
    assert!(!sync.microsoft && sync.enabled);
    assert_eq!(sync.folder, r"\");
    assert_eq!(sync.command, r"C:\Tools\sync.exe /quiet");
    assert_eq!(sync.publisher.as_deref(), Some("Sync Ltd"));
    assert_eq!(listing.tasks[1].command, "");
    // Tasks are not entries: they never reach the switchable list.
    assert!(listing.entries.is_empty());
}

#[test]
fn unreadable_tasks_are_a_problem_class_and_leave_the_entries_alone() {
    let f = fixture();
    add_run(&f, "Alpha", r"C:\a.exe");
    let manager =
        f.manager
            .with_logon_tasks(Arc::new(FakeLogonTasks::failing(PortError::AccessDenied(
                "secret".into(),
            ))));
    let listing = manager.list();
    assert_eq!(listing.entries.len(), 1);
    assert!(listing.tasks.is_empty());
    assert_eq!(listing.tasks_problem, Some("access denied"));
    assert!(listing.problems.is_empty());
}

#[test]
fn listing_entries_only_never_asks_the_task_source() {
    let f = fixture();
    add_run(&f, "Alpha", r"C:.exe");
    let tasks = Arc::new(FakeLogonTasks::new(vec![logon_task(
        r"\Sync Updater",
        r"C:\Tools\sync.exe",
        "",
        true,
    )]));
    let manager = f.manager.with_logon_tasks(tasks.clone());

    let entries = manager.list_entries();
    assert_eq!(entries.entries.len(), 1);
    assert!(entries.tasks.is_empty());
    assert_eq!(tasks.calls(), 0);

    let full = manager.list();
    assert_eq!(full.tasks.len(), 1);
    assert_eq!(tasks.calls(), 1);
}

// ---- RunOnce, auto-start services, protection and the approval flag -------------------------

use super::fakes::FakeAutoStartServices;

fn once_value(leaf: &str, name: &str) -> RegistryValueRef {
    run_value(&format!(r"{ROOT}\{leaf}"), name)
}

fn add_once(f: &Fixture, leaf: &str, name: &str, command: &str) {
    f.system.registry.set(
        &once_value(leaf, name),
        RegistryValue::Sz(command.to_owned()),
    );
}

fn service(name: &str, display: &str, image: &str) -> AutoStartService {
    AutoStartService {
        name: name.to_owned(),
        display_name: display.to_owned(),
        image_path: image.to_owned(),
        delayed: false,
        account: String::new(),
    }
}

#[test]
fn the_approval_flag_is_reported_as_recorded_missing_or_not_used() {
    let f = fixture();
    add_run(&f, "Alpha", r"C:\a.exe");
    add_run(&f, "Beta", r"C:\b.exe");
    f.manager
        .set_enabled("user-run/Beta", false, Source::Manual)
        .expect("disables");
    add_once(&f, "RunOnce", "Gamma", r"C:\g.exe");

    let listing = f.manager.list();

    assert_eq!(
        find(&listing, "user-run/Alpha").approval,
        ApprovalRecord::Missing
    );
    assert_eq!(
        find(&listing, "user-run/Beta").approval,
        ApprovalRecord::Recorded
    );
    assert_eq!(listing.once[0].approval, ApprovalRecord::NotUsed);
}

#[test]
fn run_once_entries_are_listed_apart_from_the_entries_with_their_state_and_origin() {
    let f = fixture();
    add_run(&f, "Alpha", r"C:\a.exe");
    add_once(
        &f,
        "RunOnce",
        "Setup Step",
        r#""C:\Sample\setup.exe" /finish"#,
    );
    add_once(&f, "RunOnceMachine", "Vendor Step", r"C:\Vendor\step.exe");
    add_once(&f, "RunOnceMachine32", "Old Step", r"C:\Legacy\step.exe");
    add_once(&f, "HeldRunOnce", "Held Step", r"C:\Sample\held.exe");
    f.system
        .registry
        .set(&once_value("RunOnce", "Not Text"), RegistryValue::Dword(1));
    f.publisher.set(r"C:\Sample\setup.exe", "Sample Ltd");

    let listing = f.manager.list();
    let entries = f.manager.list_entries();

    assert_eq!(
        listing.entries.len(),
        1,
        "RunOnce is not a switchable entry"
    );
    assert!(entries.once.is_empty(), "the cheap listing skips RunOnce");
    let names: Vec<_> = listing.once.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["Held Step", "Old Step", "Setup Step", "Vendor Step"]
    );
    let held = &listing.once[0];
    assert_eq!(held.origin, StartupOrigin::UserRunOnce);
    assert_eq!(held.state, StartupState::Disabled);
    assert_eq!(held.id, "user-once/Held Step");
    let setup = &listing.once[2];
    assert_eq!(setup.state, StartupState::Enabled);
    assert_eq!(setup.program.as_deref(), Some(r"C:\Sample\setup.exe"));
    assert_eq!(setup.publisher.as_deref(), Some("Sample Ltd"));
    assert_eq!(setup.lock(), None);
    assert_eq!(
        listing.once[1].lock(),
        Some(StartupLock::NeedsAdministrator)
    );
}

#[test]
fn switching_a_run_once_entry_off_moves_it_aside_in_one_journaled_batch_and_on_moves_it_back() {
    let f = fixture();
    add_once(&f, "RunOnce", "Setup Step", r"C:\Sample\setup.exe");

    let dry = f
        .manager
        .plan("user-once/Setup Step", false)
        .expect("plans")
        .expect("a change");
    assert_eq!(dry.items.len(), 2);
    assert!(f.system.log.events().is_empty(), "a dry run writes nothing");

    let change = f
        .manager
        .set_enabled("user-once/Setup Step", false, Source::Manual)
        .expect("disables");

    assert_eq!(change.state, Some(StartupState::Disabled));
    assert_eq!(
        f.system.registry.get(&once_value("RunOnce", "Setup Step")),
        None
    );
    assert_eq!(
        f.system
            .registry
            .get(&once_value("HeldRunOnce", "Setup Step")),
        Some(RegistryValue::Sz(r"C:\Sample\setup.exe".into()))
    );
    let entries = f.system.journal.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].batch, entries[1].batch);
    assert_eq!(
        f.manager.list().once[0].state,
        StartupState::Disabled,
        "still listed, now off"
    );

    let back = f
        .manager
        .set_enabled("user-once/Setup Step", true, Source::Manual)
        .expect("enables");
    assert_eq!(back.state, Some(StartupState::Enabled));
    assert_eq!(
        f.system.registry.get(&once_value("RunOnce", "Setup Step")),
        Some(RegistryValue::Sz(r"C:\Sample\setup.exe".into()))
    );
    assert_eq!(
        f.system
            .registry
            .get(&once_value("HeldRunOnce", "Setup Step")),
        None
    );
}

#[test]
fn undoing_a_run_once_switch_puts_the_value_back_where_it_was() {
    let f = fixture();
    add_once(&f, "RunOnce", "Setup Step", r"C:\Sample\setup.exe");
    let change = f
        .manager
        .set_enabled("user-once/Setup Step", false, Source::Manual)
        .expect("disables");
    let ChangeOutcome::Changed { batch, .. } = change.outcome else {
        panic!("expected a change");
    };

    let report = f
        .manager
        .executor
        .revert_batch(batch, Source::Manual, None)
        .expect("reverts");

    assert!(report.outcomes.iter().all(|outcome| outcome.result.is_ok()));
    assert_eq!(
        f.system.registry.get(&once_value("RunOnce", "Setup Step")),
        Some(RegistryValue::Sz(r"C:\Sample\setup.exe".into()))
    );
    assert_eq!(
        f.system
            .registry
            .get(&once_value("HeldRunOnce", "Setup Step")),
        None
    );
    assert_eq!(f.manager.list().once[0].state, StartupState::Enabled);
}

#[test]
fn a_machine_wide_run_once_entry_needs_administrator_rights() {
    let f = fixture();
    add_once(&f, "RunOnceMachine", "Vendor Step", r"C:\Vendor\step.exe");
    let error = f
        .manager
        .set_enabled("machine-once/Vendor Step", false, Source::Manual)
        .expect_err("refused");
    assert!(matches!(error, StartupError::NeedsAdministrator { .. }));
    assert!(f.system.journal.entries().is_empty());
    let missing = f
        .manager
        .set_enabled("user-once/Nope", false, Source::Manual)
        .expect_err("not there");
    assert!(matches!(missing, StartupError::NotFound(_)));
}

#[test]
fn a_microsoft_entry_is_locked_until_windows_items_are_unlocked() {
    let f = fixture();
    add_run(&f, "Cloud Sync", r"C:\Sample\cloud.exe");
    add_run(&f, "Own", r"C:\Own\own.exe");
    f.publisher
        .set(r"C:\Sample\cloud.exe", "Microsoft Corporation");

    let listing = f.manager.list();
    let cloud = find(&listing, "user-run/Cloud Sync");
    assert_eq!(cloud.protection, StartupProtection::Microsoft);
    assert_eq!(cloud.lock(), Some(StartupLock::Microsoft));
    assert_eq!(find(&listing, "user-run/Own").lock(), None);

    let error = f
        .manager
        .set_enabled("user-run/Cloud Sync", false, Source::Manual)
        .expect_err("locked");
    assert_eq!(
        error,
        StartupError::Protected {
            name: "Cloud Sync".into(),
            critical: false
        }
    );
    assert_eq!(
        f.manager.plan("user-run/Cloud Sync", false),
        Err(error),
        "the dry run refuses the same way"
    );
    assert!(f.system.journal.entries().is_empty());
    assert!(f.system.log.events().is_empty());

    let change = f
        .manager
        .set_enabled_with("user-run/Cloud Sync", false, Source::Manual, true)
        .expect("unlocked");
    assert_eq!(change.state, Some(StartupState::Disabled));
}

#[test]
fn a_critical_entry_is_never_switchable_even_when_windows_items_are_unlocked() {
    let f = fixture();
    add_run(
        &f,
        "SecurityHealth",
        r"%windir%\system32\SecurityHealthSystray.exe",
    );
    f.publisher.set(
        r"%windir%\system32\SecurityHealthSystray.exe",
        "Microsoft Corporation",
    );

    let listing = f.manager.list();
    assert_eq!(
        find(&listing, "user-run/SecurityHealth").lock(),
        Some(StartupLock::Critical)
    );
    for unlocked in [false, true] {
        let error = f
            .manager
            .set_enabled_with("user-run/SecurityHealth", false, Source::Manual, unlocked)
            .expect_err("refused");
        assert_eq!(
            error,
            StartupError::Protected {
                name: "SecurityHealth".into(),
                critical: true
            }
        );
    }
    assert!(f.system.journal.entries().is_empty());
    assert!(
        f.system
            .registry
            .get(&user_approval("SecurityHealth"))
            .is_none()
    );
}

#[test]
fn auto_start_services_are_listed_read_only_with_publisher_and_protection() {
    let f = fixture();
    f.publisher
        .set(r"C:\Program Files\Sample\sync.exe", "Sample Ltd");
    f.publisher
        .set(r"C:\Windows\system32\svchost.exe", "Microsoft Corporation");
    let services = FakeAutoStartServices::new(vec![
        service(
            "SampleSync",
            "Sample Sync Service",
            r#""C:\Program Files\Sample\sync.exe" --service"#,
        ),
        service(
            "Dnscache",
            "DNS Client",
            r"C:\Windows\system32\svchost.exe -k NetworkService -p",
        ),
        service("Themes", "", r"C:\Windows\system32\svchost.exe -k netsvcs"),
        service("NoImage", "No Image", ""),
    ]);
    let manager = f.manager.with_auto_start_services(Arc::new(services));

    let listing = manager.list();

    assert_eq!(listing.services_problem, None);
    let names: Vec<_> = listing.services.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["Dnscache", "NoImage", "SampleSync", "Themes"]);
    let sample = &listing.services[2];
    assert_eq!(sample.publisher.as_deref(), Some("Sample Ltd"));
    assert_eq!(sample.protection, StartupProtection::None);
    assert_eq!(sample.lock(), StartupLock::NeedsHelper);
    assert_eq!(listing.services[0].protection, StartupProtection::Critical);
    assert_eq!(listing.services[0].lock(), StartupLock::Critical);
    // A service without a display name shows its name.
    assert_eq!(listing.services[3].display_name, "Themes");
    assert_eq!(listing.services[1].program, None);
    // Services are not entries, and the cheap listing does not read them.
    assert!(listing.entries.is_empty());
    assert!(manager.list_entries().services.is_empty());
}

#[test]
fn each_service_program_is_asked_about_once() {
    let f = fixture();
    let services = FakeAutoStartServices::new(
        (0..5)
            .map(|n| {
                service(
                    &format!("Svc{n}"),
                    "x",
                    r"C:\Windows\system32\svchost.exe -k group",
                )
            })
            .collect(),
    );
    let publisher = f.publisher.clone();
    let manager = f.manager.with_auto_start_services(Arc::new(services));

    let _ = manager.list();

    assert_eq!(publisher.asked().len(), 1);
}

#[test]
fn unreadable_services_are_a_problem_class_and_leave_the_rest_alone() {
    let f = fixture();
    add_run(&f, "Alpha", r"C:\a.exe");
    let manager = f
        .manager
        .with_auto_start_services(Arc::new(FakeAutoStartServices::failing(
            PortError::AccessDenied("secret".into()),
        )));
    let listing = manager.list();
    assert_eq!(listing.entries.len(), 1);
    assert!(listing.services.is_empty());
    assert_eq!(listing.services_problem, Some("access denied"));
}

#[test]
fn tasks_say_why_they_have_no_switch() {
    let f = fixture();
    let mut system_task = logon_task(r"\Vendor Boot", r"C:\Vendor\boot.exe", "", true);
    system_task.user_id = "SYSTEM".into();
    let mut own_task = logon_task(r"\My Sync", r"C:\Sample\sync.exe", "", true);
    own_task.user_id = r"PC\sample".into();
    let defender = logon_task(r"\Microsoft\Windows\Windows Defender\Scan", "", "", true);
    let manager = f
        .manager
        .with_logon_tasks(Arc::new(FakeLogonTasks::new(vec![
            system_task,
            own_task,
            defender,
        ])));

    let listing = manager.list();
    let lock = |name: &str| {
        listing
            .tasks
            .iter()
            .find(|task| task.name == name)
            .expect("task")
            .lock()
    };
    assert_eq!(lock("Vendor Boot"), StartupLock::NeedsHelper);
    assert_eq!(lock("My Sync"), StartupLock::NotYet);
    assert_eq!(lock("Scan"), StartupLock::Critical);
}

#[test]
fn a_run_once_switch_that_fails_halfway_is_rolled_back() {
    let f = fixture();
    add_once(&f, "RunOnce", "Setup Step", r"C:\Sample\setup.exe");
    // The copy aside works; removing the original is refused.
    f.system.registry.fail_on(
        &once_value("RunOnce", "Setup Step"),
        PortError::AccessDenied("no".into()),
    );

    let error = f
        .manager
        .set_enabled("user-once/Setup Step", false, Source::Manual)
        .expect_err("fails");

    assert!(matches!(error, StartupError::Change(_)), "{error:?}");
    assert_eq!(
        f.system.registry.get(&once_value("RunOnce", "Setup Step")),
        Some(RegistryValue::Sz(r"C:\Sample\setup.exe".into()))
    );
    assert_eq!(
        f.system
            .registry
            .get(&once_value("HeldRunOnce", "Setup Step")),
        None,
        "the copy aside was rolled back"
    );
}
