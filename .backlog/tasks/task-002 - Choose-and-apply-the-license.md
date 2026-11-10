---
id: TASK-002
title: Choose and apply the license
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - docs
  - release
milestone: m-0
dependencies: []
priority: high
type: task
ordinal: 2000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A public repository without a license grants nobody the right to use or contribute to it. The license decision record proposes GPL-3.0-or-later and is waiting for the owner.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The owner has confirmed a license and the decision record is marked accepted
- [ ] #2 A LICENSE file exists and the license field is set in Cargo.toml and package.json
- [ ] #3 The README license section is updated
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
