//! The foundation's vertical slice, tested through the same wiring function and the same
//! functions the Tauri commands call: [`build_core`], [`list_tweaks`], [`apply_tweak`],
//! [`revert_tweak`] and [`list_activity`].
//!
//! Two kinds of test live here, and neither touches anything it should not:
//!
//! - **Real adapters.** `real_registry_flow_survives_a_restart_and_reverts_exactly` uses the
//!   real `WinRegistry` and a real `Store` in a temp directory, against a test catalogue whose
//!   every step targets only a throwaway key under `HKCU\Software\VoltTest`. The real
//!   "Show file extensions" tweak is never applied or reverted here (or anywhere in the tests).
//! - **Fakes.** Everything else runs the same wiring over the in-memory fakes of `volt-core`
//!   (plus a real `Store` in a temp directory) to reach the failure outcomes a real machine
//!   cannot produce on demand.

use std::path::Path;
use std::process::Command as Process;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, Log, Metadata, Record};
use tempfile::TempDir;
use volt_core::fakes::{FakeActivityLog, FakeSystem};
use volt_core::{
    Command, EntryId, EntryState, Journal, NewEntry, Operation, PortError, Ports, PriorState,
    Registry, RegistryHive, RegistryValue, RegistryValueRef, Resolution, Source, Timestamp,
    TweakEngine,
};
use volt_desktop_lib::IpcError;
use volt_desktop_lib::api::{
    ActivityStatus, ApplyStatus, RevertStatus, TweakStateView, apply_tweak, list_activity,
    list_tweaks, revert_tweak,
};
use volt_desktop_lib::catalogue::EMBEDDED_CATALOGUE;
use volt_desktop_lib::wiring::{AppCore, StartupReport, build_core, windows_ports};
use volt_store::Store;
use volt_win::WinRegistry;

// ---- helpers -------------------------------------------------------------------------------

/// Captures warnings, so a test can prove the shell logs them. The logger is global to this test
/// binary; every test that wants a record just searches the whole capture.
struct Capture(Mutex<Vec<String>>);

impl Log for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= Level::Warn
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(format!("{}: {}", record.level(), record.args()));
        }
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

fn capture_warnings() {
    // Another test may have installed it already; that is fine.
    if log::set_logger(&CAPTURE).is_ok() {
        log::set_max_level(log::LevelFilter::Warn);
    }
}

fn captured() -> Vec<String> {
    CAPTURE
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

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

fn tweak_state(core: &AppCore, id: &str) -> TweakStateView {
    list_tweaks(core)
        .expect("list works")
        .into_iter()
        .find(|tweak| tweak.id == id)
        .unwrap_or_else(|| panic!("tweak {id} is listed"))
        .state
}

/// A fake machine and the core wired over it with a real store in `dir`.
fn fake_core(dir: &TempDir, catalogue: &str) -> (FakeSystem, AppCore) {
    let system = FakeSystem::new();
    let core = build_core(system.ports(), &db(dir), catalogue).expect("core builds");
    (system, core)
}

// ---- real registry, real store: the restart flow ---------------------------------------------

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
            subkey: format!(r"Software\VoltTest\EndToEnd-{}-{nanos}", std::process::id()),
        }
    }

    fn value(&self, name: &str) -> RegistryValueRef {
        value_ref(&self.subkey, name)
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        // The shell has no unsafe code, so the unique test key is removed with the system's own
        // `reg` tool. The path is rebuilt here and checked, so this can only ever name a key
        // below `HKCU\Software\VoltTest\EndToEnd-`.
        assert!(
            self.subkey.starts_with(r"Software\VoltTest\EndToEnd-"),
            "refusing to delete {}",
            self.subkey
        );
        let _ = Process::new("reg")
            .args(["delete", &format!(r"HKCU\{}", self.subkey), "/f"])
            .output();
    }
}

#[test]
fn real_registry_flow_survives_a_restart_and_reverts_exactly() {
    let key = TestKey::new();
    let registry = WinRegistry::new();
    let catalogue = flag_catalogue(&key.subkey);
    // Guard: this test must never point at the real Explorer key.
    assert!(
        !catalogue.to_lowercase().contains("explorer"),
        "the test catalogue must only target the VoltTest key"
    );
    // Arrange the "before": Flag does not exist, Pre exists with the value 5.
    registry
        .write_value(&key.value("Pre"), &RegistryValue::Dword(5))
        .expect("arrange Pre");
    assert_eq!(registry.read_value(&key.value("Flag")), Ok(None));

    let dir = TempDir::new().expect("temp dir");

    // Session one. Nothing is applied yet.
    let core = build_core(windows_ports(), &db(&dir), &catalogue).expect("core builds");
    assert_eq!(tweak_state(&core, "e2e-flag"), TweakStateView::NotApplied);

    let applied = apply_tweak(&core, "e2e-flag").expect("apply");
    assert_eq!(applied.status, ApplyStatus::Applied);
    assert_eq!(applied.state, TweakStateView::Applied);
    assert_eq!(
        registry.read_value(&key.value("Flag")),
        Ok(Some(RegistryValue::Dword(0)))
    );
    assert_eq!(
        registry.read_value(&key.value("Pre")),
        Ok(Some(RegistryValue::Dword(1)))
    );
    // Applying again changes nothing and journals nothing.
    let again = apply_tweak(&core, "e2e-flag").expect("second apply");
    assert_eq!(again.status, ApplyStatus::AlreadyApplied);

    // The restart: every in-memory object is dropped; only the database and the registry stay.
    drop(core);
    let core = build_core(windows_ports(), &db(&dir), &catalogue).expect("core builds again");
    assert!(
        core.startup
            .reconcile
            .as_ref()
            .is_ok_and(|report| report.outcomes.is_empty()),
        "a clean shutdown leaves nothing to reconcile: {:?}",
        core.startup.reconcile
    );
    assert_eq!(tweak_state(&core, "e2e-flag"), TweakStateView::Applied);

    // Undo still works after the restart, and restores the original state exactly.
    let reverted = revert_tweak(&core, "e2e-flag").expect("revert");
    assert_eq!(reverted.status, RevertStatus::Reverted);
    assert_eq!(reverted.restored, 2);
    assert_eq!(reverted.failed, 0);
    assert_eq!(reverted.state, TweakStateView::NotApplied);
    assert_eq!(
        registry.read_value(&key.value("Flag")),
        Ok(None),
        "a value that did not exist before must not exist after the revert"
    );
    assert_eq!(
        registry.read_value(&key.value("Pre")),
        Ok(Some(RegistryValue::Dword(5))),
        "a value that existed before comes back with its old data"
    );
    // Nothing left to undo.
    let nothing = revert_tweak(&core, "e2e-flag").expect("second revert");
    assert_eq!(nothing.status, RevertStatus::NothingToRevert);

    // The activity log holds the records, newest first: two reverts, then two applies.
    let page = list_activity(&core, 50, None).expect("activity");
    let statuses: Vec<_> = page.items.iter().map(|item| item.status).collect();
    assert_eq!(
        statuses,
        [
            ActivityStatus::Reverted,
            ActivityStatus::Reverted,
            ActivityStatus::Applied,
            ActivityStatus::Applied
        ]
    );
    assert_eq!(page.next_cursor, None);
    assert!(page.items.iter().all(|item| {
        item.label.as_deref() == Some("e2e-flag")
            && item.time.ends_with('Z')
            && item.id.parse::<u64>().is_ok()
    }));
    // The summary names the target and the outcome, and the redaction rule holds.
    assert!(
        page.items[2].summary.contains("Flag") || page.items[3].summary.contains("Flag"),
        "{:?}",
        page.items
    );

    // Paging: two records at a time, following the cursor.
    let first = list_activity(&core, 2, None).expect("first page");
    assert_eq!(first.items.len(), 2);
    let cursor = first.next_cursor.clone().expect("there is more");
    let second = list_activity(&core, 2, Some(&cursor)).expect("second page");
    assert_eq!(second.items.len(), 2);
    assert_eq!(second.next_cursor, None);
    assert_eq!(
        first
            .items
            .iter()
            .chain(&second.items)
            .map(|item| item.id.clone())
            .collect::<Vec<_>>(),
        page.items
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>()
    );
}

// ---- fakes: failure outcomes ---------------------------------------------------------------

#[test]
fn a_failing_step_is_rolled_back_and_explained() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, &flag_catalogue(subkey));
    system.registry.fail_on(
        &value_ref(subkey, "Pre"),
        PortError::AccessDenied("hkcu".into()),
    );

    let result = apply_tweak(&core, "e2e-flag").expect("a failed step is a result, not an error");
    assert_eq!(result.status, ApplyStatus::RolledBack);
    let detail = result.detail.expect("the reason is given");
    assert!(detail.contains("access denied"), "{detail}");
    assert_eq!(result.state, TweakStateView::NotApplied);
    assert_eq!(system.registry.get(&value_ref(subkey, "Flag")), None);

    // The history says what happened.
    let page = list_activity(&core, 10, None).expect("activity");
    let statuses: Vec<_> = page.items.iter().map(|item| item.status).collect();
    assert!(statuses.contains(&ActivityStatus::Failed), "{statuses:?}");
    assert!(
        statuses.contains(&ActivityStatus::RolledBack),
        "{statuses:?}"
    );
}

#[test]
fn a_leftover_effect_after_a_failure_needs_attention() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let catalogue = format!(
        r#"
[[tweak]]
id = "e2e-mixed"
title = "Mixed"
description = "Registry step plus a command that leaks."
category = "performance"
risk = "caution"
evidence = "measured"
evidence_note = "Test."

[[tweak.steps]]
kind = "registry_set"
hive = "HKCU"
subkey = '{subkey}'
name = "Flag"
value = {{ dword = 0 }}

[[tweak.steps]]
kind = "command"
detect = {{ program = "tool", args = ["--on"] }}
apply = {{ program = "tool", args = ["--enable"] }}
revert = {{ program = "tool", args = ["--disable"] }}
"#
    );
    let (system, core) = fake_core(&dir, &catalogue);
    let detect = Command::new("tool", ["--on"]);
    let apply = Command::new("tool", ["--enable"]);
    system.commands.add_toggle(
        detect.clone(),
        apply.clone(),
        Command::new("tool", ["--disable"]),
        false,
    );
    // The command switches its effect on and then exits non-zero: it is not atomic.
    system.commands.leak(&apply, 9);

    let result = apply_tweak(&core, "e2e-mixed").expect("report");
    assert_eq!(result.status, ApplyStatus::NeedsAttention);
    assert!(result.detail.is_some());
    assert_eq!(result.state, TweakStateView::Partial);
}

#[test]
fn a_revert_that_cannot_restore_everything_says_it_was_incomplete_and_can_be_retried() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, &flag_catalogue(subkey));
    assert_eq!(
        apply_tweak(&core, "e2e-flag").expect("apply").status,
        ApplyStatus::Applied
    );

    // Windows refuses to restore Pre.
    system.registry.fail_on(
        &value_ref(subkey, "Pre"),
        PortError::AccessDenied("hkcu".into()),
    );
    let first = revert_tweak(&core, "e2e-flag").expect("revert returns a result");
    assert_eq!(first.status, RevertStatus::Incomplete);
    assert_eq!((first.restored, first.failed), (1, 1));
    assert!(
        first
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("access denied")),
        "{:?}",
        first.detail
    );

    // The entry that failed stays applied, so once the cause is gone the undo can finish.
    system.registry.clear_failures();
    let second = revert_tweak(&core, "e2e-flag").expect("retry");
    assert_eq!(second.status, RevertStatus::Reverted);
    assert_eq!(second.state, TweakStateView::NotApplied);
}

#[test]
fn a_tweak_changed_outside_volt_has_nothing_to_revert() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let (system, core) = fake_core(&dir, &flag_catalogue(subkey));
    // Both values are already in their target state, set by something else.
    system
        .registry
        .set(&value_ref(subkey, "Flag"), RegistryValue::Dword(0));
    system
        .registry
        .set(&value_ref(subkey, "Pre"), RegistryValue::Dword(1));

    assert_eq!(tweak_state(&core, "e2e-flag"), TweakStateView::Applied);
    let result = revert_tweak(&core, "e2e-flag").expect("not an error");
    assert_eq!(result.status, RevertStatus::NothingToRevert);
    assert_eq!(result.state, TweakStateView::Applied, "nothing was touched");
}

#[test]
fn an_unknown_tweak_is_not_found_for_both_commands() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, &flag_catalogue(r"Software\VoltTest\Fake"));
    assert!(matches!(
        apply_tweak(&core, "nope"),
        Err(IpcError::NotFound { .. })
    ));
    assert!(matches!(
        revert_tweak(&core, "nope"),
        Err(IpcError::NotFound { .. })
    ));
}

/// A commands port whose every call fails, like a machine where the program cannot be run.
struct FailingCommands;

impl volt_core::Commands for FailingCommands {
    fn run(
        &self,
        _command: &volt_core::Command,
    ) -> Result<volt_core::CommandOutput, volt_core::PortError> {
        Err(volt_core::PortError::Unsupported("no commands here".into()))
    }
}

#[test]
fn a_state_that_cannot_be_read_is_listed_as_unreadable_and_cannot_be_applied() {
    let dir = TempDir::new().expect("temp dir");
    let system = FakeSystem::new();
    // A commands port that cannot run anything, so command state cannot be read.
    let ports = Ports {
        commands: Arc::new(FailingCommands),
        ..system.ports()
    };
    let catalogue = r#"
[[tweak]]
id = "needs-command"
title = "Needs a command"
description = "Detect runs an external command."
category = "performance"
risk = "safe"
evidence = "anecdotal"
evidence_note = "Test."

[[tweak.steps]]
kind = "command"
detect = { program = "tool", args = ["--on"] }
apply = { program = "tool", args = ["--enable"] }
revert = { program = "tool", args = ["--disable"] }
"#;
    let core = build_core(ports, &db(&dir), catalogue).expect("core builds");
    // One unreadable tweak does not fail the list.
    assert!(matches!(
        tweak_state(&core, "needs-command"),
        TweakStateView::Unreadable { .. }
    ));
    // And nothing is guessed when applying.
    assert!(matches!(
        apply_tweak(&core, "needs-command"),
        Err(IpcError::Unsupported { .. })
    ));
}

// ---- wiring --------------------------------------------------------------------------------

#[test]
fn the_embedded_catalogue_builds_a_core_with_its_one_tweak_listed_with_its_labels() {
    let dir = TempDir::new().expect("temp dir");
    // Fakes only: the real Explorer key is never touched by a test.
    let (_system, core) = fake_core(&dir, EMBEDDED_CATALOGUE);
    let tweaks = list_tweaks(&core).expect("list");
    assert_eq!(tweaks.len(), 1);
    let tweak = &tweaks[0];
    assert_eq!(tweak.id, "show-file-extensions");
    assert_eq!(
        tweak.state,
        TweakStateView::NotApplied,
        "the fake registry is empty"
    );
    assert_eq!(
        serde_json::to_value(tweak).expect("serializes")["risk"],
        "safe"
    );
}

#[test]
fn a_corrupt_database_is_moved_aside_and_reported() {
    let dir = TempDir::new().expect("temp dir");
    std::fs::write(db(&dir), b"this is not a database, not even close at all").expect("write");
    let (_system, core) = fake_core(&dir, &flag_catalogue(r"Software\VoltTest\Fake"));
    let recovery = core.startup.recovery.as_ref().expect("recovery reported");
    assert!(
        recovery.backup_path.exists(),
        "the old file is kept, not deleted"
    );
    // The fresh database works.
    assert_eq!(tweak_state(&core, "e2e-flag"), TweakStateView::NotApplied);
}

#[test]
fn an_invalid_catalogue_is_refused_at_build_time() {
    let dir = TempDir::new().expect("temp dir");
    let result = build_core(
        FakeSystem::new().ports(),
        &db(&dir),
        "[[tweak]]\nid = \"x\"\n",
    );
    assert!(result.is_err());
}

#[test]
fn startup_resolves_an_entry_a_crash_left_pending_and_logs_it_in_the_report() {
    let dir = TempDir::new().expect("temp dir");
    let subkey = r"Software\VoltTest\Fake";
    let target = value_ref(subkey, "Flag");
    let system = FakeSystem::new();
    // The "crash": the journal holds a pending entry, and the change did happen.
    {
        let store = Store::open(&db(&dir)).expect("store").store;
        let batch = store.new_batch(Some("e2e-flag")).expect("batch");
        store
            .record_pending(NewEntry {
                batch,
                timestamp: Timestamp::from_unix_millis(1),
                source: Source::Manual,
                operation: Operation::SetRegistryValue {
                    target: target.clone(),
                    value: RegistryValue::Dword(0),
                },
                prior: PriorState::RegistryValue(None),
            })
            .expect("pending");
    }
    system.registry.set(&target, RegistryValue::Dword(0));

    let core = build_core(system.ports(), &db(&dir), &flag_catalogue(subkey)).expect("core");
    let report = core.startup.reconcile.as_ref().expect("reconciled");
    assert_eq!(report.outcomes.len(), 1);
    assert_eq!(report.outcomes[0].resolution, Resolution::ConfirmedApplied);
    let entry = core
        .journal
        .entry(EntryId(1))
        .expect("read")
        .expect("entry exists");
    assert_eq!(entry.state, EntryState::Applied);
    // The reconciliation is in the activity history too.
    let page = list_activity(&core, 10, None).expect("activity");
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].status, ActivityStatus::Applied);
}

// ---- activity listing ----------------------------------------------------------------------

#[test]
fn the_activity_list_validates_its_cursor_and_clamps_its_limit() {
    let dir = TempDir::new().expect("temp dir");
    let (_system, core) = fake_core(&dir, &flag_catalogue(r"Software\VoltTest\Fake"));
    assert!(matches!(
        list_activity(&core, 10, Some("not a number")),
        Err(IpcError::InvalidInput { .. })
    ));
    assert_eq!(
        list_activity(&core, 0, None)
            .expect("empty log")
            .items
            .len(),
        0
    );
    apply_tweak(&core, "e2e-flag").expect("apply");
    assert_eq!(
        list_activity(&core, 0, None)
            .expect("limit 0 means 1")
            .items
            .len(),
        1
    );
    let all = list_activity(&core, u32::MAX, None).expect("huge limit is clamped");
    assert_eq!(all.items.len(), 2);
    assert_eq!(all.next_cursor, None);
}

// ---- activity write failures ---------------------------------------------------------------

#[test]
fn an_activity_log_that_cannot_write_is_logged_as_a_warning_and_never_blocks_the_change() {
    capture_warnings();
    let system = FakeSystem::new();
    let subkey = r"Software\VoltTest\Fake";
    let catalogue =
        Arc::new(volt_core::Catalogue::from_toml(&flag_catalogue(subkey)).expect("catalogue"));
    let activity = Arc::new(FakeActivityLog::new());
    activity.fail_records_with(Some(PortError::Other("disk full".into())));
    let engine = TweakEngine::new(
        catalogue,
        system.ports(),
        system.journal.clone(),
        system.clock.clone(),
    )
    .with_activity_log(activity.clone());
    let core = AppCore::new(
        engine,
        system.journal.clone(),
        Arc::new(volt_core::fakes::FakeSettings::new()),
        activity,
        StartupReport {
            store_path: Path::new("fake").to_path_buf(),
            recovery: None,
            reconcile: Ok(volt_core::ReconcileReport::default()),
        },
    );

    let result = apply_tweak(&core, "e2e-flag").expect("apply");
    assert_eq!(
        result.status,
        ApplyStatus::Applied,
        "the change is not blocked"
    );
    assert!(
        captured().iter().any(
            |line| line.contains("activity record(s) could not be written")
                && line.contains("applying a tweak")
        ),
        "the warning is in the diagnostic log: {:?}",
        captured()
    );
}
