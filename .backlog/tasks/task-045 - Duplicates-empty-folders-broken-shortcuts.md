---
id: TASK-045
title: Duplicates, empty folders, broken shortcuts
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
milestone: m-3
dependencies:
  - TASK-032
priority: low
type: feature
ordinal: 45000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Common requests that PC Manager partly covers. Matrix rows C13 and C14.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Duplicates are found by size, then partial hash, then full hash
- [ ] #2 The user chooses which copy to keep; nothing is preselected in system or program folders
- [ ] #3 Empty folders and shortcuts with missing targets are listed separately
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
