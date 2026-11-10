---
id: TASK-003
title: Scaffold the Tauri 2 desktop shell
status: Done
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:19:00Z'
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
- [x] #1 `pnpm tauri dev` opens a window from apps/desktop on Windows 11
- [x] #2 The crate is named volt-desktop, is a workspace member and passes scripts/check-arch.mjs
- [x] #3 Frontend typecheck, Biome and Vitest run as part of `pnpm check` and in CI
- [x] #4 A frontend boundary rule stops feature folders importing each other, mirroring the crate check
- [x] #5 Tauri, React, Vite, Tailwind and TypeScript versions are pinned and the Resources document is updated
- [x] #6 The Rust toolchain pin is raised only if a dependency requires it, and the reason is noted
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [x] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [x] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [x] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Scaffold apps/desktop (@volt/desktop): Vite 8, React 19, TS 6, Tailwind 4, Vitest; folders src/features, src/shared/ui, src/shared/ipc.
2. Add Rust crate apps/desktop/src-tauri (volt-desktop, Tauri 2.11.x, workspace lints) with one `app_info` command; minimum capabilities, no plugins.
3. Add dependency-cruiser boundary rule; prove it fails on violations, then remove them.
4. Wire typecheck, Vitest and boundaries into scripts/check.ps1 (and so CI); keep Biome ignoring generated output.
5. Verify `pnpm tauri dev` opens a window; update Resources doc (doc-005) with pinned versions.
(Recorded after the scaffold was written, not before, contrary to the loop order.)
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Versions: tauri 2.11.6, tauri-build 2.6.3, @tauri-apps/cli 2.11.5, @tauri-apps/api 2.11.1, React 19.3.1, Vite 8.3.7, Tailwind 4.3.5, TypeScript 6.0.3, Vitest 5.1.0, dependency-cruiser 18.7.2. All pinned exactly.
Toolchain: NOT raised. tauri 2.12.x and tauri-build 2.7.x declare rust-version 1.90, but tauri 2.11.6 and tauri-build 2.6.3 build and pass clippy on 1.88.0, so the pin stays (AC #6).
TypeScript is 6.0.3 rather than 7.x because dependency-cruiser 18.7.2 refuses TS >= 7 (it warned it would miss TS sources).
Gate change: scripts/check.ps1 gained three steps (typecheck, vitest, ui-boundaries); CI runs them through `pnpm check`, ci.yml only got a longer job name. .gitignore ignores src-tauri/gen/.
Boundary proof: with features/app-info importing features/other and shared importing a feature, `pnpm --filter @volt/desktop boundaries` reported 2 errors and exited 2; violations removed, rule passes.
AC #1 evidence: `pnpm tauri dev` started in the background; process volt-desktop appeared with MainWindowTitle "Volt", window about 476x739 px outer size; the UI process tree was then killed (only the processes started here).
Capabilities: empty permission list for the main window; invoke of the app's own command needs none. CSP is default-src 'self'.
Icons: a placeholder yellow disc generated from icons/source.png via `tauri icon`; replace in the design task.
Not verified: cargo-deny and typos (not installed locally; run in CI), CI itself on windows-latest, the rendered UI text (only the window existence was checked, the component is covered by a Vitest test with a mocked ipc).
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added the Tauri 2 desktop shell: apps/desktop (@volt/desktop, React 19 + Vite + Tailwind 4) and crate volt-desktop in the cargo workspace, with one `app_info` command shown on a minimal screen. `pnpm check` now also runs TypeScript typecheck, Vitest and a dependency-cruiser boundary rule (features cannot import each other, shared cannot import features). `pnpm tauri dev` opens the Volt window. Rust toolchain stays at 1.88 (Tauri pinned to 2.11.6). DoD #3 is met vacuously: nothing here modifies the system.
<!-- SECTION:FINAL_SUMMARY:END -->
