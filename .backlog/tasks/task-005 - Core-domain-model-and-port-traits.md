---
id: TASK-005
title: Core domain model and port traits
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - core
  - safety
milestone: m-0
dependencies:
  - TASK-001
priority: high
type: feature
ordinal: 5000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Everything Volt does is a system change that must be previewed, applied and reverted. Without a shared model for that, each feature would invent its own and undo would be unreliable. The ports keep the core testable without Windows.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The core defines an operation with detect, apply and revert, a plan made of operations with a preview, and risk and evidence labels
- [ ] #2 Port traits exist for registry, file system, services, processes, memory and journal, each with an in-memory fake
- [ ] #3 Errors are typed and never panic; a failed apply reports what was and was not changed
- [ ] #4 Unit tests cover plan, apply and revert against the fakes and run without Windows APIs
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
