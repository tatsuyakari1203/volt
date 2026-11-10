---
id: TASK-012
title: Logging and activity log
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - core
  - store
milestone: m-0
dependencies:
  - TASK-006
priority: medium
type: feature
ordinal: 12000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
When a system-changing tool misbehaves, the user and the developer need to see exactly what it did. The same record feeds the Undo page.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Every applied, reverted and failed operation is written to an activity log with time, source and outcome
- [ ] #2 Diagnostic logs rotate and stay under a size limit
- [ ] #3 Logs never contain file contents or credentials
- [ ] #4 The activity log can be exported as text
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
