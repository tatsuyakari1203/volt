---
id: TASK-052
title: Debloat preinstalled apps and OneDrive
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - tweaks
  - apps
milestone: m-3
dependencies:
  - TASK-038
priority: medium
type: feature
ordinal: 52000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Removing preinstalled apps is one of the most requested tweaks and one of the easiest to overdo. Matrix row E8.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Preinstalled UWP apps are listed with a recommendation and can be removed in bulk
- [ ] #2 Removal states whether the app can be reinstalled from the Store
- [ ] #3 OneDrive and Edge background components are separate, clearly labelled options
- [ ] #4 Nothing required by Windows itself is offered
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
