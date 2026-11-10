---
id: TASK-060
title: Default apps, taskbar repair, pop-up blocking
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - apps
  - spike
milestone: m-4
dependencies:
  - TASK-038
priority: low
type: spike
ordinal: 60000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Parity items from PC Manager with uncertain feasibility on current Windows builds. Matrix rows F4 and H8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Default app associations can be reviewed and reset where Windows allows it
- [ ] #2 A spike documents whether app pop-up ads can be blocked reliably and safely; the feature ships only if the answer is yes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
