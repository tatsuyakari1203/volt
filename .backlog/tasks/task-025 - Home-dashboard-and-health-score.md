---
id: TASK-025
title: Home dashboard and health score
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
  - health
milestone: m-1
dependencies:
  - TASK-014
  - TASK-010
priority: high
type: feature
ordinal: 25000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Home is the first screen and has to answer how the PC is doing and what to do about it. Matrix rows A2 and G1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A 0 to 100 score is computed from explainable rules such as free disk space, startup load, memory pressure and pending reboots
- [ ] #2 Each deduction is listed with a reason and a fix action
- [ ] #3 Four live metric cards are shown
- [ ] #4 The score never changes without a visible reason
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
