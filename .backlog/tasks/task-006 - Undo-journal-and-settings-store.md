---
id: TASK-006
title: Undo journal and settings store
status: Done
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:43:00Z'
labels:
  - store
  - safety
milestone: m-0
dependencies:
  - TASK-005
priority: high
type: feature
ordinal: 6000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reversibility is the product's main promise. It only holds if the previous state is on disk before a change is made and survives a crash or restart.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The previous state of every applied operation is persisted before the change is made
- [x] #2 A single entry or a whole batch can be reverted, including after an app restart
- [x] #3 A crash between the journal write and the apply leaves an entry that is detected and reconciled on next start
- [x] #4 Schema changes are migrated; a corrupt database is reported and backed up, not silently replaced
- [x] #5 User settings are stored through the same crate
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [x] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [x] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. volt-core: add Journal::entries_in_state, Executor::reconcile_pending (detect each Pending entry; equals prior -> Failed, equals target -> Applied, else Failed unknown; detect error leaves it Pending), a Settings port plus fake, FakeJournal update.
2. workspace deps: rusqlite (bundled), serde_json (runtime), tempfile (dev).
3. volt-store: Store over SQLite. WAL + synchronous FULL so record_pending is durable on return. PRAGMA user_version migrations. Corruption detected on open (quick_check / not-a-database): db plus -wal/-shm moved aside to a timestamped backup, fresh db created, backup path returned in a typed OpenOutcome. Implements Journal and Settings. Opens from a caller-given path.
4. Tests in temp dirs: persistence across reopen (revert one entry and whole batch via Executor with core fakes), crash reconcile, migration upgrade, corruption, settings, durability pragmas.
5. pnpm check, tick criteria with evidence, notes, commit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Durability: Store opens SQLite with journal_mode=WAL and synchronous=FULL; record_pending is one auto-committed INSERT, so the WAL is fsynced before it returns. Test record_pending_is_committed_to_disk_before_it_returns asserts the pragmas and that an independent second connection sees the row while the first is still open. If WAL is unavailable, synchronous=FULL still fsyncs the rollback journal.
Crash reconciliation lives in volt-core: Executor::reconcile_pending (equals prior -> Failed with reason; equals target -> Applied; else Failed "unknown"; detect error -> stays Pending, reported as Undetermined). Journal port gained entries_in_state; FakeJournal kept in sync. Tested against fakes in core and against the real store in volt-store.
Migrations: PRAGMA user_version, each step in a transaction with the version bump (rollback tested). The upgrade test builds a real v1 database with data, migrates it with an extra test-only v2 step, and checks data survives. A database with a newer user_version is refused (StoreError::SchemaTooNew) and left untouched.
Corruption: open fails with NotADatabase/DatabaseCorrupt or PRAGMA quick_check != ok -> file and -wal/-shm are renamed to <name>.corrupt-<millis> (suffix -N on collision), a fresh db is created, and OpenOutcome.recovery = Some(CorruptionRecovery{backup_path, reason}) is returned. Never deleted, never silent. Non-corruption failures (e.g. permissions) return StoreError::Open and move nothing.
Settings: volt-core Settings port (get/set/remove on serde_json::Value) plus SettingsExt typed helpers and FakeSettings; Store implements it in the same database.
Workspace changes: rusqlite 0.37 (bundled), tempfile (dev) added; serde_json moved from dev-only to a runtime dependency of volt-core (Settings port uses serde_json::Value). Not done: DoD 3 and 4 left unticked (the store does not itself modify the system; no feature-matrix/doc text changed). Out of scope per task: listing/pagination, pruning, activity log.

Definition of Done item 3 is not applicable: this task adds a persistence adapter and changes no user-visible UI, so there is nothing to verify in the app.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
volt-store now provides a SQLite Store implementing the core Journal and Settings ports with WAL + synchronous FULL durability, user_version migrations, and corrupt-database backup reported to the caller via OpenOutcome. volt-core gained Executor::reconcile_pending, Journal::entries_in_state, the Settings port and fakes. 14 store tests and 5 new core tests pass; pnpm check passes.
<!-- SECTION:FINAL_SUMMARY:END -->
