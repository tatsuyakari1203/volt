---
id: TASK-032
title: Disk analyzer with MFT scan and treemap
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - cleaner
  - win
  - ui
milestone: m-2
dependencies:
  - TASK-007
  - TASK-010
priority: medium
type: feature
ordinal: 32000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
Users cannot decide what to delete without seeing where space went. An MFT scan makes that take seconds instead of minutes. Matrix row C11.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An NTFS volume is indexed from the MFT; other volumes fall back to a directory walk
- [ ] #2 A treemap and a sortable folder list show sizes
- [ ] #3 A scan of the owner's system drive completes in a recorded, acceptable time
- [ ] #4 Files can be opened in Explorer or sent to the Recycle Bin from the view
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
