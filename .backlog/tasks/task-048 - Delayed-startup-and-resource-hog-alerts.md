---
id: TASK-048
title: Delayed startup and resource hog alerts
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - startup
  - process
  - automation
milestone: m-3
dependencies:
  - TASK-019
  - TASK-042
priority: low
type: feature
ordinal: 48000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Some programs are wanted at startup but not in the first minute, and runaway processes usually go unnoticed. Matrix rows D3 and D7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A startup entry can be delayed by a chosen time instead of disabled
- [ ] #2 A process that sustains unusual CPU, memory or disk use triggers a notification with actions
- [ ] #3 Alert thresholds and a mute list are configurable
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
