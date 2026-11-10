---
id: TASK-047
title: Services manager with safe presets
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - startup
  - win
  - tweaks
milestone: m-3
dependencies:
  - TASK-035
priority: medium
type: feature
ordinal: 47000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Service tuning is popular and easy to get wrong. Presets with clear risk labels are safer than lists copied from forums. Matrix row D6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Services are listed with state, start type and a plain-language description
- [ ] #2 Presets change only services with documented, low-risk effects
- [ ] #3 Every change is journaled, and a dependency check warns before disabling a service others need
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
