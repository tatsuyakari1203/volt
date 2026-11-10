---
id: TASK-018
title: Browser cache rules
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
  - safety
milestone: m-1
dependencies:
  - TASK-016
priority: medium
type: feature
ordinal: 18000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Browser caches are usually the largest safe win, but the same folders hold cookies, history and saved logins that must never be touched. Matrix row C4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Chrome, Edge, Firefox and Brave caches are found per profile
- [ ] #2 Cookies, history, sessions, saved passwords and extensions are never included; a test asserts this
- [ ] #3 A running browser is detected and the user is told what will be skipped
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
