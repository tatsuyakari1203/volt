---
id: TASK-042
title: Automation with schedules and thresholds
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - automation
  - desktop
milestone: m-3
dependencies:
  - TASK-011
  - TASK-015
  - TASK-016
priority: high
type: feature
ordinal: 42000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The goal of this milestone is that Volt looks after the machine without being opened. Matrix rows A6, B6 and C16.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Memory cleaning can trigger on thresholds (standby above X and free below Y) with a cooldown
- [ ] #2 Cleaning can run on a schedule using a saved selection of rules
- [ ] #3 Automatic runs are logged and undoable like manual ones
- [ ] #4 Automation pauses while a fullscreen app is active unless a profile says otherwise
- [ ] #5 The idle CPU and memory cost of the background agent is measured
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
