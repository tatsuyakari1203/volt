---
id: TASK-058
title: Installer and auto-update
status: To Do
assignee: []
created_date: '2026-11-10T03:02:00Z'
labels:
  - release
  - desktop
milestone: m-4
dependencies:
  - TASK-028
  - TASK-057
priority: high
type: feature
ordinal: 58000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
A public release needs a normal install and update path. Updates are also how fixes for a system-modifying tool reach users quickly. Matrix row H7.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 An installer is produced for x64
- [ ] #2 Updates are signed with the Tauri updater key and verified before install
- [ ] #3 Update checks are the only network call by default, are stated in Settings and can be turned off
- [ ] #4 The private key is stored as a CI secret, never in the repo
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
