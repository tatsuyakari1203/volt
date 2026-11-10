---
id: TASK-034
title: Memory insights and exclusions
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - memory
  - ui
milestone: m-2
dependencies:
  - TASK-015
priority: medium
type: feature
ordinal: 34000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Advanced users want to see where memory sits and protect specific processes from trimming. Matrix rows B5, B7 and B8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A chart shows in-use, standby, modified and free memory over time
- [ ] #2 Processes on the exclusion list are never trimmed
- [ ] #3 Combined page list and registry cache operations are available as separate options
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
