---
id: TASK-044
title: Tweak drift detection, import and export
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - tweaks
milestone: m-3
dependencies:
  - TASK-022
priority: medium
type: feature
ordinal: 44000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Windows updates silently reset settings. Users should be told, and should be able to carry their configuration to another machine. Matrix rows E11 and E12.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Tweaks that were applied by Volt and have since changed are detected and listed
- [ ] #2 The user can reapply them or accept the new state
- [ ] #3 Configuration exports to a file and imports with a preview of what will change
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
