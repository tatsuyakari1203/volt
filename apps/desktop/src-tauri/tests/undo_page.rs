//! The Undo page's backend, tested through the functions the Tauri commands call:
//! [`list_activity`], [`undo_entry`], [`undo_batch`], [`preview_undo_batch`] and the export.
//!
//! Like `end_to_end.rs`, two kinds of test live here, and neither touches anything it should not:
//!
//! - **Real adapters.** `undo_one_entry_then_a_batch_then_nothing_survives_a_restart` uses the real
//!   `WinRegistry` and a real `Store` in a temp directory, against a test catalogue whose every
//!   step targets one unique throwaway key under `HKCU\Software\VoltTest`. The real "Show file
//!   extensions" tweak is never applied or reverted here.
//! - **Fakes.** Everything else runs the same wiring over the in-memory fakes of `volt-core` (plus
//!   a real `Store` in a temp directory) to reach outcomes a real machine cannot produce on demand.

use std::ffi::OsStr;
use std::path::Path;
use std::process::Command as Process;
use std::time::{SystemTime, UNIX_EPOCH};

use tempfile::TempDir;
use volt_core::fakes::FakeSystem;
use volt_core::{
    ActivityOutcome, BatchId, EntryState, NewActivity, NewEntry, Operation, PortError, PriorState,
    Registry, RegistryHive, RegistryValue, RegistryValueRef, Source, Timestamp,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{
    ActivityItem, ActivityStatus, ApplyStatus, ChangeState, ExportResult, UndoStatus, apply_tweak,
    clear_history, export_activity_text, export_file_name, history_status, list_activity,
    preview_undo_batch, preview_undo_entry, undo_batch, undo_entry, write_export,
};
use volt_desktop_lib::wiring::{AppCore, build_core, windows_ports};
use volt_win::WinRegistry;

// ---- helpers -------------------------------------------------------------------------------

/// A catalogue with one two-step registry tweak. `subkey` is where both steps point.
fn flag_catalogue(subkey: &str) -> String {
    format!(
        r#"
[[tweak]]
id = "e2e-flag"
title = "End to end flag"
description = "Test tweak."
category = "performance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = '{subkey}'
name = "Flag"
value = {{ dword = 0 }}

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = '{subkey}'
name = "Pre"
value = {{ dword = 1 }}
"#
    )
}

fn value_ref(subkey: &str, name: &str) -> RegistryValueRef {
    RegistryValueRef::new(RegistryHive::CurrentUser, subkey, name)
}

fn db(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("volt.db")
}

fn fake_core(dir: &TempDir, subkey: &str) -> (FakeSystem, AppCore) {
    let system = FakeSystem::new();
    let core = build_core(system.ports(), &db(dir), &flag_catalogue(subkey)).expect("core builds");
    (system, core)
}

fn items(core: &AppCore) -> Vec<ActivityItem> {
    list_activity(core, 100, None)
        .expect("activity lists")
        .items
}

/// The row with this summary fragment and status (there is one per step and status).
fn row<'a>(items: &'a [ActivityItem], name: &str, status: ActivityStatus) -> &'a ActivityItem {
    items
        .iter()
        .find(|item| item.status == status && item.summary.contains(name))
        .unwrap_or_else(|| panic!("no {status:?} row for {name} in {items:#?}"))
}

/// A throwaway key under `HKCU\Software\VoltTest`, removed on drop.
struct TestKey {
    subkey: String,
}

impl TestKey {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self {
            subkey: format!(r"Software\VoltTest\UndoPage-{}-{nanos}", std::process::id()),
        }
    }

    fn value(&self, name: &str) -> RegistryValueRef {
        value_ref(&self.subkey, name)
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        // The unique test key is removed with the system's own `reg` tool. The path is checked,
        // so this can only ever name a key below `HKCU\Software\VoltTest\UndoPage-`.
        assert!(
            self.subkey.starts_with(r"Software\VoltTest\UndoPage-"),
            "refusing to delete {}",
            self.subkey
        );
        let _ = Process::new("reg")
            .args(["delete", &format!(r"HKCU\{}", self.subkey), "/f"])
            .output();
    }
}

// ---- real registry, real store ---------------------------------------------------------------

/// The real registry and a real store: a throwaway key where `Pre` is 5 and `Flag` does not exist,
/// the test catalogue that targets only that key, and a temp directory for the database.
struct Real {
    key: TestKey,
    registry: WinRegistry,
    catalogue: String,
    dir: TempDir,
}

impl Real {
    fn new() -> Self {
        let key = TestKey::new();
        let registry = WinRegistry::new();
        let catalogue = flag_catalogue(&key.subkey);
        // Guard: these tests must never point at the real Explorer key.
        assert!(
            !catalogue.to_lowercase().contains("explorer"),
            "the test catalogue must only target the VoltTest key"
        );
        registry
            .write_value(&key.value("Pre"), &RegistryValue::Dword(5))
            .expect("arrange Pre");
        Self {
            key,
            registry,
            catalogue,
            dir: TempDir::new().expect("temp dir"),
        }
    }

    /// A core over the real registry and this database; building it again is a restart.
    fn core(&self) -> AppCore {
        build_core(windows_ports(), &db(&self.dir), &self.catalogue).expect("core builds")
    }

    fn read(&self, name: &str) -> Result<Option<RegistryValue>, PortError> {
        self.registry.read_value(&self.key.value(name))
    }
}

#[test]
fn undo_one_entry_then_the_rest_of_the_batch_and_it_all_survives_a_restart() {
    let real = Real::new();

    // Apply: one batch with two entries.
    let core = real.core();
    assert_eq!(
        apply_tweak(&core, "e2e-flag").expect("apply").status,
        ApplyStatus::Applied
    );
    let rows = items(&core);
    assert_eq!(rows.len(), 2);
    let flag = row(&rows, "Flag", ActivityStatus::Applied).clone();
    let pre = row(&rows, "Pre", ActivityStatus::Applied).clone();
    assert_eq!(flag.batch, pre.batch, "one apply is one batch");
    let batch = flag.batch.clone().expect("a batch");
    for item in [&flag, &pre] {
        assert_eq!(item.state, ChangeState::CanUndo);
        assert_eq!(item.title.as_deref(), Some("End to end flag"));
        assert_eq!(item.label.as_deref(), Some("e2e-flag"));
        assert_eq!(item.reason, None);
    }
    assert_eq!(flag.previous.as_deref(), Some("(not set)"));
    assert_eq!(pre.previous.as_deref(), Some("REG_DWORD 5"));

    // Undo one entry: Pre only.
    let pre_entry = pre.entry.clone().expect("an entry");
    let result = undo_entry(&core, &pre_entry).expect("undo one");
    assert_eq!(
        (result.status, result.restored, result.failed),
        (UndoStatus::Restored, 1, 0)
    );
    assert_eq!(real.read("Pre"), Ok(Some(RegistryValue::Dword(5))));
    assert_eq!(
        real.read("Flag"),
        Ok(Some(RegistryValue::Dword(0))),
        "the other entry of the batch is untouched"
    );

    // A second undo of the same entry has nothing to do and changes nothing.
    let again = undo_entry(&core, &pre_entry).expect("second undo");
    assert_eq!(again.status, UndoStatus::NothingToUndo);
    assert_eq!((again.restored, again.failed), (0, 0));
    assert_eq!(real.read("Pre"), Ok(Some(RegistryValue::Dword(5))));

    // The restart: only the database and the registry stay.
    drop(core);
    let core = real.core();
    let rows = items(&core);
    assert_eq!(
        row(&rows, "Pre", ActivityStatus::Applied).state,
        ChangeState::Undone
    );
    assert_eq!(
        row(&rows, "Flag", ActivityStatus::Applied).state,
        ChangeState::CanUndo
    );
    let restored = row(&rows, "Pre", ActivityStatus::Reverted);
    assert_eq!(restored.state, ChangeState::Restored);
    assert_eq!(restored.previous, None);
    assert_eq!(
        restored.label.as_deref(),
        Some("e2e-flag"),
        "an undo from the page keeps the tweak label"
    );
    assert_eq!(restored.batch.as_deref(), Some(batch.as_str()));

    // Undo the batch after the restart: only what is still applied is restored.
    let preview = preview_undo_batch(&core, &batch).expect("preview");
    assert_eq!(preview.items.len(), 1);
    assert!(
        preview.items[0].what.starts_with("Registry value "),
        "{preview:?}"
    );
    assert!(preview.items[0].what.ends_with("Flag"), "{preview:?}");
    assert_eq!(
        preview.items[0].restore,
        "removed (it did not exist before)"
    );
    let result = undo_batch(&core, &batch).expect("undo batch");
    assert_eq!(
        (result.status, result.restored, result.failed),
        (UndoStatus::Restored, 1, 0)
    );
    assert_eq!(
        real.read("Flag"),
        Ok(None),
        "a value that did not exist before must not exist after the undo"
    );
    assert!(
        preview_undo_batch(&core, &batch)
            .expect("preview again")
            .items
            .is_empty()
    );

    // A second undo of the batch answers "nothing to undo".
    let nothing = undo_batch(&core, &batch).expect("second batch undo");
    assert_eq!(nothing.status, UndoStatus::NothingToUndo);
    assert_eq!(real.read("Pre"), Ok(Some(RegistryValue::Dword(5))));

    // Applying again is a new batch, not the old one coming back to life.
    assert_eq!(
        apply_tweak(&core, "e2e-flag").expect("apply again").status,
        ApplyStatus::Applied
    );
    let newest = items(&core)[0].batch.clone().expect("a batch");
    assert_ne!(newest, batch, "a new apply is a new batch");
}

#[test]
fn a_whole_batch_of_two_is_undone_in_one_go_after_a_restart_and_every_undo_is_recorded() {
    let real = Real::new();
    let core = real.core();
    apply_tweak(&core, "e2e-flag").expect("apply");
    let batch = items(&core)[0].batch.clone().expect("a batch");
    drop(core);

    let core = real.core();
    let preview = preview_undo_batch(&core, &batch).expect("preview");
    assert_eq!(preview.items.len(), 2);
    assert!(
        preview.items[0].what.ends_with("Pre"),
        "newest first: {preview:?}"
    );
    assert_eq!(preview.items[0].restore, "set back to REG_DWORD 5");
    let result = undo_batch(&core, &batch).expect("undo batch");
    assert_eq!(
        (result.status, result.restored, result.failed),
        (UndoStatus::Restored, 2, 0)
    );
    assert_eq!(real.read("Flag"), Ok(None));
    assert_eq!(real.read("Pre"), Ok(Some(RegistryValue::Dword(5))));

    // A second undo of the batch answers "nothing to undo" and changes nothing.
    let nothing = undo_batch(&core, &batch).expect("second batch undo");
    assert_eq!(
        (nothing.status, nothing.restored, nothing.failed),
        (UndoStatus::NothingToUndo, 0, 0)
    );
    assert_eq!(real.read("Flag"), Ok(None));
    assert_eq!(real.read("Pre"), Ok(Some(RegistryValue::Dword(5))));

    // Every undo is in the history, newest first, and every applied row says it was undone.
    let rows = items(&core);
    let count = |status| rows.iter().filter(|item| item.status == status).count();
    assert_eq!(count(ActivityStatus::Reverted), 2);
    assert_eq!(count(ActivityStatus::Applied), 2);
    assert!(
        rows.iter()
            .filter(|item| item.status == ActivityStatus::Applied)
            .all(|item| item.state == ChangeState::Undone)
    );
    assert!(rows.windows(2).all(|pair| {
        pair[0].id.parse::<u64>().unwrap_or(0) > pair[1].id.parse::<u64>().unwrap_or(0)
    }));
}

// ---- fakes: partial failure ---------------------------------------------------------------

#[test]
fn a_batch_that_cannot_be_fully_undone_says_what_failed_and_can_be_finished_later() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, subkey);
    apply_tweak(&core, "e2e-flag").expect("apply");
    let batch = items(&core)[0].batch.clone().expect("a batch");

    // Windows refuses to restore Pre; Flag is restored.
    system.registry.fail_on(
        &value_ref(subkey, "Pre"),
        PortError::AccessDenied("hkcu".into()),
    );
    let first = undo_batch(&core, &batch).expect("a partial undo is a result");
    assert_eq!(first.status, UndoStatus::Partial);
    assert_eq!((first.restored, first.failed), (1, 1));
    assert_eq!(first.failures.len(), 1);
    assert_eq!(
        first.failures[0].what,
        format!(r"Registry value HKCU\{subkey}\Pre")
    );
    assert!(
        first.failures[0].reason.contains("access denied"),
        "{first:?}"
    );
    assert_eq!(system.registry.get(&value_ref(subkey, "Flag")), None);

    // The failed entry stays undoable and the page says so.
    let rows = items(&core);
    assert_eq!(
        row(&rows, "Pre", ActivityStatus::Applied).state,
        ChangeState::CanUndo
    );
    assert_eq!(
        row(&rows, "Flag", ActivityStatus::Applied).state,
        ChangeState::Undone
    );
    assert!(
        rows.iter()
            .any(|item| item.status == ActivityStatus::Failed && item.state == ChangeState::Failed),
        "the failed revert is a failed row"
    );

    // One entry on its own, failing: nothing was restored.
    let pre = row(&rows, "Pre", ActivityStatus::Applied)
        .entry
        .clone()
        .expect("entry");
    let single = undo_entry(&core, &pre).expect("a failed undo is a result");
    assert_eq!(single.status, UndoStatus::Failed);
    assert_eq!((single.restored, single.failed), (0, 1));

    // Once the cause is gone the rest can be undone.
    system.registry.clear_failures();
    let second = undo_batch(&core, &batch).expect("retry");
    assert_eq!(second.status, UndoStatus::Restored);
    assert_eq!((second.restored, second.failed), (1, 0));
    assert_eq!(system.registry.get(&value_ref(subkey, "Pre")), None);
}

// ---- row states -------------------------------------------------------------------------------

#[test]
fn rows_without_a_journal_entry_or_with_a_missing_one_cannot_be_undone_and_say_why() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, r"Software\VoltTest\Fake");
    let record = |summary: &str, outcome, entry| NewActivity {
        timestamp: Timestamp::from_unix_millis(1_000),
        source: Source::Manual,
        outcome,
        summary: summary.to_owned(),
        batch: None,
        entry,
        label: None,
    };
    // A cleaning or memory action: an applied record with no journal entry at all.
    core.activity
        .record(record(
            "Moved 3 files to the Recycle Bin",
            ActivityOutcome::Applied,
            None,
        ))
        .expect("record");
    // A record whose journal entry is gone.
    core.activity
        .record(record(
            "Applied something",
            ActivityOutcome::Applied,
            Some(volt_core::EntryId(999)),
        ))
        .expect("record");
    core.activity
        .record(record("Failed to clean", ActivityOutcome::Failed, None))
        .expect("record");

    let rows = items(&core);
    assert_eq!(rows.len(), 3);
    let failed = &rows[0];
    assert_eq!(failed.state, ChangeState::Failed);
    assert_eq!(failed.reason, None);

    let missing = &rows[1];
    assert_eq!(missing.state, ChangeState::CannotUndo);
    assert!(
        missing
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("no longer in the journal")),
        "{missing:?}"
    );
    assert_eq!(missing.previous, None);

    let cleaned = &rows[2];
    assert_eq!(cleaned.state, ChangeState::CannotUndo);
    assert_eq!(cleaned.entry, None);
    assert_eq!(cleaned.batch, None);
    assert!(
        cleaned
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("saved no earlier value")),
        "{cleaned:?}"
    );

    // Asking to undo what has no journal entry is an error the UI can show, not a guess.
    assert!(matches!(
        undo_entry(&core, "999"),
        Err(IpcError::NotFound { .. })
    ));
    assert!(matches!(
        undo_entry(&core, "not a number"),
        Err(IpcError::InvalidInput { .. })
    ));
    assert!(matches!(
        undo_batch(&core, "-1"),
        Err(IpcError::InvalidInput { .. })
    ));
    assert_eq!(
        undo_batch(&core, "999").expect("unknown batch").status,
        UndoStatus::NothingToUndo
    );
}

#[test]
fn a_failed_apply_and_its_rollback_are_a_failed_row_and_a_restored_row() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, subkey);
    system.registry.fail_on(
        &value_ref(subkey, "Pre"),
        PortError::AccessDenied("hkcu".into()),
    );
    assert_eq!(
        apply_tweak(&core, "e2e-flag").expect("result").status,
        ApplyStatus::RolledBack
    );
    let rows = items(&core);
    let states: Vec<_> = rows.iter().map(|item| (item.status, item.state)).collect();
    assert_eq!(
        states,
        [
            (ActivityStatus::RolledBack, ChangeState::Restored),
            (ActivityStatus::Failed, ChangeState::Failed),
            (ActivityStatus::Applied, ChangeState::Undone),
        ],
        "the step that was applied and then rolled back counts as undone"
    );
    // The whole batch is one group for the page, and it has nothing left to undo.
    assert!(rows.iter().all(|item| item.batch == rows[0].batch));
    let batch = rows[0].batch.clone().expect("batch");
    assert_eq!(
        undo_batch(&core, &batch).expect("nothing").status,
        UndoStatus::NothingToUndo
    );
}

// ---- redaction and export --------------------------------------------------------------------

#[test]
fn previous_values_and_the_export_follow_the_redaction_rule() {
    const SECRET: &str = "hunter2-SECRET-9f3a";
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, subkey);
    // The value Pre holds a string with a secret; Flag does not exist.
    system.registry.set(
        &value_ref(subkey, "Pre"),
        RegistryValue::Sz(SECRET.to_owned()),
    );
    apply_tweak(&core, "e2e-flag").expect("apply");

    let rows = items(&core);
    let pre = row(&rows, "Pre", ActivityStatus::Applied);
    assert_eq!(
        pre.previous.as_deref(),
        Some(format!("REG_SZ ({} chars)", SECRET.chars().count()).as_str()),
        "a string is shown as kind and length only"
    );
    let batch = pre.batch.clone().expect("batch");
    let preview = preview_undo_batch(&core, &batch).expect("preview");
    assert!(
        preview
            .items
            .iter()
            .any(|item| item.restore.starts_with("set back to REG_SZ (")),
        "{preview:?}"
    );

    undo_batch(&core, &batch).expect("undo");
    let export = export_activity_text(&core).expect("export");
    assert_eq!(export.records, 4);
    assert_eq!(export.text.lines().count(), 4, "one line per record");
    assert!(
        !export.text.contains(SECRET),
        "the secret must not reach the export"
    );
    let everything = format!("{:?}{:?}{preview:?}", items(&core), export.text);
    assert!(
        !everything.contains(SECRET),
        "the secret must not reach the page"
    );
    // Newest first, exactly what the page lists.
    let first_line = export.text.lines().next().expect("a line");
    assert!(first_line.contains("reverted"), "{first_line}");
    assert!(
        first_line.contains("[label e2e-flag, batch "),
        "{first_line}"
    );
    // And the registry really holds the secret again: the journal restored it in full.
    assert_eq!(
        system.registry.get(&value_ref(subkey, "Pre")),
        Some(RegistryValue::Sz(SECRET.to_owned()))
    );
}

#[test]
fn the_export_is_written_to_the_chosen_file_and_named_by_the_date() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, r"Software\VoltTest\Fake");
    apply_tweak(&core, "e2e-flag").expect("apply");
    let export = export_activity_text(&core).expect("export");
    assert_eq!(export.records, 2);
    assert!(
        export.file_name.starts_with("volt-activity-")
            && Path::new(&export.file_name).extension() == Some(OsStr::new("txt")),
        "{}",
        export.file_name
    );
    assert_eq!(
        export_file_name(Timestamp::from_unix_millis(1_794_000_000_000)),
        "volt-activity-2026-11-06.txt"
    );

    let path = dir.path().join("saved.txt");
    write_export(&path, &export.text).expect("written");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        export.text
    );

    // A location that cannot be written is an error the UI can show.
    let missing = dir.path().join("no-such-folder").join("saved.txt");
    assert!(matches!(
        write_export(&missing, &export.text),
        Err(IpcError::NotFound { .. })
    ));
}

#[test]
fn an_empty_log_exports_an_empty_text_and_cancelling_is_a_value_not_an_error() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, r"Software\VoltTest\Fake");
    let export = export_activity_text(&core).expect("export");
    assert_eq!((export.records, export.text.as_str()), (0, ""));
    // The save dialog itself needs a window; its two outcomes are plain values.
    let cancelled = serde_json::to_value(ExportResult::Cancelled).expect("serializes");
    assert_eq!(cancelled, serde_json::json!({ "kind": "cancelled" }));
    let saved = serde_json::to_value(ExportResult::Saved {
        path: "C:/x.txt".to_owned(),
        records: 2,
    })
    .expect("serializes");
    assert_eq!(
        saved,
        serde_json::json!({ "kind": "saved", "path": "C:/x.txt", "records": 2 })
    );
}

// ---- later changes and incomplete history ------------------------------------------------------

#[test]
fn undoing_an_older_change_names_the_later_change_it_would_overwrite() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, r"Software\VoltTest\Overwrite");
    apply_tweak(&core, "e2e-flag").expect("apply");
    let rows = items(&core);
    let older = row(&rows, "Flag", ActivityStatus::Applied)
        .entry
        .clone()
        .expect("entry");
    assert!(
        preview_undo_entry(&core, &older)
            .expect("preview")
            .overwrites
            .is_empty(),
        "nothing changed Flag since"
    );

    // Something else changes the same value later: a journal entry only, the fake system is
    // not touched.
    let batch = core.journal.new_batch(None).unwrap_or(BatchId(99));
    let later = core
        .journal
        .record_pending(NewEntry {
            batch,
            timestamp: Timestamp::from_unix_millis(5),
            source: Source::Automation,
            operation: Operation::SetRegistryValue {
                target: value_ref(r"Software\VoltTest\Overwrite", "Flag"),
                value: RegistryValue::Dword(7),
            },
            prior: PriorState::RegistryValue(Some(RegistryValue::Dword(0))),
        })
        .expect("record");
    core.journal
        .set_state(later, EntryState::Applied, None)
        .expect("applied");

    let preview = preview_undo_entry(&core, &older).expect("preview");
    assert_eq!(preview.items.len(), 1);
    assert_eq!(preview.overwrites.len(), 1, "{preview:?}");
    assert_eq!(preview.overwrites[0].entry, older);
    assert_eq!(preview.overwrites[0].later_entry, later.0.to_string());
    assert!(preview.overwrites[0].what.ends_with("Flag"), "{preview:?}");

    // Once the later change is undone, the warning is gone.
    core.journal
        .set_state(later, EntryState::Reverted, None)
        .expect("reverted");
    assert!(
        preview_undo_entry(&core, &older)
            .expect("preview")
            .overwrites
            .is_empty()
    );
}

#[test]
fn the_history_notice_stays_once_an_activity_write_failed() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, r"Software\VoltTest\Incomplete");
    assert!(!history_status(&core).expect("status").incomplete);
    core.note_history_incomplete();
    assert!(history_status(&core).expect("status").incomplete);
    assert!(history_status(&core).expect("status").incomplete);
}

// ---- clearing the history --------------------------------------------------------------------

#[test]
fn clearing_the_history_keeps_what_can_still_be_undone_and_never_touches_the_journal() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\FakeClear";
    let (_system, core) = fake_core(&dir, subkey);
    apply_tweak(&core, "e2e-flag").expect("apply");
    let rows = items(&core);
    let batch = rows[0].batch.clone().expect("batch");

    // Everything is still undoable: nothing is removed.
    assert_eq!(clear_history(&core).expect("clear").removed, 0);
    assert_eq!(items(&core).len(), rows.len());

    undo_batch(&core, &batch).expect("undo");
    let before = items(&core).len();
    assert_eq!(before, 4);
    // The applied records stay as undone history; once nothing can be undone they all go.
    let removed = clear_history(&core).expect("clear").removed;
    assert_eq!(removed as usize, before);
    assert!(items(&core).is_empty());
    // The journal still has its (reverted) entries.
    let entries = core
        .journal
        .batch_entries(BatchId(batch.parse().expect("id")));
    assert_eq!(entries.expect("entries").len(), 2);
}
