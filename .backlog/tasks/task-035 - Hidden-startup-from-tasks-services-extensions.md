---
id: TASK-035
title: Hidden startup from tasks, services, extensions
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - startup
  - win
milestone: m-2
dependencies:
  - TASK-019
priority: medium
type: feature
ordinal: 35000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Much of what slows boot is not in the Run keys. Matrix row D2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Logon-triggered scheduled tasks and auto-start third-party services are listed
- [ ] #2 Microsoft and system-critical entries are marked and protected by default
- [ ] #3 Changes are journaled and reversible
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
