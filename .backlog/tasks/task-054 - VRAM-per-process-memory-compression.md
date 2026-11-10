---
id: TASK-054
title: VRAM per process, memory compression
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - memory
  - monitor
  - win
milestone: m-3
dependencies:
  - TASK-014
priority: low
type: feature
ordinal: 54000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Local AI and gaming workloads are limited by VRAM, and Windows gives little visibility into which process holds it. Matrix rows B9 and B10.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 VRAM use is shown per process on NVIDIA GPUs and, where the API allows, on others
- [ ] #2 Memory compression can be toggled, with an explanation of the trade-off
- [ ] #3 Pagefile size and usage are shown
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
