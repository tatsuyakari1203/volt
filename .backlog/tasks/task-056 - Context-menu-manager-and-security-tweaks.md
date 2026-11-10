---
id: TASK-056
title: Context menu manager and security tweaks
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - tweaks
  - safety
milestone: m-4
dependencies:
  - TASK-022
priority: low
type: feature
ordinal: 56000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Advanced customisation with real downsides if misused, so it ships last and behind clear labels. Matrix rows E7 and E9.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Right-click menu entries can be listed and toggled individually
- [ ] #2 Security tweaks are labelled Advanced, explain the protection being removed and need explicit confirmation
- [ ] #3 All changes are reversible
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
