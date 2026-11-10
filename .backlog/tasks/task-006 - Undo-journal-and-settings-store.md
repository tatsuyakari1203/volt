---
id: TASK-006
title: Undo journal and settings store
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
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
- [ ] #1 The previous state of every applied operation is persisted before the change is made
- [ ] #2 A single entry or a whole batch can be reverted, including after an app restart
- [ ] #3 A crash between the journal write and the apply leaves an entry that is detected and reconciled on next start
- [ ] #4 Schema changes are migrated; a corrupt database is reported and backed up, not silently replaced
- [ ] #5 User settings are stored through the same crate
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
