---
id: TASK-053
title: Leftover scan after uninstall
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - apps
  - cleaner
milestone: m-3
dependencies:
  - TASK-038
priority: low
type: feature
ordinal: 53000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Uninstallers leave data behind. Finding it by name matching is risky, so results need a confidence level. Matrix row F2.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 After an uninstall, remaining folders and registry keys are listed with a confidence level
- [ ] #2 Nothing below high confidence is selected by default
- [ ] #3 Removal is journaled where possible
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
