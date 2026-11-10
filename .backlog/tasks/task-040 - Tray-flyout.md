---
id: TASK-040
title: Tray flyout
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - desktop
  - ui
milestone: m-2
dependencies:
  - TASK-011
  - TASK-026
priority: low
type: feature
ordinal: 40000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Most interactions are a glance and a Boost; they should not need the full window. Matrix row A4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Clicking the tray icon opens a small panel with memory, CPU and a Boost button
- [ ] #2 The panel closes on focus loss and does not keep a WebView alive afterwards
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
