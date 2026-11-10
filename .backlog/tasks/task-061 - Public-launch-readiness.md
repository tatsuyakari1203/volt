---
id: TASK-061
title: Public launch readiness
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - docs
  - release
milestone: m-4
dependencies:
  - TASK-058
  - TASK-002
priority: medium
type: docs
ordinal: 61000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The repo is public from the start, but a 1.0 announcement needs the open questions (license, name) closed and docs a newcomer can follow.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The README has screenshots, install steps, a feature overview and the privacy statement
- [ ] #2 The license file is in place and the name is final
- [ ] #3 Contribution docs explain how to add a tweak or a cleaning rule
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
