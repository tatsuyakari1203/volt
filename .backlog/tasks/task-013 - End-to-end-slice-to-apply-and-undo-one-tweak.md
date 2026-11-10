---
id: TASK-013
title: End-to-end slice to apply and undo one tweak
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - desktop
  - tweaks
  - safety
milestone: m-0
dependencies:
  - TASK-008
  - TASK-007
  - TASK-009
  - TASK-010
priority: high
type: feature
ordinal: 13000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
M0 is done when the layers work together, not when each exists. One real tweak through UI, IPC, engine, adapter and journal proves the architecture before M1 builds on it.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A Safe sample tweak can be detected, applied and undone from the UI on a real machine
- [ ] #2 State survives an app restart and Undo still works
- [ ] #3 The flow is covered by an automated test at the highest level practical
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
