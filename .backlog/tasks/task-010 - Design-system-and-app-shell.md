---
id: TASK-010
title: Design system and app shell
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:02:00Z'
labels:
  - ui
milestone: m-0
dependencies:
  - TASK-003
priority: high
type: feature
ordinal: 10000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The owner asked for a UI that is attractive, simple and easy to use, and wants to approve the look before features are built on it. A token-based design system stops each screen from drifting. Matrix row H1.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Colour, type, spacing and motion tokens exist for light and dark themes
- [ ] #2 The shell has the sidebar (Home, Clean, Memory, Tweaks, Apps and Startup, Health, Settings, Undo) and a compact default window of about 460 x 700 that resizes
- [ ] #3 Core components exist: button, switch, card, list row, progress ring, toast with Undo, dialog
- [ ] #4 Everything is keyboard navigable with visible focus
- [ ] #5 The owner has approved a mock of Home and one list screen; screenshots are attached to the task
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
