---
id: TASK-003
title: Scaffold the Tauri 2 desktop shell
status: To Do
assignee: []
created_date: '2026-11-10T03:01:00Z'
labels:
  - desktop
  - ui
  - devflow
milestone: m-0
dependencies:
  - TASK-001
priority: high
type: feature
ordinal: 3000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The repo has library skeletons but nothing that launches. Every UI and IPC task is blocked until a Tauri 2 app exists in apps/desktop and is part of the cargo and pnpm workspaces under the same gates as the rest of the repo.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 `pnpm tauri dev` opens a window from apps/desktop on Windows 11
- [ ] #2 The crate is named volt-desktop, is a workspace member and passes scripts/check-arch.mjs
- [ ] #3 Frontend typecheck, Biome and Vitest run as part of `pnpm check` and in CI
- [ ] #4 A frontend boundary rule stops feature folders importing each other, mirroring the crate check
- [ ] #5 Tauri, React, Vite, Tailwind and TypeScript versions are pinned and the Resources document is updated
- [ ] #6 The Rust toolchain pin is raised only if a dependency requires it, and the reason is noted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->
