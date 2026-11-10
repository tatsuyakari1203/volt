---
id: TASK-057
title: Code signing decision and setup
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - release
milestone: m-4
dependencies:
  - TASK-028
priority: high
type: task
ordinal: 57000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
An elevated unsigned app triggers SmartScreen and antivirus suspicion. Signing costs money, so the owner has to choose between a certificate, Azure Artifact Signing or shipping unsigned.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 Options and current costs are documented for the owner
- [ ] #2 The owner's decision is recorded as a decision
- [ ] #3 If signing is chosen, release builds are signed in CI
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
