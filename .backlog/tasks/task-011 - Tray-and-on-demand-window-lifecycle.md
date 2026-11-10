---
id: TASK-011
title: Tray and on-demand window lifecycle
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - desktop
milestone: m-0
dependencies:
  - TASK-003
  - TASK-004
priority: medium
type: feature
ordinal: 11000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An optimizer that itself holds a hundred megabytes while idle undermines its purpose. The window and WebView should exist only while the user is looking at them.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Closing the window keeps a tray icon; the WebView is destroyed when hidden
- [ ] #2 Idle memory with the window closed is measured and recorded
- [ ] #3 Only one instance runs; starting a second focuses the first
- [ ] #4 The tray menu offers Open, Boost and Quit
- [ ] #5 Start with Windows is optional and does not show a UAC prompt
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
