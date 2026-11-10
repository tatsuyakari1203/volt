# Architecture

Volt follows a ports-and-adapters layout so that the rules about _what_ to change on a system are
separate from the code that _does_ it. That separation is what makes a system-modifying tool
testable: the domain can be exercised against in-memory fakes, and real Windows calls are confined
to one crate.

## Layers

```
apps/desktop (crate volt-desktop: Tauri shell + React UI)      crates/volt-cli (planned)
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

| Crate          | Responsibility                                                        | May depend on    | Must not depend on                                      |
| -------------- | --------------------------------------------------------------------- | ---------------- | ------------------------------------------------------- |
| `volt-core`    | Domain types, use cases, port traits, in-memory fakes                 | nothing internal | `tauri`, `windows`, `windows-sys`, `winreg`, `rusqlite` |
| `volt-win`     | Implements ports with the Windows API. The only crate with `unsafe`   | `volt-core`      | `tauri`, `rusqlite`                                     |
| `volt-store`   | Implements persistence ports                                          | `volt-core`      | `tauri`, `windows`, `windows-sys`, `winreg`             |
| `volt-desktop` | Tauri commands, tray, window lifecycle; wires adapters into use cases | all three        | -                                                       |
| `volt-cli`     | Command-line front end (planned)                                      | all three        | `tauri`                                                 |

`scripts/check-arch.mjs` enforces this table on direct dependencies, in the pre-commit hook, in
`pnpm check` and in CI. A workspace crate with no rule fails the check: adding a crate is an
architecture decision and should be discussed in an issue first.

## Core concepts

What `volt-core` contains today:

- **Operation:** the unit of change, a declarative `#[non_exhaustive]` enum: set registry value,
  delete registry value, set service start type, run a command. It is data, not behaviour. Its
  apply and revert are crate-private and reachable only through the Executor, so no caller can
  change the system without going through the journal.
- **Commands port:** a `Command` is a program and an argument list, never a shell string. A
  command operation carries three commands: detect (exit `0` = in effect, `1` = not, anything else
  is a read error), apply and revert. The revert command is mandatory, because everything Volt
  applies must be undoable. A detect command must not change anything; Volt cannot enforce that,
  so a catalogue reviewer has to. If the effect was already in place before apply, revert leaves it
  alone.
- **Executor and write-ahead journaling:** for every operation the Executor detects the prior
  state, writes a pending journal entry holding that state, applies the change, then marks the
  entry applied. A failed journal write blocks the change: nothing is mutated without a durable
  record. Revert restores the recorded prior state, including "the value did not exist".
- **Plan and Preview:** a Plan is a list of operations, each with a Preview, a Risk and an
  Evidence label. Building a plan only reads the system, so building a plan is the dry run. The
  user confirms, then the Executor applies it.
- **Ports:** the `Ports` bundle holds the registry, file system, services, processes, memory and
  commands traits that operations and use cases may touch. The core only sees these traits; composition
  roots build the bundle from real adapters, tests build it from fakes.
- **Journal port:** the persistent undo journal. It allocates batch ids (one batch per applied
  plan), stores entries with states (pending, applied, reverted, failed) and takes time from an
  injected `Clock`, so it is deterministic in tests. Revert reads the journal, so it works after a
  restart and for whole batches. A batch can carry a label (a plan's `label`, which the tweak
  engine sets to the tweak id), and `batches_with_label` finds a label's batches again.
- **Risk and evidence labels:** `Safe`, `Caution`, `Advanced`; `Measured` or `Anecdotal`. The UI
  shows both.
- **`fakes` cargo feature:** in-memory fakes of every port (`FakeSystem` wires them together
  with a shared `EventLog` that records side effects in order, so a test can prove the journal
  entry was written before the registry changed). Compiled for the crate's own tests and for
  other crates that enable the feature.
- **Not Operations:** file deletion and memory purges are deliberately not Operations, because
  they cannot be undone. They only have ports (`FileSystem`, `Memory`); cleaning shows a preview
  and prefers the Recycle Bin.

- **Tweak definition:** data, not code (`volt_core::tweak`). A `Tweak` has an id, title,
  description, category, risk level, evidence level with a short evidence note, a restart
  requirement (none, sign-out, reboot) and an ordered list of steps: registry set, registry
  delete, service start type or command. Each step maps onto one Operation, so apply and revert
  always run through the Executor. Adding a tweak needs no new Rust code in most cases.
  - **Catalogue:** `Catalogue::from_toml` loads a set of tweaks from TOML text; the core never
    reads files. Loading validates everything and rejects the catalogue as a whole, reporting all
    problems at once, each naming the tweak id and the field path (duplicate or malformed ids,
    empty step lists, empty or malformed registry subkeys, a command without a revert, unknown
    enum values, unknown or missing fields, two steps changing the same thing). The format is
    documented on `Catalogue::from_toml`.
  - **Tweak engine:** `TweakEngine` reads the state of a tweak (Applied when every step is in its
    target state, NotApplied when none is, Partial otherwise; read-only, so it also reports tweaks
    changed outside Volt, and a state that cannot be read is an error, never a guess), previews it
    (`plan`, the dry run) and applies it. Apply builds a Plan from the steps not already in their
    target state, labels it with the tweak id and runs it through `apply_or_rollback`. When a step
    fails the report says whether the system was read again and found back as it was
    (`RolledBack`) or needs attention (`NeedsAttention`), with the executor's per-step results.
  - **Revert** finds the journal batches labelled with the tweak id that still have applied
    entries and reverts them newest first, so it works after a restart. A change made outside Volt
    has no recorded prior state and cannot be reverted by Volt; neither can steps that were already
    in their target state when the tweak was applied, because Volt did not change them.

Still to be built:

- **Cleaning rule:** data as well: a root, include and exclude patterns, safety constraints (never
  follow junctions or symlinks outside the root) and a category.

## Persistence

`volt-store` implements the journal and settings ports with SQLite through `rusqlite` (bundled).

- The database runs in WAL mode with `synchronous = FULL`, so a pending journal entry is on disk
  before the system is changed.
- Schema migrations run through `PRAGMA user_version`. A database with a newer schema than the
  binary knows is refused, never downgraded. Version 2 added the optional label of a batch.
- A corrupt database is moved aside as `<file>.corrupt-<millis>` and reported to the caller. It is
  never deleted.
- On startup the composition root opens the store, builds the Executor and calls
  `reconcile_pending`. For each entry left pending by a crash: if the system state equals the prior
  state, the change was never applied and the entry is marked Failed; if it equals the target, the
  entry is marked Applied and stays undoable; otherwise it is marked Failed as unknown.

## Windows adapters

`volt-win` implements the registry, services and file system ports with the Windows API:
`WinRegistry`, `WinServices` and `WinFileSystem`.

- File deletion goes to the Recycle Bin through `IFileOperation`. There is no permanent-delete
  fallback: if the Recycle Bin cannot take a file, the deletion fails.
- `move_to_trash` takes a root. It refuses paths outside that root and anything whose path
  traverses a reparse point (junction or symlink).
- Tests are confined to `HKCU\Software\VoltTest` and temp directories.
- Changing a real service's start type is deliberately not covered by an automated test, because
  it would change the machine running the tests.
- There is no adapter for the Commands port yet, so command tweaks only run against the fake until
  one is written.

## Process and elevation model

Not decided yet. The proposal, to be confirmed by a prototype before code depends on it: one
elevated process that is both the tray agent and the Tauri host. The window and its WebView are
created on demand and destroyed when hidden, so idle memory stays low. Start-with-Windows uses a
scheduled task with highest privileges to avoid a UAC prompt at every login.

The alternative, an unelevated UI talking to an elevated helper over a named pipe, has a smaller
attack surface but costs an IPC protocol and a second binary.

## Frontend

- `apps/desktop/src`: React + TypeScript + Tailwind.
- Feature folders (`features/clean`, `features/memory`, ...) on top of `shared/ui` (design system)
  and `shared/ipc` (bindings to the Rust commands). Features do not import each other; a boundary
  rule (dependency-cruiser, `apps/desktop/.dependency-cruiser.cjs`, run by `pnpm check`) enforces
  it and also forbids `shared` importing features, mirroring the crate check.
- The UI holds no business rules: it renders plans and results produced by the core.
- **Typed IPC.** The Rust shell registers every command, event and transfer type in one place
  (`apps/desktop/src-tauri/src/ipc.rs`, `bindings_builder`). `tauri-specta` generates the invoke
  handler and `apps/desktop/src/shared/ipc/bindings.ts` from that registry. The generated file is
  committed and excluded from Biome. The UI imports only from `shared/ipc` (the generated module
  plus a thin wrapper in `index.ts`); it never calls `invoke` directly or hand-writes a transfer
  type. `pnpm bindings` regenerates the file; the `bindings` step of `pnpm check` (and CI)
  regenerates it in memory and fails when the committed file differs, so a changed Rust signature
  cannot reach the UI unnoticed.
  - **Pinned versions:** `tauri-specta =2.0.0-rc.21`, `specta =2.0.0-rc.22`,
    `specta-typescript =0.0.9`. They are release candidates, so they are pinned exactly and move
    together. Newer candidates (`rc.24` to `rc.26`) were tried and do not compile on the pinned
    Rust 1.89 (they use `fmt::from_fn` and a const `TypeId::of`, stabilised in later Rust);
    revisit when the toolchain is bumped.
  - **Errors.** Every command returns `Result<T, IpcError>`. `IpcError` is a serialisable enum
    tagged by `kind` (`notFound`, `accessDenied`, `unsupported`, `invalidInput`, `other`), built
    from `volt_core::PortError`. The UI receives `{ status: "error", error: { kind, message } }`
    and switches on `kind`; the wrapper throws `IpcCommandError` carrying the typed value.
  - **Events.** Events are structs deriving `tauri_specta::Event` and registered in
    `bindings_builder`; the sample is `AppReady(AppInfo)`, listened to as `events.appReady`.
  - **64-bit integers.** `specta-typescript` refuses to export `u64`/`i64` by default because
    JavaScript numbers lose precision above 2^53. A type that exposes one fails the export (and
    so the gate) until it is given an explicit representation, for example a decimal string.
    Registry QWORD values will need that when commands expose them.
  - **Transfer types.** Types derive `specta::Type` in the shell for now. `volt-core` stays free
    of the derive until commands expose its domain model.

## Testing strategy

| Level      | What                                          | How                                                                                                   |
| ---------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------- |
| Unit       | Core use cases, tweak engine, rule evaluation | In-memory fakes of the ports; runs on any OS                                                          |
| Adapter    | `volt-win`, `volt-store`                      | Real APIs against a throwaway `HKCU\Software\VoltTest` key, a temp directory and a temp database only |
| UI         | Components and feature state                  | Vitest                                                                                                |
| End to end | A few flows through the real app              | WebDriver, added once flows exist                                                                     |

Tests never apply a real tweak, clean real user data or purge memory on the machine running them.

## Error handling

Errors are typed values in the core, never panics. They cross the IPC boundary as data so the UI
can explain what failed and what was (or was not) changed. `unwrap()` is denied by lint outside
tests.
