---
id: TASK-059
title: Release automation and changelog
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - ci
  - release
milestone: m-4
dependencies:
  - TASK-028
priority: medium
type: chore
ordinal: 59000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Manual releases get skipped. Conventional Commits were adopted so the changelog and version bump can be generated.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 A release is cut from a tag or a release pull request without manual steps
- [ ] #2 The changelog is generated from Conventional Commits
- [ ] #3 Cargo and package.json versions stay in sync
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
