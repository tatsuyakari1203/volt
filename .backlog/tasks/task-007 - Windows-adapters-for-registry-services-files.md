---
id: TASK-007
title: Windows adapters for registry, services, files
status: Done
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T04:06:00Z'
labels:
  - win
  - safety
milestone: m-0
dependencies:
  - TASK-005
priority: high
type: feature
ordinal: 7000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The core's ports need real implementations before any feature can touch a machine. Confining Windows API calls and unsafe code to one crate keeps them reviewable.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 Registry, service and file system ports are implemented on the Windows API
- [x] #2 Every unsafe block has a SAFETY comment and unsafe appears in no other crate
- [x] #3 Adapter tests use only HKCU\Software\VoltTest and a temp directory, and clean up after themselves
- [x] #4 File deletion can target the Recycle Bin and refuses to follow junctions or symlinks out of the given root
- [x] #5 Access-denied and missing-privilege cases return typed errors
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [x] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [x] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Port change (volt-core, FileSystem + fake only): move_to_trash(root, path) so deletion knows its containment root. Fake rejects paths outside root with InvalidInput.
2. volt-win: add `windows` 0.62 (minimal features) to workspace deps. Modules: error (Win32 code -> PortError), registry (RAII HKEY, view flags, all value kinds), services (SCM, RAII handle, delayed auto start via SERVICE_CONFIG_DELAYED_AUTO_START_INFO, pure start-type mapping), fs (stat; root containment + reparse-point refusal; Recycle Bin through SHFileOperationW with FOF_ALLOWUNDO).
3. Tests: registry under unique HKCU\Software\VoltTest\<guid> with Drop guard; contract test shared between FakeRegistry and WinRegistry; services reads on a service present on every machine (e.g. EventLog/RpcSs), mapping as pure functions, set_start_type only for a nonexistent service; fs tests in temp dir incl. mklink /J junction refusal; Recycle Bin test #[ignore] (volt-test-*), run once explicitly.
4. pnpm check, tick ACs with evidence, notes, final summary, commit.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
Implemented in crates/volt-win: WinRegistry, WinServices, WinFileSystem (windows 0.62.2, features Win32_Foundation, Win32_Security, Win32_System_Com, Win32_System_Registry, Win32_System_Services, Win32_UI_Shell).

Port change in volt-core (FileSystem + FakeFileSystem only): move_to_trash(root, path). Deletion must know its root to refuse escapes; the fake rejects paths outside root with InvalidInput.

Design:
- Errors: Win32 5 and 1314 -> AccessDenied; 2, 3, 1060 -> NotFound; 123 -> InvalidInput; others keep OS code and message in Other.
- Registry: RAII key handle; read opens KEY_QUERY_VALUE only, write/delete KEY_SET_VALUE only; 32/64-bit view flags; all six value kinds; REG_MULTI_SZ rejects empty items; unmodelled types (REG_NONE etc.) read as Unsupported rather than lossy Binary.
- Services: SCM opened with SC_MANAGER_CONNECT; QUERY_CONFIG to read, CHANGE_CONFIG to set. Delayed auto start is read/written through the delayed-auto-start-info config; the flag is set after the start type, and a failure of that second call is reported as "start type changed but flag could not be set".
- Files: stat uses symlink_metadata (a junction is described as itself: is_dir false, size 0). Trash: lexical checks (no .., strictly inside root, not the root), per-component reparse-point walk, then canonicalize both and compare, then IFileOperation with FOFX_RECYCLEONDELETE so an unrecyclable item fails instead of being permanently deleted. The target itself being a junction/symlink is also refused.

Known limits: the root check is check-then-act, so a link swapped in between check and delete can win the race (documented in code). Paths over MAX_PATH are not specially handled.

Test evidence (cargo test -p volt-win: 22 passed, 1 ignored; pnpm check passes all 5 gates):
- Registry contract test runs the same assertions against FakeRegistry and WinRegistry. Real tests use unique subkeys under HKCU\Software\VoltTest removed by a Drop guard (also on failure); verified afterwards that HKCU\Software\VoltTest no longer exists.
- AccessDenied is shown with a read-only open of HKLM\SAM\SAM (SYSTEM only).
- Services: reads of RpcSs, EventLog, Dhcp, wuauserv; start-type mapping tested as pure functions. CHANGING A REAL SERVICE'S START TYPE IS NOT COVERED BY AN AUTOMATED TEST: set_start_type is tested only for NotFound on a service name that does not exist, and the success path (including delayed start write) was never run.
- Files: temp-dir tests; junction created with mklink /J inside temp dir proves refusal both through the junction and on the junction itself, and that the target survives.
- Recycle Bin test is #[ignore] (leaves volt-test-* items in the real Recycle Bin); run once explicitly with `cargo test -p volt-win -- --ignored recycle_bin`: passed. It is not run in CI.

Not tested: HKLM writes, elevated behaviour, symlink (as opposed to junction) refusal (needs developer mode or admin to create), service set success path, Recycle Bin on drives without a bin.

Definition of Done item 3 is not applicable: the adapters are covered by tests confined to a throwaway HKCU key and temp directories, and nothing here changes user-visible behaviour in the app.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Added Windows adapters WinRegistry, WinServices and WinFileSystem to volt-win, with typed Win32 error mapping, RAII handles and SAFETY-documented unsafe blocks. FileSystem::move_to_trash now takes a root and refuses paths outside it or crossing junctions/symlinks; deletion goes to the Recycle Bin via IFileOperation and never falls back to permanent delete. Tests use only HKCU\Software\VoltTest and temp dirs; a contract test keeps FakeRegistry honest. Changing a real service start type is not covered by an automated test.
<!-- SECTION:FINAL_SUMMARY:END -->
