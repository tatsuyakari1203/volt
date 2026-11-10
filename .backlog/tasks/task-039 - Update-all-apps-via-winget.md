---
id: TASK-039
title: Update all apps via winget
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - apps
milestone: m-2
dependencies:
  - TASK-038
priority: low
type: feature
ordinal: 39000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Outdated apps are a security and stability problem and winget is already present on Windows 11. Matrix row F3.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Available updates are listed from winget
- [ ] #2 Selected or all updates can be installed with progress and per-app results
- [ ] #3 A missing or broken winget is reported with a suggested fix
- [ ] #4 The UI states that this feature uses the network
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
