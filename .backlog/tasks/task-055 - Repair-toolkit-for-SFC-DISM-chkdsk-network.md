---
id: TASK-055
title: Repair toolkit for SFC, DISM, chkdsk, network
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - health
  - win
milestone: m-4
dependencies:
  - TASK-025
priority: medium
type: feature
ordinal: 55000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
These built-in tools fix real problems, but most users never run them from a console. Matrix rows G2, G4 and G5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SFC, DISM health restore and chkdsk can be started with progress and a readable result
- [ ] #2 Network reset and Windows Update repair are available with a statement of what they reset
- [ ] #3 A battery report is shown on devices with a battery
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
