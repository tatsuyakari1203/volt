---
id: TASK-028
title: MVP release build v0.1
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - release
  - ci
milestone: m-1
dependencies:
  - TASK-026
  - TASK-019
  - TASK-020
  - TASK-023
  - TASK-024
  - TASK-018
  - TASK-027
priority: high
type: task
ordinal: 28000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
M1 ends with something the owner can install and use daily in place of PC Manager, so that problems show up in real use.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A tagged build produces an installer artifact in CI
- [ ] #2 The unsigned build and the SmartScreen prompt are documented in the README
- [ ] #3 The owner has used the build on their own machine and issues found are filed as tasks
- [ ] #4 The Feature Matrix marks every P0 row as shipped or re-prioritised
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
