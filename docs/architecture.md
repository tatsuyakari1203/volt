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
- **Activity log:** the history written for people, and the data the future Undo page reads.
  `ActivityLog` is a port (`volt_core::activity`) that appends records and lists them newest first
  with a limit and a cursor (the id of the last record seen). A record has a time from the injected
  `Clock`, a `Source` (manual or automation), an outcome (applied, reverted, failed, rolled back),
  a one-line summary, and optional links to the journal batch and entry and to the tweak id or plan
  label. It is deliberately not a view over the journal: it can hold events that are not journaled
  Operations (cleaning and memory purges will write to it), and a record outlives the journal entry
  it mentions. The journal stays the source of truth for undo. `render_text` renders records as
  stable plain-text lines (UTC ISO 8601 time, source, outcome, summary, links) for export.
  - **Who writes it:** the Executor, when given a log with the builder-style
    `Executor::with_activity_log` (`TweakEngine::with_activity_log` passes it on and makes the
    tweak id the label). Without one nothing changes. It writes one record for every operation it
    applies, fails, reverts or rolls back, and for every entry `reconcile_pending` resolves.
  - **A failing activity log never blocks a change.** The system change and its journal entry come
    first and are never conditional on the record. If a record cannot be written the failure is
    written to the diagnostic log (a warning) and counted: `Executor::activity_write_failures` is
    the number since the executor was created (clones share it), so the shell can tell the user
    that the history is incomplete.
  - **What the Undo page shows today:** the newest records, read through `list_activity`: time,
    outcome and summary, read-only. Per-row revert and export come in a later task.
  - **Source of a revert:** a rollback keeps the source of its plan; a revert requested through
    `revert_entry` or `revert_batch` is recorded as manual, because those calls do not take a
    source.
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

`volt-store` implements the journal, settings and activity log ports with SQLite through `rusqlite`
(bundled).

- The database runs in WAL mode with `synchronous = FULL`, so a pending journal entry is on disk
  before the system is changed.
- Schema migrations run through `PRAGMA user_version`. A database with a newer schema than the
  binary knows is refused, never downgraded. Version 2 added the optional label of a batch. Version
  3 added the `activity` table, which has no foreign key to the journal. Each step is logged at
  info level, and a failed step is rolled back and logged as an error.
- A corrupt database is moved aside as `<file>.corrupt-<millis>` and reported to the caller. It is
  never deleted.
- On startup the composition root opens the store, builds the Executor and calls
  `reconcile_pending`. For each entry left pending by a crash: if the system state equals the prior
  state, the change was never applied and the entry is marked Failed; if it equals the target, the
  entry is marked Applied and stays undoable; otherwise it is marked Failed as unknown.

## Composition root

`apps/desktop/src-tauri/src/wiring.rs` is where the crates meet. `build_core(ports, store_path,
catalogue_toml)` is the one function that does the wiring, and it takes everything environmental as
a parameter, so the app calls it with the real adapters and tests call the same function with
fakes, or with a throwaway registry key and a temp directory.

- **One store, three ports.** It opens the `Store` once (creating the directory first) and uses
  that one object as the `Journal`, the `Settings` and the `ActivityLog`. When the store reports a
  corruption recovery the shell logs a warning that says where the old file was kept; the file is
  never deleted and the app carries on with the fresh database (earlier changes then cannot be
  undone from Volt).
- **Executor and engine.** It builds an `Executor` and a `TweakEngine` over the `Ports`, both with
  the activity log attached, and calls `reconcile_pending` once, before anything can be applied.
  The report is logged (counts per resolution, a warning when a state is unknown or could not be
  read) and kept in `StartupReport`. A reconcile failure never stops the app: the entries stay
  pending for the next start.
- **Ports.** `windows_ports()` builds the bundle from `WinRegistry`, `WinFileSystem` and
  `WinServices`. The processes, memory and commands ports have no Windows adapter yet, so
  `unsupported.rs` supplies `UnsupportedProcesses`, `UnsupportedMemory` and `UnsupportedCommands`,
  which fail every call with `PortError::Unsupported`. A tweak with a command step would show its
  state as unreadable and refuse to apply until a real commands adapter exists.
- **Managed state.** The result is `AppState` in Tauri's managed state, not owned by the window, so
  the engine and the database connection outlive the on-demand window. If the core cannot be built
  (an unusable database, a build bug in the catalogue) the app still starts, the tray and window
  keep working, and every command answers with the same error, which the UI shows. `AppCore`
  holds a lock that serialises list, apply and revert, so two quick clicks cannot interleave.
- **The catalogue** lives in the repository at `catalogue/tweaks.toml` and is embedded into the
  binary with `include_str!` (`catalogue.rs`); `volt-core` only ever receives the text. A test loads
  the embedded text and fails the build on any problem. Today it holds exactly one tweak, Show file
  extensions (`HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced`,
  `HideFileExt` = DWORD 0): Safe, evidence Anecdotal because it is a usability and safety
  preference and not a performance gain, no restart (open Explorer windows pick it up on refresh).
  The real catalogue is a later task.
- **Where the files are.** The database is `volt.db` in the app data directory
  (`%APPDATA%\io.github.tatsuyakari1203.volt`); the diagnostic log is in the app log directory.

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
- There is no adapter for the processes, memory or commands ports yet. The composition root hands
  the executor small stand-ins that fail every call with `PortError::Unsupported` (see
  Composition root), so command tweaks only run against the fake until a real adapter is written.

## Process and elevation model

The elevation model is not decided yet. The proposal, to be confirmed by a prototype before code
depends on it: one elevated process that is both the tray agent and the Tauri host. The
alternative, an unelevated UI talking to an elevated helper over a named pipe, has a smaller
attack surface but costs an IPC protocol and a second binary. Everything below works unelevated
and does not depend on that decision, except where it says so.

### Window lifecycle and tray

Volt keeps very little memory while idle because the window and its WebView exist only while
someone is looking at them. The code is in `apps/desktop/src-tauri/src`: `window.rs`, `tray.rs`
and `autostart.rs`.

- **On-demand window.** The `main` window stays declared in `tauri.conf.json` (460 x 700, minimum
  420 x 600) with `create: false`, so Tauri does not build it at start-up. `open_main_window`
  builds it from that declaration, or focuses it (restoring it first when hidden or minimised)
  when it exists. The create-or-focus decision (`decide_open`) is a pure function. Open requests
  are serialised and run on a worker thread, because building a window from inside a tray or
  plugin callback on the main thread can deadlock on Windows. A failure to open is written to the
  diagnostic log.
- **Closing destroys the WebView.** The close button destroys the window and its WebView. Tauri
  then asks the process to exit because the last window is gone; `window::keep_running` refuses
  that request (only requests without an exit code, so Quit, which exits with code 0, still
  works). The process and the tray icon stay.
- **Tray.** One icon with the menu Open, Boost and Quit. A left click (on release) opens or
  focuses the window; the menu is on the right click. The menu id to action mapping
  (`TrayAction`) is a pure function. **Boost has no engine yet:** it opens the window, which
  starts on Home (a window that is already open is moved to Home by the UI), and emits the typed
  event `TrayBoostRequested` for a later task to consume. It calls no memory purge. When the
  window has to be created, the request flag is stored before the window is built, so the page-load
  handler cannot run first and miss it; the handler then announces it. For a window that already
  exists the open callback announces it. Whoever takes the flag announces it, so it is sent once, and
  a failed open discards it. Delivery stays best effort until a pull command exists, because the UI
  registers its listener asynchronously.
- **Single instance.** `tauri-plugin-single-instance` is registered before every other plugin. A
  second launch focuses (or recreates) the window of the first instance and exits.
- **Launch mode.** A launch with `--background` (what the Start with Windows entry passes) does
  not open a window and stays in the tray; any other launch opens the window.
- **Capabilities.** Both plugins are driven from Rust only, so no plugin permission is granted
  and the default capability is unchanged. The plugins' own JavaScript commands are therefore
  unreachable from the UI.

### Start with Windows

Optional and off by default. It is exposed as two typed commands, `autostart_enabled` and
`set_autostart` (which returns the state read back afterwards), and a switch in Settings.

- **Unelevated path only.** `tauri-plugin-autostart` writes a per-user entry under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. It needs no administrator rights and
  shows no UAC prompt. The entry carries `--background`.
- **Port and fake.** The logic sits behind the small `Autostart` trait; `set_state` only touches
  the registration when it differs from the request and reports the state read back. Unit tests
  run against an in-memory fake. Only the plugin adapter writes the Run key, and no test or
  script may call it on a developer machine.
- **If Volt runs elevated, this entry no longer works.** Windows does not start an elevated
  program from the Run key without a UAC prompt at every sign-in. An elevated build would need a
  scheduled task with highest privileges instead, created by an elevated helper; that replaces
  the adapter behind the same trait and is part of the elevation decision.
- **Known limit.** The registration is found by name, so an entry written by another build of
  Volt (for example a development build) counts as enabled.

### Idle memory

Measured on 2026-11-10 on the development machine (Windows 11, WebView2 runtime installed). Method:
`pnpm tauri build --no-bundle` (release profile, no installer); start `volt-desktop.exe`; wait 15
seconds with the window open and record the private working set (the `Working Set - Private`
performance counter) of `volt-desktop.exe` and of every descendant `msedgewebview2.exe` process; close the
window with `CloseMainWindow`; wait 30 seconds and record again. A second launch of the exe then
recreated the window and the same close and measure was repeated (10 second wait).

| State                                | `volt-desktop.exe` | WebView2 processes   | Total   |
| ------------------------------------ | ------------------ | -------------------- | ------- |
| Window open (Home), 15 s after start | 3.8 MB             | 6 processes, 81.2 MB | 85.0 MB |
| Window closed, 30 s after close      | 3.8 MB             | none                 | 3.8 MB  |
| Window recreated by a second launch  | 3.9 MB             | 6 processes, 71.9 MB | 75.8 MB |
| Closed again, 10 s after close       | 3.9 MB             | none                 | 3.9 MB  |

With the window closed no `msedgewebview2.exe` child of Volt remains, so the WebView is really
destroyed, not hidden. The measurement shares the machine with other builds and applications, so
treat the figures as an order of magnitude, not a benchmark. Memory shared between processes is
not counted in a private working set.

## Frontend

- `apps/desktop/src`: React + TypeScript + Tailwind.
- Feature folders (`features/clean`, `features/memory`, ...) on top of `shared/ui` (design system)
  and `shared/ipc` (bindings to the Rust commands). Features do not import each other; a boundary
  rule (dependency-cruiser, `apps/desktop/.dependency-cruiser.cjs`, run by `pnpm check`) enforces
  it and also forbids `shared` importing features, mirroring the crate check.
- The app shell lives in `src/app` (navigation rail, section switching, Settings, placeholders) and
  is composed by `src/App.tsx`. It sits on top of both `features` and `shared`; nothing else may
  import from it, and the same boundary rule enforces that. Navigation is plain local state, with
  no router.
- `features/settings` holds the Start with Windows switch (`StartupSettings`, built from the shared
  Switch and ListRow); `SettingsScreen` mounts it. The switch reads and writes through the typed
  commands and shows the state the shell reports back, not the state that was asked for.
- `features/tweaks` is the Tweaks screen over the real commands. It lists the catalogue with the
  live state, shows loading, empty and error states, and reads the state again whenever it is
  mounted, which includes a freshly created window. The switch applies (on) or reverts (off); it is
  disabled with a "Turning on..." or "Undoing..." line while the command runs and shows only the
  state the shell reports back. A success shows a toast with Undo (which calls revert); a rolled
  back or needs-attention apply, an incomplete revert, "nothing to undo" and a failed command are
  explained in a persistent notice in plain words, and a partly applied tweak gets an Undo button.
  The screen carries no Sample data marker; Home still does.
- `features/undo` is the Undo screen for now: a read-only list of recent activity (time, outcome
  and the redacted one-line summary) read from the activity log, newest first, with a Show older
  button that follows the cursor. Undoing a single line and exporting the list come later; a tweak
  is undone from the Tweaks screen. Features do not import each other.
- Outside the Volt window (a plain browser) the generated bindings fail with a `TypeError` because
  the Tauri bridge is missing. The wrapper turns that into `IpcUnavailableError`, and screens say
  "not available outside the Volt app" instead of showing the raw error.
- The shell listens for the typed `tray-boost-requested` event and shows Home, so the tray's Boost
  item lands on Home even when the window is already open on another section.
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
  - **Commands.** Besides `app_info` and the two autostart commands, the shell exposes
    `list_tweaks` (every catalogue tweak with its state read live; a tweak whose state cannot be
    read is listed as `unreadable` and does not fail the list), `apply_tweak(id)`,
    `revert_tweak(id)` and `list_activity(limit, cursor)`. They are async and run their work on the
    blocking thread pool, never on the main thread, because they touch the registry and `SQLite`.
    The commands are thin: the logic is in `src-tauri/src/api.rs`, which the integration tests call.
    Apply reports precisely how it ended: `applied`, `alreadyApplied`, `rolledBack` (a step failed
    and the system was read again and is as before) or `needsAttention` (it may differ), with a
    redacted reason (the error class, never command output or data) and the state read again.
    Revert reports `reverted`, `incomplete` (with counts) or `nothingToRevert` (the journal has
    nothing to undo: Volt did not apply it, or it is already undone; not an error). Activity pages
    are newest first; the limit is clamped to 1 to 200 and `nextCursor` is the cursor of the next,
    older page (or null).
  - **Events.** Events are structs deriving `tauri_specta::Event` and registered in
    `bindings_builder`; the sample is `AppReady(AppInfo)`, listened to as `events.appReady`.
    `TrayBoostRequested` (`events.trayBoostRequested`, payload `null`) is sent when the user
    picks Boost in the tray menu.
  - **64-bit integers.** `specta-typescript` refuses to export `u64`/`i64` by default because
    JavaScript numbers lose precision above 2^53. A type that exposes one fails the export (and
    so the gate) until it is given an explicit representation, for example a decimal string.
    Registry QWORD values will need that when commands expose them. The tweak and activity
    types avoid 64-bit numbers altogether: record ids are decimal strings (and so is the cursor) and
    times are ISO 8601 UTC strings.
  - **Transfer types.** Types derive `specta::Type` in the shell, which owns them (`api.rs`: risk,
    evidence, restart, category, state, tweak info, apply and revert results, activity items). The
    domain types are not exposed wholesale, so a change in the core cannot change the UI contract by
    accident, and `volt-core` stays free of the derive.
  - **Errors from the engine.** `TweakError` maps into `IpcError` keeping the kind of the port
    failure underneath (access denied stays access denied); an unknown tweak is `notFound`, and a
    store that cannot be opened is `other`.

### Design system

- **Tokens** are CSS variables in `src/index.css`. Colour and elevation tokens exist for light and
  dark (`:root` and `:root[data-theme='dark']`) and are mapped into Tailwind's theme, so classes
  such as `bg-surface`, `text-muted` and `border-line` follow the active theme. Type, radius,
  spacing and motion tokens are theme independent. Tailwind's default palette is switched off: only
  token colours exist, with one accent colour.
- **Theme** (`src/shared/theme`) follows the system by default. The user can override it in
  Settings; the choice is kept in localStorage. A `?theme=light|dark` URL parameter pins a theme
  for one session without saving it (used for screenshots). A `?section=<id>` parameter opens a
  section directly, for the same reason.
- **Contrast** is a test: `tokens.test.ts` parses the token blocks and checks every text and
  control pair against WCAG AA (4.5:1 for text, 3:1 for controls and the focus ring) in both
  themes. Adding a token to one theme without the other also fails it.
- **Motion** is short (120 and 200 ms) and collapses to nothing under `prefers-reduced-motion`.
- **Components** live in `src/shared/ui` and are imported from its index: Button, Switch, Card,
  ListGroup and ListRow, ProgressRing, Badge (risk levels), Toast with an action such as Undo,
  Dialog (focus trap, Escape, focus returns), SearchField, SegmentedControl, PageHeader and
  ErrorBoundary. They hold no business logic and use native elements where one exists.
- **Window**: 460 x 700 by default, minimum 420 x 600, resizable. Below 760 px the navigation is an
  icon rail with tooltips; at 760 px and wider it shows labels.
- Design review screenshots are kept in `docs/design`.

## Testing strategy

| Level       | What                                          | How                                                                                                                                                                                                  |
| ----------- | --------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Unit        | Core use cases, tweak engine, rule evaluation | In-memory fakes of the ports; runs on any OS                                                                                                                                                         |
| Adapter     | `volt-win`, `volt-store`                      | Real APIs against a throwaway `HKCU\Software\VoltTest` key, a temp directory and a temp database only                                                                                                |
| Integration | The shell's wiring and commands               | The real `WinRegistry` on a throwaway `HKCU\Software\VoltTest` key and a real store in a temp directory (apply, restart, revert, activity), then the same wiring over fakes for the failure outcomes |
| UI          | Components and feature state                  | Vitest, with the IPC wrapper mocked                                                                                                                                                                  |
| End to end  | A few flows through the real app              | WebDriver, added once flows exist                                                                                                                                                                    |

Tests never apply a real tweak, clean real user data or purge memory on the machine running them.
In particular no test applies or reverts the shipped "Show file extensions" tweak: the end to end
test uses its own catalogue whose every step targets one unique key below `HKCU\Software\VoltTest`,
and removes that key afterwards. The first run of the real flow on a real machine is a manual check
by the owner.

## Error handling

Errors are typed values in the core, never panics. They cross the IPC boundary as data so the UI
can explain what failed and what was (or was not) changed. `unwrap()` is denied by lint outside
tests.

## Logging

There are two logs, for two readers.

- **Activity log** (users): what Volt did, kept in the database, shown on the Undo page and
  exportable as text. See Core concepts.
- **Diagnostic log** (developers): what Volt is doing and why something went wrong. Crates emit
  records through the `log` facade (port errors by class, reconcile results, migration steps,
  activity write failures). Only the Tauri shell installs a logger: `src-tauri/src/logging.rs`
  writes `volt.log` in the app log directory with a hard size bound. The file rotates at 512 KiB
  into `volt.log.1` and `volt.log.2`, three files at most (1.5 MiB in total), and a line longer than
  a file is cut. Info level in release builds, debug in debug builds. A failure to start or write
  the log never stops the app. `tauri-plugin-log` was evaluated and not adopted: its total size
  could not be tested without a running app and it adds a plugin and dependencies, while the
  writer needed here is small and its bound is tested against a temp directory. There is no
  frontend logging.

### Redaction rule

Both logs and the text export name the **target** and the **outcome** of an event and nothing
else:

- Name the target: registry key and value name, service name, the program of a command.
- Registry values: numbers of the `DWORD` and `QWORD` kinds may be shown. For string and binary
  kinds (`REG_SZ`, `REG_EXPAND_SZ`, `REG_MULTI_SZ`, `REG_BINARY`) show only the kind and the length.
- Never include command arguments or command output.
- Never include file contents.
- Never include the text of a port error: it can carry command output or data. Show its class
  ("access denied", "not found", ...) instead.

The helpers in `volt_core::activity` (`redacted_value`, `error_class`) implement the rule, and the
summaries the Executor writes are built only from them. Code that adds a new kind of event writes
its summary the same way. Tests put a recognisable secret into a string value, a binary value, a
command argument, a command's output and a port error, and assert that it appears in no summary, in
no export and in no diagnostic log line.

The undo journal is not a log: it must hold prior values in full to restore them, so it is stored
only in the user's database and is never copied into either log.
