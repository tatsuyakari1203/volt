---
id: TASK-017
title: Core cleaning rules
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
milestone: m-1
dependencies:
  - TASK-016
priority: high
type: feature
ordinal: 17000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The baseline every cleaner covers and PC Manager already offers. Matrix rows C1 to C3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Rules cover user and system temp, WER reports, crash dumps, logs, thumbnail cache, Windows Update cache, Delivery Optimization cache and the Recycle Bin
- [ ] #2 Windows Update cache cleaning stops and restarts the service safely
- [ ] #3 Temp files newer than a configurable age are left alone
- [ ] #4 Each rule has a test with a fixture tree
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
