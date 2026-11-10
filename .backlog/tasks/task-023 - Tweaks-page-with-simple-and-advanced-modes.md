---
id: TASK-023
title: Tweaks page with simple and advanced modes
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
  - tweaks
milestone: m-1
dependencies:
  - TASK-022
  - TASK-010
priority: medium
type: feature
ordinal: 23000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The same catalogue has to serve someone who wants three switches and someone who wants to read every registry key.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Simple mode shows grouped switches and only Safe tweaks
- [ ] #2 Advanced mode shows every tweak with the keys, services or commands it changes
- [ ] #3 Current state is detected on open, including tweaks changed outside Volt
- [ ] #4 Search and category filters work
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
