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
  engine sets to the tweak id), `batches_with_label` finds a label's batches again and
  `batch_label` reads the label of one batch. A revert that is not told the label (one entry or one
  batch picked from the Undo page) asks the journal, so its activity records carry the tweak id
  like the apply did.
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
  - **What the Undo page shows:** the records read through `list_activity`, newest first, each
    joined to its journal entry. See Undo page below.
  - **Source of a revert:** a rollback keeps the source of its plan; a revert requested through
    `revert_entry` or `revert_batch` is recorded as manual, because those calls do not take a
    source.
- **Live metrics:** the `MetricsSource` port (`volt_core::metrics`) reads one _raw sample_: the
  cumulative CPU time counters, the physical memory totals, the space of every fixed volume and one
  reading per GPU adapter. A raw sample means little alone (a CPU counter only grows), so
  `MetricsTracker` keeps the previous sample and turns each new one into a `MetricsSnapshot`: CPU
  load from the difference of two samples (`None` for the first sample and when the counters did
  not advance or went backwards), memory in use and available, used and free space per disk, and
  the primary GPU (the adapter with the most dedicated video memory). **GPU, GPU utilisation and
  video memory are optional by type**: a machine without a readable GPU has an empty adapter list
  and `gpu: None`, which is a normal case, never an error. The port is deliberately **not** part
  of the `Ports` bundle: it is read-only, independent of the executor, and the sampler is built
  with its own source. `FakeMetrics` (in `volt_core::metrics::fake`, behind the `fakes` feature)
  answers a scripted sequence of samples and errors.
- **Risk and evidence labels:** `Safe`, `Caution`, `Advanced`; `Measured` or `Anecdotal`. The UI
  shows both.
- **`fakes` cargo feature:** in-memory fakes of every port (`FakeSystem` wires them together
  with a shared `EventLog` that records side effects in order, so a test can prove the journal
  entry was written before the registry changed). Compiled for the crate's own tests and for
  other crates that enable the feature.
- **Not Operations:** file deletion and memory purges are deliberately not Operations, because
  they cannot be undone. They only have ports (`FileSystem`, `Memory`); cleaning shows a preview
  and goes to the Recycle Bin (see the clean engine below).

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

- **Cleaning rule and the clean engine:** data as well, and one engine (`volt_core::clean`) for all
  file removal, because deleting files is the riskiest thing Volt does. Cleaning is not an
  Operation: it cannot be reverted through the journal (the Recycle Bin is the way back), and its
  ports are not in the `Ports` bundle: `CleanEngine::new` takes the rules, a `KnownFolders` port,
  the `FileSystem` port and a `Clock`, and `with_activity_log` attaches the activity log.
  - **Rules are data.** `CleaningRules::from_toml` loads `[[rule]]` tables with the same
    strictness as the tweak catalogue: the whole file is rejected and every problem is reported
    with the rule id and the field path (`RuleIssue`). A rule has an id, title, description,
    category, risk, a root, `include` patterns (at least one), optional `exclude` patterns and
    constraints: `min_age_days`, `max_depth` (at most 32) and `remove_empty_dirs`. Patterns are
    relative to the root, `/` separated, ignore case and support `*`, `?` and `**` as a whole
    component; `..`, a leading `/`, a drive letter, `\` and character classes are refused. Two
    optional fields describe the rule's environment: `requires_admin` (the location needs
    administrator rights) and `min_age_setting` (the settings key that may override the minimum
    age, see below). The real rule set is `catalogue/cleaning.toml` (see Shipped cleaning rules).
  - **Roots are never paths.** A root is one of a closed set of named `Location`s (`user_temp`,
    `local_app_data`, `roaming_app_data`, `program_data`, `windows`) plus a relative path. The
    `KnownFolders` port resolves a location, so the core never reads the environment. Refused at
    load: an absolute path or drive, `..`, and an empty path for any location but `user_temp`.
    Refused at resolution, on every scan and again before every clean: a root that is not
    absolute, a drive root or a top-level folder (depth below two), and a root that equals or lies
    above a protected folder (the profile, the Windows directory, Program Files, the user's
    documents, and so on, compared ignoring case).
  - **Scan, plan, confirm, clean.** `scan` is read-only and returns a `CleanPlan`: per rule the
    items (files, and empty folders when the rule removes them, children before parents) with
    size and modification time, totals, exact counts and a capped list of what was skipped and
    why (link, too new, unknown age, beyond the depth limit, access denied, unreadable). A scan
    carries the rule's `requires_admin` and the minimum age it applied. A scan never follows a
    junction, symlink or other reparse point (a root that is one is refused), never lists a
    folder outside the root and never plans a path outside it. The caller shows the plan and calls
    `CleanPlan::confirm` (or `confirm_rules`); only the resulting `ConfirmedPlan` can be passed
    to `clean`. Plans cannot be built or deserialised outside the crate.
  - **Clean re-checks everything.** It takes the plan, never a rule, and touches only the paths
    in it. Before each item it re-resolves the root and requires it to be the planned folder, then
    checks that the path is inside the root, that the rule still allows it (include, exclude,
    depth), that every folder on the way is still a plain folder, that the item still exists, is
    not a link and has the same kind, size and modification time as in the scan, and that a
    folder is still empty. Anything else is skipped and reported with a reason. The move goes
    through `FileSystem::move_to_trash`, which has its own containment checks. There is no
    permanent delete and no retry: a locked or in-use file is reported as such and left alone.
  - **An honest result.** `CleanReport` states what was moved (files, empty folders, bytes) and
    what was skipped with reasons. It deliberately has no "freed" figure: the files sit in the
    Recycle Bin and keep their space until it is emptied, which Volt never does. `describe()`
    says so, and UI text must say "moved to the Recycle Bin".
  - **Activity.** With a log attached, one record per rule (outcome applied, or failed when
    nothing could be moved) names the rule id and counts only, following the redaction rule: no
    path, no file name, no port message. A failing log is counted in the report and never blocks
    a clean.
  - **Administrator rights are typed, not guessed.** A rule flagged `requires_admin` is still
    scanned. When Windows refuses the root (or a folder on the way to it) the scan fails with
    `ScanFailure::RootAccessDenied`; when it refuses something inside, the entry is skipped as
    `SkipReason::AccessDenied` and counted in `ScanCounts::access_denied`.
    `RuleScan::needs_administrator()` is true for a flagged rule with either, so the UI can say
    "this needs administrator rights" instead of showing an empty result as "nothing to clean". A
    missing root is not a refusal. The flag is the rule's own statement; how the rights are
    obtained is the undecided elevation model (see Process and elevation model).
  - **Minimum age and the setting that overrides it.** Every rule states `min_age_days` (0 for
    caches whose age means nothing). A rule with `min_age_setting` takes the age from that key of
    the `Settings` port when the engine was given one (`CleanEngine::with_settings`): a whole
    number of days from 1 to 3650 replaces the rule's age (it may lower it, never to 0); no value
    keeps the rule's age; an unreadable or out-of-range value fails that rule's scan with
    `ScanFailure::MinAgeSettingUnreadable` or `MinAgeSettingInvalid`, because falling back to a
    smaller age than the user chose could take files they meant to keep. `RuleScan::min_age_days`
    reports the age that was applied. The scan is the only place the age is judged: an entry newer
    than it is never listed, and a clean only moves what the plan lists.
  - **Shipped cleaning rules** (`catalogue/cleaning.toml`). Per user, no administrator rights:
    `user-temp` (the temp folder, 7 days, configurable), `user-wer-archive` and `user-wer-queue`
    (Windows Error Reporting, 7 days), `user-crash-dumps` (`*.dmp` in `CrashDumps`, 7 days),
    `thumbnail-cache` and `icon-cache` (`thumbcache_*.db` and `iconcache_*.db` in the Explorer
    folder, no minimum age; Explorer keeps some open, and those are reported as in use). Machine
    wide, `requires_admin`: `windows-temp` (7 days, configurable, never `CBS`), `machine-wer-archive`
    and `machine-wer-queue`, `windows-update-logs` (`*.etl`) and `delivery-optimization-cache`
    (risk Caution: the service owns it). Every description says what the files are, why removing
    them is safe and what the user notices. **Deliberately not rules:** the Windows Update
    download cache (clearing it needs the update service stopped and started again, which needs
    elevation) and emptying the Recycle Bin (a permanent delete, which will be a separate
    explicit action on the Cleaner page). A location is left out when there is doubt.
  - **Every rule has a fixture.** `apps/desktop/src-tauri/tests/cleaning_rules.rs` builds, for
    each rule of the embedded file, a tree with the core fakes holding entries the rule must take
    and entries it must not (too new, at the age limit, excluded, matching no include, too deep,
    a link with content below it, a folder that is not empty, and files outside the root), scans
    it, compares the plan with the expectation, then cleans the plan against the fake and checks
    that exactly the planned entries moved. A guard test fails when a rule has no fixture or a
    fixture lacks a kind of entry the rule's shape makes possible.

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
  `WinServices`. `WinCommands` runs the commands port. The processes and memory ports have no Windows adapter yet, so
  `unsupported.rs` supplies `UnsupportedProcesses` and `UnsupportedMemory`, which fail every call
  with `PortError::Unsupported`.
- **Managed state.** The result is `AppState` in Tauri's managed state, not owned by the window, so
  the engine and the database connection outlive the on-demand window. If the core cannot be built
  (an unusable database, a build bug in the catalogue) the app still starts, the tray and window
  keep working, and every command answers with the same error, which the UI shows. `AppCore`
  holds a lock that serialises list, apply and revert, so two quick clicks cannot interleave.
- **The clean engine.** `build_core` also builds a `CleanEngine` from the embedded cleaning rules,
  the `ports.file_system` adapter, `WinKnownFolders`, the system clock, and the store (as the
  activity log and as the settings, for the minimum age of the temp rules). `AppCore::clean_engine()`
  returns it. `build_core(ports, store_path, catalogue_toml)` keeps its signature and calls
  `build_core_with_cleaning`, which takes a `CleaningSetup` (the rules text and the
  `KnownFolders` resolver) so a test can pass its own. The rules text is validated before the
  database is opened; a file that does not load is `InitError::CleaningRules` (a typed init
  error, like an invalid tweak catalogue), never a panic. There are no IPC commands or UI for
  cleaning yet: the Cleaner page is a later task.
- **The catalogue** lives in the repository at `catalogue/tweaks.toml` and is embedded into the
  binary with `include_str!` (`catalogue.rs`); `volt-core` only ever receives the text. A test loads
  the embedded text and fails the build on any problem. Today it holds exactly one tweak, Show file
  extensions (`HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced`,
  `HideFileExt` = DWORD 0): Safe, evidence Anecdotal because it is a usability and safety
  preference and not a performance gain, no restart (open Explorer windows pick it up on refresh).
  The real catalogue is a later task. The cleaning rules are embedded the same way
  (`catalogue/cleaning.toml`, `EMBEDDED_CLEANING_RULES`), and a test loads the embedded text.
- **Where the files are.** The database is `volt.db` in the app data directory
  (`%APPDATA%\io.github.tatsuyakari1203.volt`); the diagnostic log is in the app log directory.

## Windows adapters

`volt-win` implements the registry, services, file system, known-folders, metrics and commands ports
with the Windows API: `WinRegistry`, `WinServices`, `WinFileSystem`, `WinKnownFolders`, `WinMetrics` and
`WinCommands`.

- File deletion goes to the Recycle Bin through `IFileOperation`. There is no permanent-delete
  fallback: if the Recycle Bin cannot take a file, the deletion fails. The shell reports a locked
  file only as an aborted operation, so the adapter probes the file and, when another program
  holds it open, returns `AccessDenied` (also what a sharing or lock violation maps to), which the
  clean engine reports as locked or in use.
- `inspect` describes one path and `read_dir` lists one directory, both without following links:
  an entry is a file, a directory or a reparse point (never both), with its size and modification
  time. `read_dir` refuses a reparse point or a file, so a scan cannot be led out of its folder.
  Sizes and times in a listing come from the directory data and are not re-read per file.
- `WinKnownFolders` implements `KnownFolders`: the shell's known folders, and the process temp
  folder in its long, resolved form. The protected list holds the profile, the Windows directory,
  Program Files, ProgramData, the application data folders and the user's document folders.
- `move_to_trash` takes a root. It refuses paths outside that root and anything whose path
  traverses a reparse point (junction or symlink).
- Tests are confined to `HKCU\Software\VoltTest` and temp directories. Scan and clean tests run
  the real engine over a temp directory with a real junction (a junction swapped in after the scan
  included); the tests that put items in the real Recycle Bin are `#[ignore]`d and run on request.
  `tests/cleaning_rules.rs` runs some of the shipped rules through `WinFileSystem` over a temp
  directory with a known-folders double (scan only), and an opt-in `--ignored` test scans every
  shipped rule on the real machine, read-only, printing counts and sizes and no names.
- **`WinMetrics`** needs no administrator rights and no vendor library, so it works for any GPU.
  CPU is `GetSystemTimes` (idle, kernel and user time; kernel includes idle), memory is
  `GlobalMemoryStatusEx`, disks are `GetLogicalDrives` filtered to `DRIVE_FIXED` with
  `GetDiskFreeSpaceExW` (removable drives, optical drives and network shares are never touched; a
  volume that cannot be read, such as a locked BitLocker volume, is left out). The GPU part
  (`gpu.rs`) uses the performance counters `GPU Engine(*)\Utilization Percentage` and
  `GPU Adapter Memory(*)\Dedicated Usage` through PDH (the English counter paths, so the display
  language does not matter) and DXGI for the adapter name and the installed dedicated memory
  (software adapters are skipped). The processes sharing a GPU engine are added up per engine and
  the busiest engine represents the adapter, which is how Task Manager reports it. The counter
  instance parsing and the aggregation are pure functions tested on strings. A GPU that cannot be
  read (no counters, a virtual machine, a driver without WDDM 2) reads as an empty list and one
  warning in the diagnostic log, never as an error. **NVML is not used**: the counters already
  cover NVIDIA, AMD and Intel, and a run-time-loaded NVML would be an optional refinement for a
  later task. Opening a `WinMetrics` opens a PDH query, so it is created when sampling starts and
  dropped when it stops. Reading is allowed in tests (it changes nothing); they assert
  plausibility, not exact values.
- Changing a real service's start type is deliberately not covered by an automated test, because
  it would change the machine running the tests.
- `WinCommands` starts a program directly with its argument list through `CreateProcess`: never
  through a shell, never from a string built by hand, so each argument arrives intact.
  - Resolution: a bare name (`powercfg`) is looked up in the Windows system directory only (never
    `PATH` or the current directory) and gets `.exe` when it has no extension. An absolute path is
    used as given. Relative paths, UNC paths and batch scripts (`.bat`, `.cmd`) are rejected as
    `InvalidInput`; a program that is not there is `NotFound`; other start failures keep the OS
    error through the same Win32 mapping the other adapters use.
  - The process gets no console window and a null standard input. Standard output and error share
    one pipe, so the text keeps its order. At most 64 KiB is kept, the rest is dropped and a
    note is appended. Text is decoded without failing (UTF-8, UTF-16 with a byte order mark, or
    the OEM code page).
  - A command has 60 seconds by default. The child runs in a job object that kills its members
    when it closes. On timeout the whole process tree is terminated and the call fails with
    `CommandError::TimedOut`, which reaches the port as `PortError::Other` (the core has no timeout
    variant). When the program exits by itself, processes it left behind are killed too.
  - Only the program's file name and the exit code are logged, per the redaction rule.
  - Tests run the test executable itself as the helper process (exit codes, output, arguments,
    sleeping), plus read-only system programs, so nothing changes the machine.
- There is no adapter for the processes or memory ports yet. The composition root hands the
  executor small stand-ins that fail every call with `PortError::Unsupported` (see Composition
  root).

## Process and elevation model

The elevation model is not decided yet. The proposal, to be confirmed by a prototype before code
depends on it: one elevated process that is both the tray agent and the Tauri host. The
alternative, an unelevated UI talking to an elevated helper over a named pipe, has a smaller
attack surface but costs an IPC protocol and a second binary. Everything below works unelevated
and does not depend on that decision, except where it says so.

What already depends on it: the machine-wide cleaning rules (`windows-temp`, `machine-wer-*`,
`windows-update-logs`, `delivery-optimization-cache`) are flagged `requires_admin`. Unelevated they
are scanned and report `needs_administrator()`; nothing is attempted by other means. Measured on a
development machine without elevation (a read-only scan, counts only): Windows temp and the
Delivery Optimization cache refuse the root itself, the machine-wide error report folders list
but refuse every report folder inside, and the Windows Update logs are readable. Cleaning them
for real (and the Windows Update download cache, which also needs the update service stopped and
started) waits for the elevation decision.

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
- **Capabilities.** Both plugins are driven from Rust only, so no plugin permission is granted.
  The default capability grants the window exactly two core permissions, `core:event:allow-listen`
  and `core:event:allow-unlisten`: without them Tauri refuses `listen` from the page ("event.listen
  not allowed"), so no typed event (`app-ready`, `tray-boost-requested`, `metrics-updated`) could
  reach the UI. The window still cannot emit events. The plugins' own JavaScript commands are therefore
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

### Live metrics

CPU, RAM, GPU utilisation, video memory and disk usage reach the UI as a typed event while the
window is open. The code is `apps/desktop/src-tauri/src/sampler.rs` (the sampler) and `metrics.rs`
(transfer types, event and command).

- **Runs only while a window exists.** `Sampler` lives in Tauri's managed state and starts a
  thread named `volt-metrics` when the main window has been created (`open_main_window` calls
  `sampler::sync_with_window` after the build succeeds) and stops it when the window is destroyed
  (`Builder::on_window_event` on `Destroyed`). The decision (`decide`: start, stop, keep) is a pure
  function of "is it running" and "does the window exist", and the existence probe runs inside the
  sampler's lock, so the last caller always sees the final state. Because this code does not rely
  on the order in which Tauri unregisters a window and sends its `Destroyed` event, the thread
  also checks on every tick that the window still exists and ends by itself within one interval
  if it does not.
- **Nothing is held while idle.** The thread builds its `WinMetrics` when it starts and drops it
  when it ends, so a closed window holds no performance-counter query, and `latest_metrics`
  answers `null` again. The diagnostic log has one line when sampling starts and one when it stops
  (`live metrics sampling stopped after N samples`), which is how a real run is checked.
- **Timing.** One second between samples. `run_loop` waits on a channel with a timeout (so a
  stop request ends the wait at once) until the next deadline of a fixed grid (`next_deadline`).
  The grid keeps the period free of drift by the time a read takes, and when a read is slow the
  missed ticks are skipped, not run back to back. One thread, no queue: samples cannot pile up. A
  baseline read is taken at the start and not published, so the first event already has a CPU
  figure. A failing read is counted, logged once per streak, and sampling goes on.
- **Delivery.** `MetricsUpdated` (`events.metricsUpdated`) carries a `MetricsView` about once a
  second, emitted by `tauri-specta`. The command `latest_metrics` returns the last snapshot of the
  current run (or `null` before the first one), so a screen can show numbers at once instead of
  waiting a second; call it after registering the event listener, which is asynchronous. Every
  GPU value and `cpuPercent` can be `null`.

#### Sampling overhead

Measured on 2026-11-10 on the development machine (Windows 11, an NVIDIA GPU, four fixed disks,
other builds running at the same time, so treat the figures as an order of magnitude). Method:
`pnpm tauri build --no-bundle` (release profile); start `volt-desktop.exe`; for the CPU, read
`TotalProcessorTime` of the process (and of each of its threads) before and after a 60 to 90
second interval, once with the window open (sampler running, the window left idle on Home) and
once after closing the window with `CloseMainWindow`; for memory, read the private working set
(`Win32_PerfFormattedData_PerfProc_Process.WorkingSetPrivate`) of `volt-desktop.exe` only, not of
the WebView2 processes. The diagnostic log was read to confirm the sampler started and stopped.

| State of `volt-desktop.exe`                        | CPU time (process)                 | Private working set |
| -------------------------------------------------- | ---------------------------------- | ------------------- |
| Tray only, window never opened                     | 0.0 ms over 61 s                   | 1.8 MB              |
| Window open, sampler running (1 sample per second) | 109 ms over 61 s; 375 ms over 91 s | 9.8 to 10.1 MB      |
| Window closed again, sampler stopped               | 0.0 ms over 31, 61 and 91 s        | 8.2 to 8.5 MB       |

- **CPU while sampling:** 1.8 to 4.1 ms of CPU per second, about 0.2 to 0.4 % of one core. In the
  91 second run the whole 375 ms was spent on one thread (the `volt-metrics` thread); every other
  thread used none. Nearly all of it is the GPU counters: in a debug test run one GPU read cost
  about 8 ms, against about 0.1 ms for the disks and 0.2 ms for CPU and memory together. CPU and
  memory alone would cost far less than this; the GPU part is the price of vendor-neutral GPU
  numbers without administrator rights.
- **CPU after the window closes:** zero. The log shows `live metrics sampling started` when the
  window is created and `live metrics sampling stopped after N samples` at the moment it is
  closed (for example 111 samples over a 111 second window); nothing is read afterwards.
- **Memory, an honest cost:** the first use of the performance counters (PDH) loads
  process-wide counter tables that closing the query does not release. An A/B run of a debug
  build with the real page loaded, a trivial fake metrics source against `WinMetrics`, left 3.7 MB
  against 8.0 MB after one open and close; a standalone test process shows the same step (about
  3 MB for the first counter of any kind, about 2 MB more for the GPU counters) and the thread
  count is the same in both runs. So after the first time the window is opened, an idle Volt keeps
  about 4 to 5 MB more than before (roughly 8 MB instead of the 3.8 MB in the idle table above);
  a Volt that stays in the tray without ever opening its window is unchanged. The query itself,
  the thread and the snapshot are freed at stop. Getting this back means reading the GPU through
  `D3DKMTQueryStatistics` instead of PDH, which is a possible later task.

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
- `features/undo` is the Undo and activity page, described under Undo page below. A tweak can
  still be undone from the Tweaks screen. Features do not import each other.
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
    `revert_tweak(id)`, `list_activity(limit, cursor)` and the five Undo page commands
    (`undo_entry`, `undo_batch`, `preview_undo_batch`, `export_activity` and
    `export_activity_text`, see Undo page). They are async and run their work on the
    blocking thread pool, never on the main thread, because they touch the registry and `SQLite`.
    The commands are thin: the logic is in `src-tauri/src/api.rs`, which the integration tests call.
    Apply reports precisely how it ended: `applied`, `alreadyApplied`, `rolledBack` (a step failed
    and the system was read again and is as before) or `needsAttention` (it may differ), with a
    redacted reason (the error class, never command output or data) and the state read again.
    Revert reports `reverted`, `incomplete` (with counts) or `nothingToRevert` (the journal has
    nothing to undo: Volt did not apply it, or it is already undone; not an error). Activity pages
    are newest first; the limit is clamped to 1 to 200 and `nextCursor` is the cursor of the next,
    older page (or null). `latest_metrics` is a synchronous command that only reads a stored
    snapshot (see Live metrics).
  - **Events.** Events are structs deriving `tauri_specta::Event` and registered in
    `bindings_builder`; the sample is `AppReady(AppInfo)`, listened to as `events.appReady`.
    `TrayBoostRequested` (`events.trayBoostRequested`, payload `null`) is sent when the user
    picks Boost in the tray menu. `MetricsUpdated` (`events.metricsUpdated`, payload
    `MetricsView`) is sent about once a second while the window is open.
  - **64-bit integers.** `specta-typescript` refuses to export `u64`/`i64` by default because
    JavaScript numbers lose precision above 2^53. A type that exposes one fails the export (and
    so the gate) until it is given an explicit representation, for example a decimal string.
    Registry QWORD values will need that when commands expose them. The tweak and activity
    types avoid 64-bit numbers altogether: record ids are decimal strings (and so is the cursor) and
    times are ISO 8601 UTC strings. The metrics types are the one exception: byte counts cross as
    `f64` (exact up to 2^53, and the UI has to divide and format them), never as `u64`.
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

### Undo page

`features/undo` over the functions in `src-tauri/src/api.rs`. Reversibility is only trusted when it
is visible, so every record Volt writes is listed and says whether it can be taken back.

- **What a row shows.** The time, who started it (You, Automation), the redacted one-line summary
  of what changed, the previous value and a state. The previous value is the journal entry's prior
  state under the redaction rule: numbers and service start types are shown, string and binary
  data only as kind and length (`REG_SZ (3 chars)`), and a value that did not exist is
  `(not set)`. It is shown for a change that was made.
- **Row states** (`ChangeState`, derived by the shell from the record's outcome and its journal
  entry, never guessed by the UI): `canUndo` (applied, entry still applied: has an Undo button),
  `undone` (the entry was reverted), `failed` (the attempt failed or was interrupted),
  `cannotUndo` (a change record with no journal entry, whose entry is gone or unreadable, or not
  confirmed yet: one line, `reason`, says why; cleaning and memory actions will produce these) and
  `restored` (the line itself records an undo or an automatic rollback; no badge, no button).
- **Batches.** Neighbouring records of one journal batch (what one applied plan did, and the undo
  of it) are shown as one group named after the tweak (`title`, from the catalogue), with an Undo
  all button while something in it can be undone. Grouping is done in the UI on the newest-first
  list, so paging is unchanged; records that are not next to each other are never grouped.
- **Undoing.** A row's Undo calls `undo_entry(entry)` directly. Undo all asks first: a Dialog fed
  by `preview_undo_batch(batch)` lists, from the journal, every change still applied in the batch
  and what will be done to it (`set back to REG_DWORD 5`, `removed (it did not exist before)`),
  then `undo_batch(batch)` runs. Both go through the same `Executor` as the tweak engine
  (`TweakEngine::executor`), so they are journaled, recorded in the activity log with the tweak
  label and serialised with apply and revert by the core's lock. The result is precise:
  `restored`, `partial` (with the entries that failed, what they touch and why), `failed` or
  `nothingToUndo` (already undone, or not a change; not an error). Undo continues past a failure,
  so as much as possible is restored, and an entry that failed stays undoable. An unknown id is
  `notFound` and a malformed one `invalidInput`.
- **Freshness.** After every undo, also a failed one, the page reads the list again (keeping about
  as many records as were shown). The Tweaks screen reads the live state of every tweak whenever it
  is mounted and the shell mounts only the current section, so opening Tweaks after an undo always
  shows the state after it.
- **Known limit: two changes to one value.** An undo restores the value the journal recorded
  before that change. If a later change to the same target is still applied, undoing the earlier
  one first overwrites the later one's effect; Volt does not warn about this yet.
- **Export.** The activity log can be saved as a text file (Export) or put on the clipboard
  (Copy). `export_activity_text` renders every record with `volt_core::render_text` (UTC time,
  source, outcome, summary, links: only what the records hold, so the redaction rule holds) and the
  UI copies it with the web clipboard API. `export_activity` is the save path: the Rust side opens
  a native save dialog (`tauri-plugin-dialog`, pinned `=2.7.3`; the 2.8 releases need tauri 2.12
  and Rust 1.90), writes the file to the path the user chose and answers `saved` (path and record
  count) or `cancelled` (not an error). The plugin is registered and used from Rust only: the
  capability file still grants no permission, so the UI cannot open dialogs or touch the file
  system itself. The dialog needs a window and is not covered by an automated test; its parts (the
  text, the file name `volt-activity-<date>.txt`, writing to a path, the error mapping) are.
- **IPC surface.** `ActivityItem` carries `batch` and `entry` (decimal strings), `previous`,
  `state`, `reason` and the tweak `title` besides the earlier fields. New commands, all async on
  the blocking pool with shell-owned transfer types: `undo_entry(entry) -> UndoResult`,
  `undo_batch(batch) -> UndoResult`, `preview_undo_batch(batch) -> UndoPreview`,
  `export_activity() -> ExportResult` and `export_activity_text() -> ActivityExport`.
- Screenshots with sample data (a mocked IPC) are in `docs/design`: `undo-light-460x700.png` and
  `undo-dark-460x700.png`, and the confirmation dialog as `undo-confirm-light-460x700.png` and
  `undo-confirm-dark-460x700.png`.

## Testing strategy

| Level       | What                                       | How                                                                                                                                                                                                                                |
| ----------- | ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Unit        | Core use cases, tweak engine, clean engine | In-memory fakes of the ports; runs on any OS                                                                                                                                                                                       |
| Adapter     | `volt-win`, `volt-store`                   | Real APIs against a throwaway `HKCU\Software\VoltTest` key, a temp directory and a temp database only                                                                                                                              |
| Integration | The shell's wiring and commands            | The real `WinRegistry` on a throwaway `HKCU\Software\VoltTest` key and a real store in a temp directory (apply, restart, revert, activity, undo of one entry or a batch), then the same wiring over fakes for the failure outcomes |
| UI          | Components and feature state               | Vitest, with the IPC wrapper mocked                                                                                                                                                                                                |
| End to end  | A few flows through the real app           | WebDriver, added once flows exist                                                                                                                                                                                                  |

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
