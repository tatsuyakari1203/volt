---
id: TASK-009
title: Typed IPC between UI and Rust
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - desktop
  - ui
milestone: m-0
dependencies:
  - TASK-003
priority: medium
type: feature
ordinal: 9000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Untyped invoke calls drift silently when a Rust command changes. Generated bindings turn that into a compile error and keep the UI free of hand-written transfer types.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 TypeScript bindings for commands and events are generated from Rust
- [ ] #2 Changing a command's signature without regenerating fails typecheck or CI
- [ ] #3 Errors reach the UI as typed values
- [ ] #4 The choice of tool is recorded with its pinned version (tauri-specta is still a release candidate)
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
