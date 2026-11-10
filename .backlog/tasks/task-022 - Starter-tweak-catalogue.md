---
id: TASK-022
title: Starter tweak catalogue
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - tweaks
milestone: m-1
dependencies:
  - TASK-008
  - TASK-021
priority: high
type: feature
ordinal: 22000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The engine is useless without content. The first catalogue covers the performance, privacy and appearance tweaks people most often apply by hand or with scripts, each with an honest label. Matrix rows E1, E3 and E4.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 About 30 tweaks across Performance, Privacy and Appearance are defined as data
- [ ] #2 Every tweak has a risk level, an evidence note and a tested revert
- [ ] #3 Tweaks that need a reboot or sign-out say so
- [ ] #4 Each tweak is verified on Windows 11 build 26200 and the result recorded
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
