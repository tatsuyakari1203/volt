---
id: TASK-027
title: Offline and no-telemetry guarantee
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - safety
  - docs
milestone: m-1
dependencies:
  - TASK-003
priority: medium
type: task
ordinal: 27000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
No telemetry is a stated principle and a reason to choose Volt over closed tools. It should be enforced and documented, not just intended. Matrix row H6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The app makes no network requests in M1 and the Tauri capability configuration grants no network access
- [ ] #2 A check in CI or tests fails if a network permission is added without updating the privacy statement
- [ ] #3 The README states what Volt does and does not send
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
