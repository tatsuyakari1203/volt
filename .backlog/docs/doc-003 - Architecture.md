---
id: doc-003
title: Architecture
type: specification
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:01:00Z'
---
Volt follows a ports-and-adapters layout so that the rules about *what* to change on a system are
separate from the code that *does* it. That separation is what makes a system-modifying tool
testable: the domain can be exercised against in-memory fakes, and real Windows calls are confined
to one crate.

## Layers

```
apps/desktop (crate volt-desktop: Tauri shell + React UI)      crates/volt-cli (later)
                 |                                                  |
                 +----------------------+---------------------------+
                                        |           composition roots
                 +----------------------+---------------------------+
                 |                                                  |
        crates/volt-win                                    crates/volt-store
        Windows adapters                                   SQLite adapters
        (registry, services, processes,                    (undo journal, settings,
         memory lists, file system)                         activity log)
                 |                                                  |
                 +----------------------+---------------------------+
                                        |
                                crates/volt-core
                        domain model, use cases, port traits
```

| Crate | Responsibility | May depend on | Must not depend on |
|---|---|---|---|
| `volt-core` | Domain types, use cases, port traits, in-memory fakes | nothing internal | `tauri`, `windows`, `windows-sys`, `winreg`, `rusqlite` |
| `volt-win` | Implements ports with the Windows API. The only crate with `unsafe` | `volt-core` | `tauri`, `rusqlite` |
| `volt-store` | Implements persistence ports | `volt-core` | `tauri`, `windows`, `windows-sys`, `winreg` |
| `volt-desktop` | Tauri commands, tray, window lifecycle; wires adapters into use cases | all three | - |
| `volt-cli` | Command-line front end (M3) | all three | `tauri` |

`scripts/check-arch.mjs` enforces this table on direct dependencies, in the pre-commit hook, in
`pnpm check` and in CI. A workspace crate with no rule fails the check: adding a crate is an
architecture decision and needs a decision record.

## Core concepts (to be built in M0)

- **Operation:** the unit of change. It has `detect` (read-only: what is the current state?),
  `apply` and `revert`. Use cases produce a **plan** (a list of operations with a preview), the
  user confirms, then the plan is applied.
- **Ports:** traits such as registry, file system, services, processes, memory and journal. The
  core only sees these traits.
- **Undo journal:** before an operation is applied, its previous state is persisted. Revert reads
  the journal, so it works after a restart and for batches.
- **Tweak definition:** data, not code. Each tweak has an id, title, category, risk level,
  evidence level, a reboot flag, and detect/apply/revert steps expressed as registry, service or
  command operations. The engine interprets definitions; adding a tweak should not need new Rust
  code in most cases.
- **Cleaning rule:** data as well: a root, include and exclude patterns, safety constraints (never
  follow junctions or symlinks outside the root) and a category.
- **Risk and evidence labels:** `Safe`, `Caution`, `Advanced`; `Measured` or `Anecdotal`. The UI
  shows both.

## Process and elevation model

Proposed (to be confirmed by a spike in M0, see the decision record): one elevated process that
is both the tray agent and the Tauri host. The window and its WebView are created on demand and
destroyed when hidden, so idle memory stays low. Start-with-Windows uses a scheduled task with
highest privileges to avoid a UAC prompt at every login.

The alternative, an unelevated UI talking to an elevated helper over a named pipe, has a smaller
attack surface but costs an IPC protocol and a second binary. The spike decides.

## Frontend (when scaffolded)

- `apps/desktop/src`: React + TypeScript + Tailwind.
- Feature folders (`features/clean`, `features/memory`, ...) on top of `shared/ui` (design system)
  and `shared/ipc` (generated bindings). Features do not import each other; a boundary rule
  (dependency-cruiser or equivalent) will enforce it, mirroring the crate check.
- The UI holds no business rules: it renders plans and results produced by the core.
- IPC types are generated from Rust so a breaking change fails the TypeScript build.

## Testing strategy

| Level | What | How |
|---|---|---|
| Unit | Core use cases, tweak engine, rule evaluation | In-memory fakes of the ports; runs on any OS |
| Adapter | `volt-win`, `volt-store` | Real APIs against a throwaway `HKCU\Software\VoltTest` key, a temp directory and a temp database only |
| UI | Components and feature state | Vitest |
| End to end | A few flows through the real app | WebDriver (the Tauri docs recommend `@wdio/tauri-service`), added once flows exist |

Tests never apply a real tweak, clean real user data or purge memory on the machine running them.

## Error handling

Errors are typed values in the core, never panics. They cross the IPC boundary as data so the UI
can explain what failed and what was (or was not) changed. `unwrap()` is denied by lint outside
tests.
