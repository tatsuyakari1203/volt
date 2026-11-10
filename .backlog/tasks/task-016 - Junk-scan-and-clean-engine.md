---
id: TASK-016
title: Junk scan and clean engine
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
  - core
  - safety
milestone: m-1
dependencies:
  - TASK-005
  - TASK-007
priority: high
type: feature
ordinal: 16000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Deleting files is the riskiest thing Volt does. One engine with strict rules (preview first, Recycle Bin by default, never leave the rule's root) is safer than per-rule code, and makes rules data that can be reviewed.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Cleaning rules are data: root, include and exclude patterns, category and constraints
- [ ] #2 A scan lists files and sizes without changing anything; cleaning needs a confirmed plan
- [ ] #3 Junctions and symlinks are never followed out of a rule's root
- [ ] #4 Locked or in-use files are skipped and reported
- [ ] #5 The result states the space actually freed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
