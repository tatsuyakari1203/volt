---
id: TASK-049
title: Disk health with SMART, temperature, TRIM
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - health
  - win
milestone: m-3
dependencies:
  - TASK-025
priority: medium
type: feature
ordinal: 49000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A failing or full SSD is the health problem that matters most, and Windows hides it well. Matrix row G3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 SMART attributes, temperature and wear are shown for SATA and NVMe drives
- [ ] #2 Warnings feed the Home health score
- [ ] #3 TRIM or optimize can be run per volume
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
