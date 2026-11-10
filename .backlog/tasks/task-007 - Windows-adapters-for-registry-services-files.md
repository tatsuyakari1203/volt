---
id: TASK-007
title: Windows adapters for registry, services, files
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - win
  - safety
milestone: m-0
dependencies:
  - TASK-005
priority: high
type: feature
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core's ports need real implementations before any feature can touch a machine. Confining Windows API calls and unsafe code to one crate keeps them reviewable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Registry, service and file system ports are implemented on the Windows API
- [ ] #2 Every unsafe block has a SAFETY comment and unsafe appears in no other crate
- [ ] #3 Adapter tests use only HKCU\Software\VoltTest and a temp directory, and clean up after themselves
- [ ] #4 File deletion can target the Recycle Bin and refuses to follow junctions or symlinks out of the given root
- [ ] #5 Access-denied and missing-privilege cases return typed errors
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
