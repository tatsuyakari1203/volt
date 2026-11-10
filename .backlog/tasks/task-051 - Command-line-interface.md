---
id: TASK-051
title: Command-line interface
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cli
milestone: m-3
dependencies:
  - TASK-015
  - TASK-016
  - TASK-008
priority: medium
type: feature
ordinal: 51000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Scripting and scheduled use need a CLI, and it proves the core is independent of the UI. Matrix row H5.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A volt-cli crate exposes boost, clean (with --dry-run), tweak list, apply and revert, and undo
- [ ] #2 It uses the same core, journal and rules as the app
- [ ] #3 Output is human-readable by default and JSON on request
- [ ] #4 The crate has a layering rule and does not depend on tauri
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
