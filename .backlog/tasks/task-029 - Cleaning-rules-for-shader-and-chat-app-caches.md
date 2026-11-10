---
id: TASK-029
title: Cleaning rules for shader and chat app caches
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
milestone: m-2
dependencies:
  - TASK-016
priority: medium
type: feature
ordinal: 29000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Shader caches and chat apps (Zalo in particular for Vietnamese users) grow to many gigabytes and generic cleaners ignore them. Matrix rows C5 and C6.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 DirectX, NVIDIA and AMD shader caches are covered
- [ ] #2 Zalo, Discord, Telegram and Teams caches are covered without touching messages, files the user saved or login state
- [ ] #3 Each rule states what the app will need to download or rebuild again
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
