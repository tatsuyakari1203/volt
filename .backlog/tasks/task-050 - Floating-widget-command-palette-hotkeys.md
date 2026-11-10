---
id: TASK-050
title: Floating widget, command palette, hotkeys
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
  - desktop
milestone: m-3
dependencies:
  - TASK-014
  - TASK-026
priority: low
type: feature
ordinal: 50000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Speed-of-use features for people who keep Volt running. Matrix rows A5 and H3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An optional always-on-top widget shows chosen metrics
- [ ] #2 Ctrl+K opens a palette that reaches every action
- [ ] #3 A global hotkey for Boost is configurable and off by default
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
