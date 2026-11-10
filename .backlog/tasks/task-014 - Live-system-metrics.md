---
id: TASK-014
title: Live system metrics
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - core
  - win
  - monitor
milestone: m-1
dependencies:
  - TASK-007
priority: high
type: feature
ordinal: 14000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Home and Memory need current numbers to show what Boost changed. PC Manager shows only RAM; Volt shows CPU, RAM, GPU, VRAM and disk. Matrix row A3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 CPU, RAM, GPU utilisation, VRAM and disk usage are sampled and streamed to the UI
- [ ] #2 Sampling stops when no window is open
- [ ] #3 Machines without a discrete GPU or NVML show the remaining metrics without errors
- [ ] #4 Sampling overhead is measured and recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
