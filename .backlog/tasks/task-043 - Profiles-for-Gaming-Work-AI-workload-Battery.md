---
id: TASK-043
title: Profiles for Gaming, Work, AI workload, Battery
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - tweaks
  - automation
milestone: m-3
dependencies:
  - TASK-022
  - TASK-042
priority: medium
type: feature
ordinal: 43000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Different workloads want different settings, and switching by hand is what users give up on. Matrix row E10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A profile is a named set of tweak states and automation settings
- [ ] #2 Switching profiles applies and reverts only the differences, through the journal
- [ ] #3 A profile can activate automatically when a chosen app starts
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
