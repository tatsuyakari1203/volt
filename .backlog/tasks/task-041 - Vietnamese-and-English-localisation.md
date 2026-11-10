---
id: TASK-041
title: Vietnamese and English localisation
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - ui
milestone: m-2
dependencies:
  - TASK-010
priority: medium
type: feature
ordinal: 41000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The owner and the first users are Vietnamese; the project is public and English-first. Matrix row H4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 All UI strings come from locale files; none are hard-coded
- [ ] #2 English and Vietnamese are complete, including tweak titles and descriptions
- [ ] #3 The language follows the system by default and can be changed in Settings
- [ ] #4 A check fails when a key is missing in either locale
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
