---
id: TASK-005
title: Core domain model and port traits
status: In Progress
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:16:00Z'
labels:
  - core
  - safety
milestone: m-0
dependencies:
  - TASK-001
priority: high
type: feature
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Everything Volt does is a system change that must be previewed, applied and reverted. Without a shared model for that, each feature would invent its own and undo would be unreliable. The ports keep the core testable without Windows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 The core defines an operation with detect, apply and revert, a plan made of operations with a preview, and risk and evidence labels
- [x] #2 Port traits exist for registry, file system, services, processes, memory and journal, each with an in-memory fake
- [x] #3 Errors are typed and never panic; a failed apply reports what was and was not changed
- [x] #4 Unit tests cover plan, apply and revert against the fakes and run without Windows APIs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [x] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [x] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Cargo: add serde (derive) and thiserror to [workspace.dependencies]; volt-core gets them plus a `fakes` feature (off by default, also compiled under cfg(test)).
2. Model: PortError, Risk/Evidence, Timestamp/Clock, registry types (hive, view, value ref, value enum), ServiceStartType, journal types (EntryId, BatchId, Source, EntryState, NewEntry, JournalEntry).
3. Ports (sync, object-safe, Send + Sync): Registry, FileSystem, Services, Processes, Memory, Journal; Ports bundle of Arcs.
4. Operation (non_exhaustive enum: SetRegistryValue, DeleteRegistryValue, SetServiceStartType) with public detect and crate-private apply/revert so only the executor can mutate; PriorState records the exact prior state, including "value did not exist".
5. Plan: items with preview (current -> target), risk and evidence; plan risk = max. Plan::build only reads (this is the dry run).
6. Executor enforces write-ahead: detect -> journal pending (with prior) -> apply -> mark applied; failure marks failed. Stops at first failure; ApplyReport lists applied/failed/not attempted; optional rollback; revert from the journal by entry or batch.
7. In-memory fakes for all ports behind `fakes`, sharing an EventLog so tests can prove ordering; failure injection.
8. Tests: plan/preview, apply, mid-plan failure, rollback, revert of previously-missing value, journal-before-mutation ordering, journal failure blocks mutation, serde round trip.
9. pnpm check, record notes, commit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/volt-core (modules error, label, time, registry, service, ports, journal, operation, plan, executor, fakes).
- Operation is a #[non_exhaustive] declarative enum (SetRegistryValue, DeleteRegistryValue, SetServiceStartType). detect() is public and read-only; apply()/revert() are pub(crate), so only the Executor can mutate, and it always journals first.
- Write-ahead order enforced in Executor::apply_item: detect -> Journal::record_pending (with prior state) -> apply -> set_state(Applied). If the journal write fails the system is not touched (tested). Apply failure marks the entry Failed.
- Apply stops at the first failure; ApplyReport lists applied / failed / not attempted per item. ExecError::system_changed() distinguishes "applied but journal update failed" (system IS changed, entry stays Pending, still rolled back by rollback()). apply_or_rollback() reverts applied items newest first; revert_entry / revert_batch work from the journal alone.
- Revert restores PriorState exactly, including "value did not exist" (revert deletes it). Deleting an absent value is a journaled no-op.
- Plan::build is read-only (the dry run) and gives each item a Preview "subject: current -> target", a Risk and an Evidence; Plan::risk() is the max. The executor detects again at apply time, so a stale preview cannot corrupt the journal.
- Fakes for all six ports plus ManualClock and FakeSystem (wires them to one EventLog) behind the `fakes` feature, always compiled for the crate's own tests. The EventLog is what proves journal-before-mutation ordering.
- Dependencies: serde (derive) and thiserror in [workspace.dependencies]; serde_json added as a dev-dependency only, for JSON round-trip tests of the persisted/IPC types (not in the brief; justified because the journal and IPC rely on the serde representation, and internally/adjacently tagged enums are easy to get wrong).
- Known limits, left for later tasks: set-registry-value creates missing keys but revert does not remove keys it created; u64 (REG_QWORD) values exceed JS safe-integer range over JSON IPC; the Journal port has no listing/pruning beyond batch_entries; crash recovery of Pending entries is not implemented.
- Evidence: cargo test -p volt-core: 25 passed; pnpm check: all 5 gates passed (rustfmt, clippy -D warnings, tests, architecture, Biome); clippy also clean with --features fakes.
- DoD #4 left open on purpose: the brief forbids editing backlog documents. Architecture doc delta: the "Core concepts" section should say Operation is a declarative enum whose apply/revert are only reachable through the Executor, name the Ports bundle and the EventLog-based fakes, and mention the `fakes` feature.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the core model, the six port traits with in-memory fakes, and a write-ahead-journaling Executor in volt-core. All acceptance criteria are verified by 25 passing unit tests. Open: DoD #4 (Architecture doc delta, to be applied by the coordinator).
<!-- SECTION:FINAL_SUMMARY:END -->
