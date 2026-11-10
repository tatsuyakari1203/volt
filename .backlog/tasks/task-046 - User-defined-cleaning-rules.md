---
id: TASK-046
title: User-defined cleaning rules
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
  - safety
milestone: m-3
dependencies:
  - TASK-016
priority: low
type: feature
ordinal: 46000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No built-in rule set covers every app. Letting users add rules also makes community contributions easy. Matrix row C15.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Rules can be added from a TOML file or a simple form
- [ ] #2 User rules go through the same safety constraints as built-in ones
- [ ] #3 Roots such as a drive root, the Windows folder or a user profile root are rejected
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
