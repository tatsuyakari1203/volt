---
id: TASK-015
title: Memory cleaning engine
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - memory
  - win
  - core
milestone: m-1
dependencies:
  - TASK-007
priority: high
type: feature
ordinal: 15000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Deep memory cleaning is the main advantage over PC Manager, which only trims working sets. The standby and modified lists are where memory goes on machines that load large games or AI models. Matrix rows B1 to B4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Working-set trim, standby purge (including priority 0), modified-list flush and file-cache trim are separate, individually selectable operations
- [ ] #2 Each run reports memory by list before and after
- [ ] #3 Missing privileges or an unsupported build produce a typed error, never a crash
- [ ] #4 The NT API constants are verified against a maintained reference and on Windows 11 build 26200
- [ ] #5 The UI text states when cleaning helps and when it does not
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
