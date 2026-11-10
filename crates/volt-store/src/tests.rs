//! Store behaviour against real `SQLite` files in temp directories. The system
//! ports are volt-core fakes: nothing here touches the real machine.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rusqlite::Connection;
use tempfile::TempDir;
use volt_core::fakes::{FakeSystem, ManualClock};
use volt_core::{
    BatchId, EntryState, Evidence, Executor, Journal, NewEntry, Operation, OperationRequest, Plan,
    PortError, PriorState, RegistryHive, RegistryValue, RegistryValueRef, Resolution, Risk,
    Settings, SettingsExt, Source, Timestamp,
};

use crate::migrations::{MIGRATIONS, MigrateError, Migration, migrate};
use crate::{Store, StoreError};

fn db_path(dir: &TempDir) -> PathBuf {
    dir.path().join("volt.db")
}

fn open(path: &Path) -> Arc<Store> {
    let outcome = Store::open(path).expect("store opens");
    assert!(outcome.recovery.is_none(), "unexpected recovery");
    Arc::new(outcome.store)
}

fn value_ref(name: &str) -> RegistryValueRef {
    RegistryValueRef::new(RegistryHive::CurrentUser, r"Software\VoltTest\Store", name)
}

fn set_dword(name: &str, value: u32) -> Operation {
    Operation::SetRegistryValue {
        target: value_ref(name),
        value: RegistryValue::Dword(value),
    }
}

fn executor(system: &FakeSystem, store: &Arc<Store>) -> Executor {
    Executor::new(system.ports(), store.clone(), system.clock.clone())
}

fn new_entry(batch: BatchId, operation: Operation, prior: PriorState) -> NewEntry {
    NewEntry {
        batch,
        timestamp: Timestamp::from_unix_millis(10),
        source: Source::Manual,
        operation,
        prior,
    }
}

// ---- persistence and revert across a restart --------------------------------------------

#[test]
fn a_single_entry_and_a_whole_batch_can_be_reverted_after_a_restart() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    system
        .registry
        .set(&value_ref("B"), RegistryValue::Dword(2));

    // Session one: apply two batches, then "quit".
    let store = open(&path);
    let first = executor(&system, &store);
    let plan_one = Plan::build(
        vec![
            OperationRequest::new(set_dword("A", 10), Risk::Safe, Evidence::Measured),
            OperationRequest::new(set_dword("B", 20), Risk::Safe, Evidence::Measured),
        ],
        &system.ports(),
    )
    .expect("plan");
    let report_one = first.apply(&plan_one, Source::Manual).expect("apply");
    assert!(report_one.is_success());
    let plan_two = Plan::build(
        vec![OperationRequest::new(
            set_dword("A", 99),
            Risk::Safe,
            Evidence::Anecdotal,
        )],
        &system.ports(),
    )
    .expect("plan");
    let report_two = first.apply(&plan_two, Source::Manual).expect("apply");
    drop(first);
    drop(store);

    // Session two: a new process opens the same file and undoes things.
    let store = open(&path);
    let second = executor(&system, &store);
    assert!(
        second
            .reconcile_pending()
            .expect("reconcile")
            .outcomes
            .is_empty()
    );

    // Newest batch first: A goes 99 -> 10, via a single entry revert.
    let newest = report_two.applied().next().expect("one applied");
    let volt_core::ItemStatus::Applied { entry } = newest.status else {
        panic!("applied");
    };
    second.revert_entry(entry).expect("revert single entry");
    assert_eq!(
        system.registry.get(&value_ref("A")),
        Some(RegistryValue::Dword(10))
    );

    // Then the whole first batch: back to the original 1 and 2.
    let report = second.revert_batch(report_one.batch).expect("revert batch");
    assert!(report.is_complete());
    assert_eq!(report.outcomes.len(), 2);
    assert_eq!(
        system.registry.get(&value_ref("A")),
        Some(RegistryValue::Dword(1))
    );
    assert_eq!(
        system.registry.get(&value_ref("B")),
        Some(RegistryValue::Dword(2))
    );
    // Reverted state is persisted too.
    drop(second);
    drop(store);
    let store = open(&path);
    let entries = store.batch_entries(report_one.batch).expect("entries");
    assert!(entries.iter().all(|e| e.state == EntryState::Reverted));
}

#[test]
fn entries_round_trip_exactly_including_absent_prior_values() {
    let dir = TempDir::new().expect("temp dir");
    let store = open(&db_path(&dir));
    let batch = store.new_batch().expect("batch");
    let absent = store
        .record_pending(new_entry(
            batch,
            set_dword("X", 5),
            PriorState::RegistryValue(None),
        ))
        .expect("record");
    let present = store
        .record_pending(new_entry(
            batch,
            set_dword("Y", 5),
            PriorState::RegistryValue(Some(RegistryValue::Dword(3))),
        ))
        .expect("record");
    assert!(absent < present);
    store
        .set_state(absent, EntryState::Failed, Some("boom".to_owned()))
        .expect("state");

    let entries = store.batch_entries(batch).expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].id, absent);
    assert_eq!(entries[0].state, EntryState::Failed);
    assert_eq!(entries[0].error.as_deref(), Some("boom"));
    assert_eq!(entries[0].prior, PriorState::RegistryValue(None));
    assert_eq!(entries[1].state, EntryState::Pending);
    assert_eq!(entries[1].timestamp, Timestamp::from_unix_millis(10));
    assert_eq!(
        store.entry(present).expect("read"),
        Some(entries[1].clone())
    );
    assert_eq!(store.entry(volt_core::EntryId(999)).expect("read"), None);
    assert!(matches!(
        store.set_state(volt_core::EntryId(999), EntryState::Applied, None),
        Err(PortError::NotFound(_))
    ));
    assert_eq!(
        store
            .entries_in_state(EntryState::Pending)
            .expect("pending")
            .len(),
        1
    );
    // Ids and batch ids keep growing across reopen.
    drop(store);
    let store = open(&db_path(&dir));
    assert!(store.new_batch().expect("batch") > batch);
}

// ---- durability -----------------------------------------------------------------------------

#[test]
fn record_pending_is_committed_to_disk_before_it_returns() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    let store = open(&path);

    {
        let conn = store.conn();
        let synchronous: i64 = conn
            .pragma_query_value(None, "synchronous", |row| row.get(0))
            .expect("pragma");
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .expect("pragma");
        assert_eq!(synchronous, 2, "synchronous must be FULL");
        assert_eq!(mode, "wal");
    }

    let batch = store.new_batch().expect("batch");
    let id = store
        .record_pending(new_entry(
            batch,
            set_dword("A", 1),
            PriorState::RegistryValue(None),
        ))
        .expect("record");

    // A second, independent connection sees the row while the first is still
    // open and has not checkpointed or closed: the commit already hit the WAL.
    let other = Connection::open(&path).expect("second connection");
    let count: i64 = other
        .query_row(
            "SELECT COUNT(*) FROM journal_entries WHERE id = ?1 AND state = 'Pending'",
            [i64::try_from(id.0).expect("id fits")],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(count, 1);
}

// ---- crash reconciliation ---------------------------------------------------------------

#[test]
fn an_entry_left_pending_by_a_crash_is_reconciled_on_the_next_start() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    let system = FakeSystem::new();
    system
        .registry
        .set(&value_ref("A"), RegistryValue::Dword(1));
    system
        .registry
        .set(&value_ref("B"), RegistryValue::Dword(1));

    // The process wrote both entries, changed only B, and then died.
    let store = open(&path);
    let batch = store.new_batch().expect("batch");
    let never = store
        .record_pending(new_entry(
            batch,
            set_dword("A", 0),
            PriorState::RegistryValue(Some(RegistryValue::Dword(1))),
        ))
        .expect("record");
    let happened = store
        .record_pending(new_entry(
            batch,
            set_dword("B", 0),
            PriorState::RegistryValue(Some(RegistryValue::Dword(1))),
        ))
        .expect("record");
    system
        .registry
        .set(&value_ref("B"), RegistryValue::Dword(0));
    drop(store);

    let store = open(&path);
    let executor = executor(&system, &store);
    let report = executor.reconcile_pending().expect("reconcile");
    assert_eq!(report.outcomes.len(), 2);
    assert_eq!(report.outcomes[0].entry, never);
    assert_eq!(report.outcomes[0].resolution, Resolution::NeverApplied);
    assert_eq!(report.outcomes[1].entry, happened);
    assert_eq!(report.outcomes[1].resolution, Resolution::ConfirmedApplied);

    let never_entry = store.entry(never).expect("read").expect("exists");
    assert_eq!(never_entry.state, EntryState::Failed);
    assert!(never_entry.error.is_some());
    assert_eq!(
        store.entry(happened).expect("read").expect("exists").state,
        EntryState::Applied
    );
    // Nothing is left pending, and the confirmed change can be undone.
    assert!(
        store
            .entries_in_state(EntryState::Pending)
            .expect("pending")
            .is_empty()
    );
    executor.revert_entry(happened).expect("revert");
    assert_eq!(
        system.registry.get(&value_ref("B")),
        Some(RegistryValue::Dword(1))
    );
}

// ---- migrations -----------------------------------------------------------------------------

const TEST_V2: Migration = Migration {
    version: 2,
    sql: "ALTER TABLE journal_entries ADD COLUMN note TEXT;",
};

fn user_version(conn: &Connection) -> u32 {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version")
}

#[test]
fn a_database_at_an_older_schema_version_is_upgraded_and_keeps_its_data() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    {
        // A database as an older build left it: version 1 with data.
        let store = open(&path);
        store
            .set("theme", &serde_json::json!("dark"))
            .expect("setting");
        let batch = store.new_batch().expect("batch");
        store
            .record_pending(new_entry(
                batch,
                set_dword("A", 1),
                PriorState::RegistryValue(None),
            ))
            .expect("record");
    }

    let mut conn = Connection::open(&path).expect("open raw");
    assert_eq!(user_version(&conn), 1);
    migrate(&mut conn, &[MIGRATIONS[0], TEST_V2]).expect("upgrade");
    assert_eq!(user_version(&conn), 2);
    let (rows, note): (i64, Option<String>) = conn
        .query_row("SELECT COUNT(*), MAX(note) FROM journal_entries", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .expect("query new column");
    assert_eq!((rows, note), (1, None));
    let theme: String = conn
        .query_row("SELECT value FROM settings WHERE key = 'theme'", [], |r| {
            r.get(0)
        })
        .expect("setting survives");
    assert_eq!(theme, "\"dark\"");

    // Migrating again is a no-op.
    migrate(&mut conn, &[MIGRATIONS[0], TEST_V2]).expect("idempotent");
    assert_eq!(user_version(&conn), 2);
}

#[test]
fn a_fresh_database_gets_the_current_schema_version() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    drop(open(&path));
    let conn = Connection::open(&path).expect("open raw");
    assert_eq!(
        user_version(&conn),
        MIGRATIONS.last().expect("has migrations").version
    );
}

#[test]
fn a_failing_migration_rolls_back_and_keeps_the_old_version() {
    let mut conn = Connection::open_in_memory().expect("memory db");
    migrate(&mut conn, MIGRATIONS).expect("v1");
    let broken = Migration {
        version: 2,
        sql: "ALTER TABLE journal_entries ADD COLUMN ok TEXT; ALTER TABLE nope ADD COLUMN x TEXT;",
    };
    let result = migrate(&mut conn, &[MIGRATIONS[0], broken]);
    assert!(matches!(result, Err(MigrateError::Sql(_))));
    assert_eq!(user_version(&conn), 1);
    // The half-applied first statement was rolled back with the rest.
    assert!(
        conn.prepare("SELECT ok FROM journal_entries").is_err(),
        "column from the failed migration must not exist"
    );
}

#[test]
fn a_database_from_a_newer_build_is_refused_and_left_alone() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    drop(open(&path));
    {
        let conn = Connection::open(&path).expect("open raw");
        conn.pragma_update(None, "user_version", 99).expect("bump");
    }
    let before = fs::read(&path).expect("read");
    let error = Store::open(&path).expect_err("must refuse");
    assert_eq!(
        error,
        StoreError::SchemaTooNew {
            found: 99,
            supported: 1
        }
    );
    assert_eq!(fs::read(&path).expect("read"), before);
    assert_eq!(backups(&dir).len(), 0, "no backup for a newer schema");
}

// ---- corruption ---------------------------------------------------------------------------

fn backups(dir: &TempDir) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir.path())
        .expect("read dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".corrupt-"))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn a_garbage_file_is_backed_up_and_replaced_and_the_caller_is_told() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    let garbage = b"this is definitely not a sqlite database. ".repeat(100);
    fs::write(&path, &garbage).expect("write garbage");

    let outcome = Store::open_with_clock(&path, Arc::new(ManualClock::new(1_234))).expect("open");
    let recovery = outcome.recovery.expect("recovery must be reported");
    assert_eq!(
        recovery.backup_path.file_name().and_then(|n| n.to_str()),
        Some("volt.db.corrupt-1234")
    );
    assert!(!recovery.reason.is_empty());
    // The corrupt file is preserved byte for byte, not deleted.
    assert_eq!(fs::read(&recovery.backup_path).expect("backup"), garbage);
    // The store at the original path is a working, empty database.
    let store = outcome.store;
    assert_eq!(store.get("anything").expect("get"), None);
    let batch = store.new_batch().expect("batch");
    store
        .record_pending(new_entry(
            batch,
            set_dword("A", 1),
            PriorState::RegistryValue(None),
        ))
        .expect("record");
    drop(store);
    // The next start is clean.
    assert!(Store::open(&path).expect("reopen").recovery.is_none());
}

#[test]
fn a_database_with_damaged_pages_is_detected_by_the_integrity_check() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    {
        let store = open(&path);
        let batch = store.new_batch().expect("batch");
        for i in 0..50 {
            store
                .record_pending(new_entry(
                    batch,
                    set_dword("A", i),
                    PriorState::RegistryValue(None),
                ))
                .expect("record");
        }
    }
    // Keep the header page valid, destroy everything after it.
    let mut bytes = fs::read(&path).expect("read");
    assert!(bytes.len() > 4096, "need more than one page");
    for byte in &mut bytes[4096..] {
        *byte = 0xFF;
    }
    fs::write(&path, &bytes).expect("damage");

    let outcome = Store::open(&path).expect("open");
    let recovery = outcome.recovery.expect("damage must be reported");
    assert_eq!(fs::read(&recovery.backup_path).expect("backup"), bytes);
    assert!(
        outcome
            .store
            .entries_in_state(EntryState::Pending)
            .expect("fresh")
            .is_empty()
    );
}

#[test]
fn repeated_corruption_never_overwrites_an_earlier_backup() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    for round in 0..2_u8 {
        fs::write(&path, vec![round + 1; 8192]).expect("write garbage");
        let outcome = Store::open_with_clock(&path, Arc::new(ManualClock::new(5))).expect("open");
        assert!(outcome.recovery.is_some());
        drop(outcome);
        fs::remove_file(&path).expect("remove fresh db for the next round");
        let _ = fs::remove_file(path.with_extension("db-wal"));
        let _ = fs::remove_file(path.with_extension("db-shm"));
    }
    let found = backups(&dir);
    assert_eq!(found.len(), 2);
    assert_eq!(fs::read(&found[0]).expect("first"), vec![1; 8192]);
    assert_eq!(fs::read(&found[1]).expect("second"), vec![2; 8192]);
}

#[test]
fn a_failure_that_is_not_corruption_does_not_move_anything() {
    let dir = TempDir::new().expect("temp dir");
    // A directory where the database file should be: cannot be opened, is not corrupt.
    let path = dir.path().join("volt.db");
    fs::create_dir(&path).expect("dir");
    let error = Store::open(&path).expect_err("must fail");
    assert!(matches!(error, StoreError::Open { .. }));
    assert!(path.is_dir());
    assert!(backups(&dir).is_empty());
}

#[test]
fn the_parent_directory_is_created_but_its_location_is_the_callers_choice() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("nested").join("deeper").join("volt.db");
    drop(open(&path));
    assert!(path.is_file());
}

// ---- settings -------------------------------------------------------------------------------

#[test]
fn settings_are_stored_as_json_and_survive_a_restart() {
    let dir = TempDir::new().expect("temp dir");
    let path = db_path(&dir);
    let store = open(&path);
    assert_eq!(store.get("missing").expect("get"), None);
    store
        .set("ui", &serde_json::json!({"theme": "dark", "scale": 1.25}))
        .expect("set");
    store.set_typed("auto_purge", &true).expect("typed set");
    store
        .set("ui", &serde_json::json!({"theme": "light"}))
        .expect("overwrite");
    drop(store);

    let store = open(&path);
    assert_eq!(
        store.get("ui").expect("get"),
        Some(serde_json::json!({"theme": "light"}))
    );
    assert_eq!(
        store.get_typed::<bool>("auto_purge").expect("typed"),
        Some(true)
    );
    assert!(matches!(
        store.get_typed::<u32>("auto_purge"),
        Err(PortError::InvalidInput(_))
    ));
    assert!(store.remove("ui").expect("remove"));
    assert!(!store.remove("ui").expect("remove again"));
    assert_eq!(store.get("ui").expect("get"), None);
}
