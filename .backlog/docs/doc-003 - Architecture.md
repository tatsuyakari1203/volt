---
id: doc-003
title: Architecture
type: specification
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:43:00Z'
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

## Core concepts

What `volt-core` contains today:

- **Operation:** the unit of change, a declarative `#[non_exhaustive]` enum: set registry value,
  delete registry value, set service start type. It is data, not behaviour. Its apply and revert
  are crate-private and reachable only through the Executor, so no caller can change the system
  without going through the journal.
- **Executor and write-ahead journaling:** for every operation the Executor detects the prior
  state, writes a pending journal entry holding that state, applies the change, then marks the
  entry applied. A failed journal write blocks the change: nothing is mutated without a durable
  record. Revert restores the recorded prior state, including "the value did not exist".
- **Plan and Preview:** a Plan is a list of operations, each with a Preview, a Risk and an
  Evidence label. Building a plan only reads the system, so building a plan is the dry run. The
  user confirms, then the Executor applies it.
- **Ports:** the `Ports` bundle holds the registry, file system, services, processes and memory
  traits that operations and use cases may touch. The core only sees these traits; composition
  roots build the bundle from real adapters, tests build it from fakes.
- **Journal port:** the persistent undo journal. It allocates batch ids (one batch per applied
  plan), stores entries with states (pending, applied, reverted, failed) and takes time from an
  injected `Clock`, so it is deterministic in tests. Revert reads the journal, so it works after a
  restart and for whole batches.
- **Risk and evidence labels:** `Safe`, `Caution`, `Advanced`; `Measured` or `Anecdotal`. The UI
  shows both.
- **`fakes` cargo feature:** in-memory fakes of every port (`FakeSystem` wires them together
  with a shared `EventLog` that records side effects in order, so a test can prove the journal
  entry was written before the registry changed). Compiled for the crate's own tests and for
  other crates that enable the feature.
- **Not Operations:** file deletion and memory purges are deliberately not Operations, because
  they cannot be undone. They only have ports (`FileSystem`, `Memory`); cleaning shows a preview
  and prefers the Recycle Bin.

Still to be built:

- **Tweak definition:** data, not code. Each tweak has an id, title, category, risk level,
  evidence level, a reboot flag, and detect/apply/revert steps expressed as registry, service or
  command operations. The engine interprets definitions; adding a tweak should not need new Rust
  code in most cases.
- **Cleaning rule:** data as well: a root, include and exclude patterns, safety constraints (never
  follow junctions or symlinks outside the root) and a category.

## Persistence

`volt-store` implements the journal and settings ports with SQLite through `rusqlite` (bundled).

- The database runs in WAL mode with `synchronous = FULL`, so a pending journal entry is on disk
  before the system is changed.
- Schema migrations run through `PRAGMA user_version`. A database with a newer schema than the
  binary knows is refused, never downgraded.
- A corrupt database is moved aside as `<file>.corrupt-<millis>` and reported to the caller. It is
  never deleted.
- On startup the composition root opens the store, builds the Executor and calls
  `reconcile_pending`. For each entry left pending by a crash: if the system state equals the prior
  state, the change was never applied and the entry is marked Failed; if it equals the target, the
  entry is marked Applied and stays undoable; otherwise it is marked Failed as unknown.

## Process and elevation model

Proposed (to be confirmed by a spike in M0, see the decision record): one elevated process that
is both the tray agent and the Tauri host. The window and its WebView are created on demand and
destroyed when hidden, so idle memory stays low. Start-with-Windows uses a scheduled task with
highest privileges to avoid a UAC prompt at every login.

The alternative, an unelevated UI talking to an elevated helper over a named pipe, has a smaller
attack surface but costs an IPC protocol and a second binary. The spike decides.

## Frontend

- `apps/desktop/src`: React + TypeScript + Tailwind.
- Feature folders (`features/clean`, `features/memory`, ...) on top of `shared/ui` (design system)
  and `shared/ipc` (generated bindings). Features do not import each other; a boundary rule
  (dependency-cruiser, `apps/desktop/.dependency-cruiser.cjs`, run by `pnpm check`) enforces it and also
  forbids `shared` importing features, mirroring the crate check.
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
