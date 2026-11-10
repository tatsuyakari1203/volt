---
id: TASK-004
title: Spike on elevation and process model
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - spike
  - desktop
  - safety
milestone: m-0
dependencies:
  - TASK-003
priority: high
type: spike
ordinal: 4000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Volt needs administrator rights for most operations, but running a WebView elevated has costs (drag and drop limits, a larger attack surface) and a UAC prompt at every start is unacceptable. The decision record proposes a single elevated process; this spike tests that against a split UI plus elevated helper before code depends on either.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Both models are prototyped far enough to compare: start-up without a UAC prompt through a highest-privileges scheduled task, idle memory with the window destroyed, and WebView2 behaviour when elevated
- [ ] #2 Measurements from the owner's machine are recorded in the task
- [ ] #3 The process-model decision record is marked accepted or replaced, with consequences listed
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
