---
id: TASK-021
title: Restore point before risky changes
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - win
  - safety
milestone: m-1
dependencies:
  - TASK-007
priority: medium
type: feature
ordinal: 21000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The undo journal covers what Volt changed, but a Windows restore point is the safety net users already trust for Caution and Advanced tweaks. Matrix row E13.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A restore point is created before the first Caution or Advanced tweak in a session
- [ ] #2 If System Restore is off, the user is told and can continue or cancel
- [ ] #3 Windows' rate limit on restore points is handled without failing the tweak
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
