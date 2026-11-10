---
id: TASK-031
title: Reclaim component store, Windows.old, hiberfil
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
  - win
milestone: m-2
dependencies:
  - TASK-016
priority: medium
type: feature
ordinal: 31000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These are the largest single items on many system drives, but each has a real cost (no rollback, no hibernation) that must be stated. Matrix rows C9 and C10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Component store cleanup runs through DISM with progress
- [ ] #2 Windows.old removal warns that rollback to the previous version is lost
- [ ] #3 Disabling hibernation states the effect on Fast Startup and can be undone
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
