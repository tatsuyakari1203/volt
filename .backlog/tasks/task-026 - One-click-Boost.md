---
id: TASK-026
title: One-click Boost
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
  - memory
  - cleaner
milestone: m-1
dependencies:
  - TASK-015
  - TASK-017
  - TASK-025
priority: high
type: feature
ordinal: 26000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The single action most users will ever press. Matrix row A1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Boost runs the default memory operations and temp cleaning with one click
- [ ] #2 A before and after report shows memory and disk space freed
- [ ] #3 What Boost includes is configurable
- [ ] #4 It is also available from the tray menu
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
