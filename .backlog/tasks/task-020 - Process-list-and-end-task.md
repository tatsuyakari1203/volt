---
id: TASK-020
title: Process list and end task
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - process
  - win
  - ui
milestone: m-1
dependencies:
  - TASK-007
  - TASK-014
  - TASK-010
priority: medium
type: feature
ordinal: 20000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users need to see what is using memory and CPU right now and stop it. Matrix row D4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Processes are shown grouped by app with CPU, memory and, where available, GPU use
- [ ] #2 Ending a task asks for confirmation and warns for system-critical processes
- [ ] #3 The list updates without visible jank at a few hundred processes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
