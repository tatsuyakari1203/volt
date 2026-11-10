---
id: TASK-024
title: Undo and activity page
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
  - safety
milestone: m-1
dependencies:
  - TASK-006
  - TASK-012
  - TASK-010
priority: high
type: feature
ordinal: 24000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Reversibility has to be visible to be trusted. Matrix row H2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every change is listed with time, what changed and its previous value
- [ ] #2 A single change or a whole batch can be undone from the page
- [ ] #3 Changes that can no longer be undone (files deleted past the Recycle Bin) are marked as such
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
