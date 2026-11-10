---
id: TASK-038
title: App uninstaller with batch removal
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - apps
  - win
  - ui
milestone: m-2
dependencies:
  - TASK-007
  - TASK-010
priority: medium
type: feature
ordinal: 38000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Removing software is a core cleanup task and PC Manager offers it. Matrix row F1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Win32 and UWP apps are listed with size, publisher and install date
- [ ] #2 Several apps can be queued and uninstalled in sequence
- [ ] #3 System components are hidden by default
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
