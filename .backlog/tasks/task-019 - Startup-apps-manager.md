---
id: TASK-019
title: Startup apps manager
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - startup
  - win
  - ui
milestone: m-1
dependencies:
  - TASK-008
  - TASK-007
  - TASK-010
priority: high
type: feature
ordinal: 19000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Disabling startup entries is one of the few optimizations with a clearly measurable effect. Matrix row D1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Entries from Run keys (HKCU and HKLM, 32 and 64 bit) and Startup folders are listed with publisher and command
- [ ] #2 Disabling uses the same mechanism Windows uses, so Task Manager shows the same state
- [ ] #3 Every change goes through the journal and can be undone
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
