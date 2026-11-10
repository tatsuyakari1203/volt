---
id: TASK-036
title: Process priority, affinity, Efficiency mode
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - process
  - win
milestone: m-2
dependencies:
  - TASK-020
priority: low
type: feature
ordinal: 36000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Per-process rules that persist across restarts are something users currently install a separate tool for. Matrix row D5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Priority, affinity and Efficiency mode can be set per process
- [ ] #2 A rule can be saved so it is reapplied whenever the process starts
- [ ] #3 Saved rules are listed and removable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
