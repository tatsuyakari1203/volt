---
id: TASK-008
title: Declarative tweak engine
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - core
  - tweaks
  - safety
milestone: m-0
dependencies:
  - TASK-005
  - TASK-006
priority: high
type: feature
ordinal: 8000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A catalogue of dozens of tweaks is only maintainable and auditable if a tweak is data that states exactly what it changes and how to undo it, instead of hand-written code per tweak.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A tweak is defined as data: id, title, category, risk, evidence, reboot flag and detect, apply and revert steps for registry, service and command kinds
- [ ] #2 The engine reports current state, applies and reverts through the journal
- [ ] #3 Invalid definitions are rejected at load time with an error naming the tweak and the field
- [ ] #4 A partially applied tweak is rolled back or reported precisely
- [ ] #5 One sample tweak is covered end to end against fakes
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
