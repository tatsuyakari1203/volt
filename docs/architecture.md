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
| `volt-helper`  | The elevated helper: one pipe, one bound UI, a closed request set     | all three        | `tauri`, `rusqlite`                                     |
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
  Operations (cleaning, memory purges and Boost write to it), and a record outlives the journal entry
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
  - **Source of a revert:** `revert_entry` and `revert_batch` take a `Source` and an optional
    label, and the record carries the revert's own source (manual or automation). A rollback keeps
    the source of the original entry. A failed apply keeps only the port error class in the
    journal entry's error; `redacted_entry_error` also redacts older text.
  - **History incomplete:** `AppCore` keeps a `history_incomplete` flag, set whenever an engine
    reports a failed activity write and kept until the app restarts. `history_status` exposes it.
  - **Retention:** `Retention::DEFAULT` is 90 days and 5000 records. The `ActivityLog` port has
    `prune` and `clear`; both touch only the activity table and keep records linked to a journal
    entry that is applied or pending (the store reads the journal table in the same database),
    because the Undo page lists undoable changes from the log. Pruning runs at startup, after
    `reconcile_pending`, and on every 100th insert.
  - **Processes:** `volt_core::process` has a `ProcessMonitor` that groups processes by executable
    path (by name when the path cannot be read) and computes CPU % from two samples (it needs a
    clock and the logical CPU count). A `ProcessInfo` carries the creation time, the optional
    executable path and the session id. `end_group` enforces protection in the core: a list of 25
    Windows-critical names, every session 0 process, Volt and its descendants (through
    `parent_pid`) and pid 4 or below are refused. `terminate` takes the creation time and the
    adapter checks it on the opened handle, so a reused pid is never ended (it is reported as
    already gone). The path costs one `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` per process
    per list call and is `None` for processes a normal user cannot open; `ProcessGroupView` has an
    optional `path`. Each process gets a result
    (`Ended`, `NeedsAdministrator`, `AlreadyGone`, `Failed`), and one activity record says it
    cannot be undone; there is no journal entry.
  - **Process settings** (`volt_core::tuning`): priority class (Low to High, never Realtime), CPU
    affinity (a non-empty core set, the first 64 logical processors) and Efficiency mode (EcoQoS
    together with Low priority). One `Operation::TuneProcesses` per app is journaled with every
    process's prior settings; revert restores the processes that still run, and a revert when all
    have ended is a recorded no-op. The `Processes` port gained `settings` and `apply_settings`;
    the Windows adapter opens a process with `PROCESS_SET_INFORMATION`, so "readable" means
    "changeable" and elevated or protected processes report `AccessDenied`. Rules are stored by
    executable name (path when known) under the settings key `process.rules` and reapplied by
    `ProcessTuner::apply_rules`, called from `RulesSource`, a decorator of the sampler's metrics
    source, every 5 reads (about 5 s). There is no extra thread or timer: with no rule a tick reads
    nothing, with rules it reads the process list once per 5 s. In the tray with no window the
    sampler is stopped, so rules wait until a window opens (the first read applies them). A rule
    that cannot be applied is reported once (an activity record and the rule's problem label) until
    it works again; processes already handled are not retried. The Undo page labels the operation
    `process.tune` as "Process settings". Screenshot: `docs/design/process-rules-light.png`.
    Reapplying a saved rule to new processes is not journaled and is not an undo entry: the rule
    is the record, and removing it is the undo for the future (processes already running keep
    what they got until the manual change is undone). One Automation activity line is written per
    rule and session, on its first successful application. A rule that could not change some
    processes counts them (`RuleStatus.left_alone`, shown in the Saved rules card). Efficiency
    off means `PowerThrottling::Default`. `save_rule` checks the core mask against the app's
    system affinity mask. The manual tune path is unchanged (one `TuneProcesses` journal entry).
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
- **Health score:** `volt_core::health`, a pure function from a few read-only inputs to a score that
  explains itself. `HealthInputs` holds the free space of the system disk, memory in use as a
  share, whether Windows waits for a restart and an optional startup load; `assess` returns a
  `HealthReport`: a list of `Deduction`s and the checks that could not run. A deduction has a
  stable id (`disk.critical`, `disk.low`, `disk.watch`, `memory.critical`, `memory.high`,
  `restart.pending`, `startup.many`, `startup.some`), a fixed number of points, a title, a reason
  in plain words with the numbers found, and a fix from the closed set `FixAction` (open the
  Cleaner, open Memory, open Startup, restart Windows).
  - **The score is never stored.** `HealthReport::score` is 100 minus the points of the listed
    deductions and nothing else, so the number cannot change without a visible reason. A test runs
    it over a grid of about 20 000 inputs that crosses every threshold from both sides (also with
    missing inputs) and checks the sum, that ids are unique and known, that every deduction has
    points, a title and a reason, and that more pressure never deducts fewer points. The most the
    rules can take off is `MAX_TOTAL_DEDUCTION` (80), so the lowest score is 20.
  - **Thresholds are named constants** next to the rules, each with a one-line justification.
    Disk: below 5 percent free is critical (30 points), below 10 low (20), below 15 worth
    watching (10); under 5 GB free is always critical, and a disk with 100 GB or more free is
    never penalised, so a large disk is not blamed for a small share. Memory: 90 percent in use
    or more is critical (25), 80 percent or more high (12). A pending restart is 10 points.
    Startup load is the number of enabled startup entries from the startup manager (8 or more, 8
    points; 15 or more, 15). When a source cannot be read the figure is unknown and the check is
    listed as unchecked instead of being left out. `health_report` reads the startup manager
    through `AppState`, so it depends on the core.
  - **Never a guess.** An input that could not be read (the sample failed, the system disk is not
    in it, a registry marker is unreadable) is listed in `unchecked`, costs no points and is shown
    to the user. When no check could run the level is `Unknown` and there is no grade.
  - **Computed on demand.** Memory pressure changes second by second, so the score is worked out
    when Home opens and when the user asks to check again, never on the metrics tick: a number that
    moved while the user read it would look like a fault. The live cards show the moving values.
    `inspect` reads the system once and applies the rules.
  - **Pending restart** (`pending_restart_markers`) is read through the registry port, read-only:
    the key `HKLM\...\Component Based Servicing\RebootPending`, the key
    `HKLM\...\WindowsUpdate\Auto Update\RebootRequired`, and a non-empty
    `PendingFileRenameOperations` (only a pair with a destination, a real rename, counts;
    delete-only pairs are ignored) under `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager`.
    Positive evidence stands when another marker cannot be read; when none is set and one could
    not be read the answer is an error, never "no restart". The `Registry` port gained
    `key_exists` for the two marker keys, which hold no value: an adapter answers `true` for a key
    it may not open, because it does exist. **Known limit:** installers and updaters (the Edge
    updater, for one) queue deletions of their temporary files in `PendingFileRenameOperations`,
    so the marker can be set on a machine that needs no restart for an update; Volt reports it
    as the Windows marker it is.
- **Startup apps:** the `startup` module lists what starts with Windows and switches entries on
  and off through the executor, with the same records Task Manager uses. It needs one addition to
  the registry port (`list_values`, a default method that answers `Unsupported`, so an adapter
  without it still compiles) and one small port of its own, `FilePublisher`. `StartupManager` has
  `list_entries` (entries and source problems only) and `list` (adds logon scheduled tasks). The
  Home health check uses `list_entries`, so it does not enumerate Task Scheduler; the Startup page
  uses `list`. See Startup apps below.
- **Installed apps** (`volt_core::apps`): lists installed apps (the registry port, three Uninstall
  places) and the current user's packaged apps (the `PackagedApps` port), and removes one through
  an `Uninstaller` or the package manager. It is not an Operation: nothing goes through the undo
  journal, and the activity record says Volt cannot undo it. `HiddenReason` (system component,
  update, framework, not removable, no uninstall command) is decided in the core. Volt itself is
  never listed, by the name or because the uninstaller's folder or `InstallLocation` is Volt's own
  folder or inside it. The `QuietUninstallString` is never used. `UninstallCommand::parse` reuses
  the startup command-line splitter and refuses an empty or malformed line, a network path, a
  relative path and any program that is not an `.exe` (a script or `.msi`). The adapter expands
  `%VAR%` in the program, looks a bare name up in System32 only, and starts it through
  `ShellExecuteExW` (Windows shows any UAC prompt) with the arguments as registered, waits for the
  exit and returns the exit code. The `Uninstaller` port takes a `StopWaiting` and returns
  `UninstallerExit { exit_code, waited }`, where `waited` is `Everything`, `FirstProcess` or
  `Stopped`. `WinUninstaller` puts the started process into a count-only job object (no
  kill-on-close) and waits until the job is empty, which covers self-copying uninstallers. An
  elevated uninstaller may not join the job: the adapter then reports `FirstProcess` and
  `AppManager` polls the app's registry entry (every 2 s, up to 20 minutes) until it is gone or the
  user stops waiting. `stop_waiting_for_uninstaller` ends the wait by hand without closing the
  uninstaller; the queue goes on and the outcome is read from the list. The result is checked by
  reading the list again.
  Packaged apps have no size: Windows exposes none cheaply and the listing never walks folders.
  `WinPackagedApps` (volt-win) uses `PackageManager.FindPackagesByUserSecurityId("")` (current user,
  works unelevated) and `RemovePackageAsync` (current user only).
- **Risk and evidence labels:** `Safe`, `Caution`, `Advanced`; `Measured` or `Anecdotal`. The UI
  shows both.
- **`fakes` cargo feature:** in-memory fakes of every port (`FakeSystem` wires them together
  with a shared `EventLog` that records side effects in order, so a test can prove the journal
  entry was written before the registry changed). Compiled for the crate's own tests and for
  other crates that enable the feature.
- **Memory cleaning:** the `Memory` port reads the memory lists and purges five areas, each
  separately. `MemoryCleaner` (`volt_core::memory`) runs a selection in a fixed order (working
  sets, file cache, modified list, low-priority standby, standby), reads the lists before and
  after and writes one activity record, not a journal entry, whose text says it cannot be undone.
  `MemoryCleaner` loads the `ExclusionList` from settings (`memory.exclusions`) at the start of
  every working-set trim and passes it to `Memory::trim_working_sets`; Boost, the page clean and
  the tray Boost share `MemoryCleaner::run`, so all honour it. The adapter matches by image name
  (case-insensitive) and never opens a protected process. `PurgeOutcome.processes_excluded` is
  reported to the UI and in the activity summary. A stored list that cannot be read means no trim
  and a stated reason. The shell refuses `WorkingSets` in the elevated clean, because the helper
  does not know the list and the trim needs no administrator rights.
- **Boost:** `volt_core::boost` (settings key `boost.config`). The default is a working-set trim
  only, with no cleaning; the opt-in moves only the user-temp rule to the Recycle Bin without a
  preview. The Safe scan covers rules with risk Safe and `requires_admin` false. Reports say bytes
  moved, never freed.
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
  - **Administrator rights.** A tweak may set `requires_admin` (optional, default false). The core
    never elevates and cannot detect elevation: `TweakEngine` starts with administrator = false,
    and the composition root calls `with_administrator(true)` once the elevation model is decided
    (the shell does not today). For a `requires_admin` tweak, apply and revert return
    `TweakError::NeedsAdministrator` before any read or write; `state()` and `plan()` are
    read-only and still work.
  - **Drift.** `TweakEngine::drifted` is read-only: a tweak is drifted when the journal still
    holds an Applied entry for it and `state()` is not Applied. It runs when the Tweaks page opens
    (its own `list_tweak_drift` read) and when Home opens; there is no background or tray work.
    Reapply is the normal `apply` and adds a new journal batch on top of the old one, so undo
    walks back through both. `accept(id)` marks the Applied entries `EntryState::Accepted` and
    touches no system state: `applied_by_volt` becomes false, so Reset applies again, and the Undo
    page lists the old change as "cannot undo" (`undo.accepted`).
  - **Configuration file** (`tweak/portable.rs`, shell side `transfer.rs`). JSON with
    `format = "volt-configuration"` and `version = 1`, holding tweaks as `{id, title?}` and a
    `settings` map. A newer version, a file over 1 MiB or one naming more than 500 tweaks is
    refused. An import reads only tweak ids and the known settings keys (`boost.config`,
    `memory.exclusions`); steps, values and commands always come from the local catalogue.
    `plan_import` is the dry run and `apply_import` re-plans before applying, so each tweak goes
    through `engine.apply`, one journal batch per tweak. Left out of the file: `clean.chosen_folders`
    (machine paths), `process.rules` (cores and priorities fit one CPU and one set of programs)
    and `ui.language` (it follows Windows). Protected programs are exported by executable name
    only. Settings an import stores are not journaled (nothing in Volt journals settings); the
    preview shows the current value of a replaced setting and `memory.exclusions` is only ever
    extended. The file dialogs and file I/O are Rust only, and a previewed import waits in a
    static until the user confirms it.
  - **Steps.** A step is a registry value, a registry key (`registry_key`, `exists = true` or
    `false`), a service or a command, each mapped to a journaled Operation. `EnsureRegistryKey`
    records how many levels of the path were missing; revert removes those levels leaf-first, each
    only while it holds no values and no subkeys. A value step records the same when its key was
    missing. `DeleteRegistryKey` records the key's values (at most 64 values and 64 KiB) and is
    refused for a key with subkeys or unreadable values; revert recreates the key and rewrites the
    values. The `Registry` port has `list_subkeys`, `create_key` and `delete_key`.
  - **Describe, provenance and reset.** `describe()` lists every step whatever the state.
    `applied_by_volt()` reads the journal, so the UI can tell a tweak set outside Volt from one Volt
    applied. Each step may declare `off`, the Windows default. `reset()` and `reset_plan()` turn
    off an externally set tweak: they write the `off` value, remove the value when a set step has
    none, delete a key a step made exist, set a service's off start type or run a command's off
    command. Every step must declare a default or the tweak cannot be reset. Reset runs the steps
    last to first, is journaled under the label `<id>:reset` (so `revert()` cannot re-apply it and
    the Undo page can restore the values) and is refused with `TweakError::AppliedByVolt` for a
    tweak Volt applied itself.
  - **The starter catalogue** (`catalogue/tweaks.toml`) holds 34 tweaks. Tests run every tweak
    against the fakes (apply, revert, exact previous state), so a new tweak is covered
    automatically. Rules for adding one: documented or widely verified only; nothing that weakens
    security or deletes anything; evidence is Anecdotal unless Volt measured it. Real-machine
    verification is still open.

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
    `local_app_data`, `local_low`, `roaming_app_data`, `program_data`, `windows`, `user_profile`, and
    the user-added chosen folder) plus a relative
    path. `user_profile` is the shell Profile folder; the profile is protected, so a root there must
    lie below it (for example `.cargo`). The
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
    A rule's scan stops at `MAX_ITEMS_PER_RULE` (25,000 items) and says so (`RuleScan::truncated`).
    `CleanEngine::scan_observed` takes a `ScanObserver` that is told the progress and can cancel.
    Cancel is checked per entry inside a rule and between rules. A cancelled scan is a partial
    plan (`CleanPlan::cancelled`) with the cut rule marked truncated; there is no
    `ScanError::Cancelled`. The shell keeps a partial plan as the current plan, so it can be
    cleaned after confirmation; if no rule was reached, the earlier plan stands.
    `scan_cleaning_rules` takes an optional `continue_plan` id so a cancelled scan can be
    finished; the stored plan is merged in rule order.
  - **Install markers.** A rule that works in a user-chosen folder declares `install_markers`:
    names that must exist directly in the chosen folder. They are checked read-only on choose,
    scan and clean, so a replaced folder is not scanned and its old plan is skipped as
    `RootChanged`. `choose_cleaning_folder` returns an `Option<FolderRefusal>` (a phrase) for a
    refused folder instead of an `IpcError`. Ollama stays one whole-store item: removing a single
    model needs manifest parsing and reference counting of shared blobs.
  - **Items by entry, picked items, relocated caches.** A rule can set `unit = "entries"`: each
    entry directly inside the root is one item, and a folder goes to the Recycle Bin whole in one
    move. Its size, file count and newest file time are read again before the move (a changed
    folder is skipped), and links inside are not followed and travel with the folder. `pick = true`
    (needs entries) makes the user pick items one by one: `confirm()` and `confirm_rules()` leave
    such rules out and only `confirm_selection` takes picked items, so nothing is taken by
    default. `follow` lists documented environment variables (each with a `source`) that move the
    root; the `KnownFolders` port gained `env_folder()`. Implausible values (a drive root, a
    protected folder or above it, inside a system folder) are ignored. `Location::ChosenFolder` is
    a folder the user adds, stored in the settings store under `clean.chosen_folders` and judged
    by the same checks as a followed folder; `output` and `outputs` are never offered or taken for
    these rules, in scan and in clean. `Location::LocalLow` resolves `FOLDERID_LocalAppDataLow`
    (also in the protected list). The category `AiModelCache` holds the model caches.
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
    Recycle Bin and keep their space until it is emptied. `describe()` says so ("No disk space is
    freed until the Recycle Bin is emptied"), and UI text must say "moved to the Recycle Bin".
    Emptying is a separate, user-confirmed action outside the clean engine: the `RecycleBin` port
    and `RecycleBinManager` (`volt_core::clean::bin`, implemented by `WinRecycleBin`) report the
    bin's status and empty it, writing one activity record.
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
  - **Programs that lock a rule's files.** A rule may list `locked_by`, the executables that keep
    its files open. `CleanEngine::with_processes` lets a scan report which of them are running
    (`RuleScan::running_programs`), and the Clean page notes that their open files are skipped.
    Locked files were always skipped and reported by clean; this only names the browser before the
    user cleans.
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
    (risk Caution: the service owns it). Browser rules in the `browser_cache` category
    (Chrome, Edge, Brave, Vivaldi and Firefox) are Safe and run per profile:
    the profile wildcard is an include pattern one level below the root (`*/Cache/**`), and
    excludes keep the walk out of user data folders. The Chromium rules include the per-profile Dawn
    caches and the GPU caches beside the profiles; Service Worker storage and the crx caches are
    left out on purpose. Opera and Opera GX have no rule: the profile and cache layout could not be
    confirmed from vendor documentation or the Chromium source. The UI leaves them unticked by default, as it
    does every category listed in `UNTICKED_CATEGORIES` (`messages.ts`). Every description says what the files are, why removing
    them is safe and what the user notices. **Deliberately not rules:** the Windows Update
    download cache (clearing it needs the update service stopped and started again, which needs
    elevation) and emptying the Recycle Bin (a permanent delete, so it is the separate action
    above). A location is left out when there is doubt.
  - **Shader caches** (`shader_cache`, Safe, 1 day minimum, unticked): `directx-shader-cache`,
    `nvidia-shader-cache` (`DXCache`, `GLCache`), `nvidia-compute-cache` (the roaming
    `ComputeCache`) and `amd-shader-cache` (`DxCache`, `DxcCache`, `GLCache`). AMD's `VkCache` is left out.
  - **Chat app caches** (`chat_app_cache`, Safe, 1 day minimum, unticked): `discord-cache`,
    `telegram-cache`, `teams-cache`, `teams-classic-cache` and `zalo-cache`. They name the cache
    folders in `include` and keep user data out with `exclude` as a second guard; `locked_by` names
    the app. Zalo's `ZaloData/media` (received files) and `Database` (messages) are never taken.
  - **Developer caches** (`developer_cache`, unticked): nine rules (npm, pnpm store and metadata,
    pip, cargo, NuGet HTTP cache and packages, Gradle, Yarn). `pnpm-store`, `cargo-cache`,
    `nuget-packages` and `gradle-caches` are Caution because rebuilding them is slow or needs the
    network; the others are Safe. The rules follow `CARGO_HOME`, `PIP_CACHE_DIR`, `npm_config_cache`, `PNPM_HOME`,
    `NUGET_HTTP_CACHE_PATH`, `NUGET_PACKAGES`, `GRADLE_USER_HOME` and `YARN_CACHE_FOLDER`, each
    verified against the tool's documentation; a location set only in a tool's configuration file
    is not followed, and the descriptions say so. Tool
    clean commands (`npm cache clean`, `docker system prune`) are not used because they cannot be
    previewed or sent to the Recycle Bin; Docker is left out for that reason. Model caches
    use picked items and user-added folders (see above).
  - **Every rule has a fixture.** `apps/desktop/src-tauri/tests/cleaning_rules.rs` builds, for
    each rule of the embedded file, a tree with the core fakes holding entries the rule must take
    and entries it must not (too new, at the age limit, excluded, matching no include, too deep,
    a link with content below it, a folder that is not empty, and files outside the root), scans
    it, compares the plan with the expectation, then cleans the plan against the fake and checks
    that exactly the planned entries moved. A guard test fails when a rule has no fixture or a
    fixture lacks a kind of entry the rule's shape makes possible.
  - **Large files finder** (`volt_core::large_files`). Beside the rules, the Clean page can list
    the biggest files under a folder the user picks. The scan is read-only and goes through the
    `FileSystem` port: an iterative walk that never enters a reparse point, with a bounded heap
    that keeps the top 200. Moving the chosen files to the Recycle Bin re-checks each file's size
    and modification time and reuses `FileSystem::move_to_trash` and `CleanSkipReason`. One
    activity record is written, label `large_files`, with counts only. Exclusions:
    `KnownFolders::system_folders` (Windows, both Program Files folders and ProgramData; a scan
    root inside them is refused, and if the port cannot name them the scan refuses to run), the
    names `pagefile.sys`, `hiberfil.sys` and `swapfile.sys`, `$Recycle.Bin` and
    `System Volume Information`. `scan_filtered` applies a `ScanFilter` (type, minimum age in
    days) while walking, before the bounded top-200 heap, so filters find files beyond the 200
    biggest at the cost of a rescan when the filter changes. Files with the system attribute
    (`EntryInfo.system`) are excluded wherever they are and counted in `excluded`.
  - **Tidy finder** (`volt_core::tidy`, shell `src-tauri/src/tidy.rs`, the Duplicates view). Next
    to the large files finder, the Clean page finds duplicate files, empty folders and broken
    shortcuts. The `FileSystem` port gained `identity()` and `read_at()` (default `Unsupported`
    bodies); `WinFileSystem` reads attributes before opening and refuses reparse points and files
    not on disk (offline, recall-on-open, recall-on-data-access), so reading never triggers a
    cloud download. Hashing runs in the core with `sha2`; the port only supplies bytes.
    Duplicates: size, identity (drop placeholders, fold hard links), SHA-256 of the first 64 KiB,
    then the full SHA-256. Limits: 1 MiB minimum size, 300k files, 500k folders, 500 groups
    listed. An empty folder is the topmost folder holding nothing or only empty folders, never a
    scanned root, link, system-attribute, system or unreadable folder. A shortcut is broken only
    when its `.lnk` LinkInfo local path is on a fixed drive, the target is gone and the drive root
    exists; everything else counts as unchecked. Moves go to the Recycle Bin only, one activity
    record each (labels `tidy_duplicates`, `tidy_folders`, `tidy_shortcuts`); for duplicates a
    kept copy is re-hashed and at least one copy must stay. The scan stays in Rust: the UI sends
    the scan id and row numbers only.

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
- **Startup apps wiring.** `build_core(ports, store_path, catalogue_toml)` is unchanged and builds
  a core without the startup manager (its commands then answer `unsupported`).
  `build_core_with(.., StartupWiring)` adds it: `StartupWiring` holds the `StartupSources` (the
  registry keys and Startup folders, all injectable) and the `FilePublisher`. The app calls it with
  `StartupWiring::windows()`; tests with `StartupSources::sandboxed_in_hkcu`. The manager gets the
  registry and file system of `ports` and a handle on the tweak engine's executor, so a startup
  change is journaled, recorded in the activity log and counted like a tweak's. `AppCore` carries it
  as `startup_apps()`.
- **Ports.** `windows_ports()` builds the bundle from `WinRegistry`, `WinFileSystem` and
  `WinServices`. `WinCommands` runs the commands port. It also uses `WinProcesses` and `WinMemory`. `unsupported.rs`
  keeps only `UnsupportedProcesses`, which is wired nowhere and serves only as a test stand-in;
  it fails every call with `PortError::Unsupported`.
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
  error, like an invalid tweak catalogue), never a panic. The shell's cleaning commands
  and the Clean page are described under Typed IPC and Frontend.
- **The catalogue** lives in the repository at `catalogue/tweaks.toml` and is embedded into the
  binary with `include_str!` (`catalogue.rs`); `volt-core` only ever receives the text. A test loads
  the embedded text and fails the build on any problem. It holds the 33 starter tweaks (see
  Tweak engine above). The cleaning rules are embedded the same way
  (`catalogue/cleaning.toml`, `EMBEDDED_CLEANING_RULES`), and a test loads the embedded text.
- **Where the files are.** The database is `volt.db` in the app data directory
  (`%APPDATA%\io.github.tatsuyakari1203.volt`); the diagnostic log is in the app log directory.

## Windows adapters

`volt-win` implements the registry, services, file system, known-folders, metrics, commands and
file-publisher ports with the Windows API: `WinRegistry`, `WinServices`, `WinFileSystem`,
`WinKnownFolders`, `WinMetrics`, `WinCommands` and `WinFilePublisher`.

- `WinRegistry::list_values` enumerates the values of one key (`RegEnumValueW`), sorted by name
  ignoring case and with the spelling they were written with. A missing key is an empty list; a
  value that cannot be modelled (an unknown type) is listed with its error and does not fail the
  list.
- `WinFilePublisher` reads the `CompanyName` of a file's version resource (`GetFileVersionInfoW`,
  `VerQueryValueW`, trying the file's own translations first). It expands `%VAR%` references
  itself (pure Rust, unknown variables stay as written), looks a bare name up in the Windows
  system folder only, and never reads a network path (UNC), which could hang or send the user's
  credentials to another machine. A file without version information, a missing file or a refused
  path all read as "no publisher". It reads and never runs the file.

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
  (`gpu.rs`) uses `D3DKMTQueryStatistics`, the interface Task Manager uses, and needs no vendor
  library or elevation. The adapter query gives the segment and node counts. The segment query
  gives `BytesResident`, and `Aperture` marks shared memory, so dedicated memory is the sum over
  the non-aperture segments. The node query gives `GlobalInformation.RunningTime` in 100 ns units,
  and utilisation is the busiest node's delta over the clock delta. DXGI still supplies the
  adapter name and the installed dedicated memory (software adapters are skipped). A GPU that
  cannot be read (a virtual machine, a driver without WDDM 2) reads as an empty list and one
  warning in the diagnostic log, never as an error. **NVML is not used**: the statistics cover
  NVIDIA, AMD and Intel. Reading is allowed in tests (it changes nothing); they assert
  plausibility, not exact values.
- `WinRegistry::key_exists` opens the key with query access only: not found is `false`, access
  denied is `true` (the key exists), anything else is an error. Its test is part of the registry
  contract that both the fake and the real adapter pass, on a throwaway `HKCU\Software\VoltTest`
  key; reading the real `HKLM` markers is read-only and is only done at run time.
- `WinMetrics::without_gpu()` is a source that never queries the GPU, for a
  one-off read of CPU time, memory and disks (the health check): three cheap calls that leave
  nothing open.
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
  - A command has 60 seconds by default, or its own `timeout_secs` (1 to 600, `MAX_TIMEOUT_SECS`;
    the adapter caps whatever it is given). The child is created suspended, added to a
    kill-on-close job object and then resumed, so no grandchild can start outside the job. On
    timeout the whole job is killed and the call fails with `PortError::TimedOut`
    (`IpcError::TimedOut` at the UI boundary). When the program exits by itself, processes it left
    behind are killed too.
  - Only the program's file name and the exit code are logged, per the redaction rule.
  - Tests start only the test executable's own helper processes, `cmd /c exit` and `ping` to
    loopback, and end only what they started, so nothing changes the machine.
- **`WinProcesses`** reads `NtQuerySystemInformation(SystemProcessInformation)`: pid, parent,
  private working set and user plus kernel CPU time, read by offset (tested with a hand-built
  buffer and with the test process). It ends a process with `OpenProcess(PROCESS_TERMINATE)` and
  `TerminateProcess`, after checking the creation time on the handle, and the adapter refuses pid 4
  or below and its own process.
  `windows_ports()` uses it.
- **`WinRecycleBin`** implements the `RecycleBin` port (status and empty).
- **`WinMemory`** reads the memory lists with `NtQuerySystemInformation` class 80 (struct and class
  from phnt `ntexapi.h`), which needs no rights, and purges five areas separately. Working-set
  trim needs no rights (`PROCESS_SET_QUOTA`; processes that cannot be opened are counted and
  skipped). `availability` only reads the token (privilege held means Available, absent means
  NeedsAdministrator) and enables nothing; `purge` enables the privilege right before the call. List purges need `SeProfileSingleProcessPrivilege` and the file cache trim needs
  `SeIncreaseQuotaPrivilege`, both only in an elevated token. A missing privilege is
  `PortError::AccessDenied`; a status the build rejects is `Unsupported`.

## Process and elevation model

Volt itself never runs elevated. Administrator work goes to a second, small program, `volt-helper`
(crate `crates/volt-helper`, a composition root over `volt-core` and `volt-win` with its own rule in
`check-arch.mjs`) that the UI will start through one UAC prompt and that serves ONE connection from
that UI over a private named pipe. Tested without elevation:

- **Protocol** (`volt-core::elevation`, the `Elevator` port): JSON, one message per pipe message,
  1 KiB request cap, a closed request set (`hello`, `memory_purge`, `tweak_apply`, `tweak_revert`,
  `history`, `bye`), six non-closing error codes, unknown fields refused,
  ids exactly 1, 2, 3. A framing or order violation gets an error reply and the connection is
  closed. A request that is valid but refused (no areas, rate limited) keeps the connection and
  never extends the idle clock. A tweak is named by id only, and the helper serves only
  `requires_admin` tweaks of its own embedded catalogue.
- **The process** (`volt-win::helper`, `volt-helper::run`). It hardens itself first: environment
  scrub, DLL search limited to System32, working directory System32, five process mitigation
  policies (among them no child process) and privileges cut to three. Then it parses the one
  accepted command line (`serve --pipe volt-<32 hex> --ui-pid N --ui-created TICKS`), checks that
  it runs from the Program Files install folder (owner and ACL of the image, the folder and the
  state folder, no reparse points), verifies the UI process it is bound to, opens its own state
  (create, verify, lock, store, reconcile pending entries), creates the pipe and serves. Exit codes are in `run::exit`; 6 means the helper's user is not the UI's user.
- **The pipe.** A unique first instance, one instance, remote clients rejected, message mode,
  overlapped I/O with timeouts, so the idle limit (10 minutes), the hard cap (60 minutes) and the
  exit on disconnect need no watchdog thread. A client is served only if it is the bound process
  (pid, creation time, image path) and passes the token rules (same user, same session, at least
  Medium integrity). Any other client is disconnected and counted, and the helper keeps
  listening. Quotas: one memory purge per 5 seconds and 30 changing requests per minute.
- **Lifetime.** The helper stops, with exit code 0, when the bound UI has not connected within
  30 seconds of the start (strangers refused meanwhile do not extend it), when the bound UI process
  ends (polled every 250 ms with the held process handle), when the bound client disconnects or
  says `bye`, after 10 idle minutes, or at the 60 minute hard cap.
- **The build** is `scripts/build-helper.ps1`. The release helper has the `requireAdministrator`
  manifest, a static CRT and the Dependent Load Flag, and no test modes. One cfg
  (`helper_release`, set by the build script from the release profile) gates both the manifest and
  the debug-only test modes, so they cannot coexist. The build script refuses a release build
  without `crt-static`.

- **Elevated UI refused.** If the UI process token is elevated and UAC is on (or cannot be read),
  Volt shows a native message and exits before it creates a window or WebView
  (`volt-win::helper::uac`: the UAC switch, the built-in Administrator SID, the message box). With
  UAC off system-wide, or as the built-in Administrator (RID 500), it starts normally because there
  is no boundary to protect. The message follows the Windows display language, since the settings
  store is not open yet.

- **State.** `Program Files\Volt\state` is created by the helper with a protected ACL (SYSTEM and
  Administrators) and verified like the install folder. It holds one folder per user SID
  (`state\<SID>\helper.db`), and `state\helper.lock` (an open handle with no sharing) is the
  single-helper lock. `volt-helper` may depend on `volt-store` (a rule in `check-arch.mjs`).
- **Administrator tweaks.** `tweak_apply`, `tweak_revert` and `history` run behind a
  confirmation, the `Confirmer` port. `ConfirmGate` holds the 10 minute block and the two-denial
  lockout. `AdminRegistry` is HKLM only: every key on the path is checked on opened handles
  (owner, DACL write rights, not a symbolic link); CREATOR OWNER and CREATOR GROUP entries are
  ignored because no token holds them. At build time `Catalogue::administrator_issues` rejects a
  command step and HKLM registry steps outside `requires_admin` tweaks.
- **Confirmation** is `volt_win::helper::prompt`, on a private desktop whose DACL is
  `D:P(A;;GA;;;BA)S:(ML;;NW;;;HI)`. The UI token has Administrators deny-only, so a medium
  process cannot open it, and Windows does not deliver messages across desktops. `ask()` makes
  the window on a worker thread attached to the desktop, then calls `SwitchDesktop` on the calling
  thread; the `Switched` guard always switches back. An unreadable input desktop (locked session)
  or a failed create or switch is `Unavailable` and fails closed. The 45 s timeout counts as a
  denial. Buttons are disabled for 1.5 s and Cancel is the default. Text comes from the helper
  catalogue (English) or the embedded `vi.catalogue.json` (Vietnamese) by the Windows display
  language.
- **Restore point.** For Caution and Advanced applies only, once per 10 minutes per session,
  behind the `RestorePoints` port. A failure refuses the tweak (`ErrorCode::RestorePointFailed`).
  Windows adds its own 24 h limit with no error. The hosted runner (Server) answers 120, so a real
  creation is not shown there.
- **UI mirror.** The UI mirrors the helper log by pull (it never starts the helper) and saves it
  in the settings key `helper.mirror`. Undo of an administrator row is `tweak_revert` by tweak.
  Rows of a recreated store show without Undo, and Clear keeps rows that can still be undone.

The rest of this page works unelevated and does not depend on the helper, except where it says so.

### The UI half of the helper

`volt-win::helper::launch` and `helper::elevator` start the helper and talk to it.

- **One connection.** `WinElevator` holds at most one authenticated helper connection per UI
  session. It starts the helper only on a user request, through `ShellExecuteEx` with `runas` on
  the sibling `volt-helper.exe`, whose path comes from this process's own image path. The
  arguments are a random pipe name plus this process's id and creation time.
- **Availability** (`launch::status_for`) is read-only and a pure function over probes: the UI
  executable and the helper pass `verify_install` (Program Files\Volt, ACLs, reparse points), the
  helper exists, and the token is an administrator (consent prompt or none), not a standard user.
  Each failure is a distinct `UnavailableReason`. Per-user, portable and development copies stay
  locked with a reason: only the all-users installer puts the helper next to the UI.
- **Trust.** Before the first request `server_is(process handle)` and a trusted pipe owner
  (Administrators or SYSTEM) are required (`Untrusted` otherwise). The hello reply must match the
  protocol and both catalogue hashes (`Mismatch` otherwise).
- **Lifetime.** A connection is reused while the process has not exited and the pipe is open. A
  request that breaks half-way is `Disconnected` and never retried; the next request relaunches
  (a new prompt). `bye` goes out on app exit.
- **Shell.** The commands `elevation_status` and `clean_memory_elevated`. The real elevator is
  attached only in `build_real_core`. An elevated purge writes one activity record in the UI
  store. Boost and the tray never call the Elevator (a behaviour test and a source guard).

What already depends on it: the machine-wide cleaning rules (`windows-temp`, `machine-wer-*`,
`windows-update-logs`, `delivery-optimization-cache`) are flagged `requires_admin`. Unelevated they
are scanned and report `needs_administrator()`; nothing is attempted by other means. Measured on a
development machine without elevation (a read-only scan, counts only): Windows temp and the
Delivery Optimization cache refuse the root itself, the machine-wide error report folders list
but refuse every report folder inside, and the Windows Update logs are readable. Cleaning them
for real (and the Windows Update download cache, which also needs the update service stopped and
started) waits for the elevation decision. The memory page shows the three admin-only kinds of
purge disabled with the reason, and `requires_admin` tweaks are refused (`needsAdmin` in the UI).

### Elevated CI job

`.github/workflows/elevated-helper.yml` runs the release helper for real on a GitHub-hosted Windows
runner (a throwaway VM with a full administrator token), on pull requests and on pushes to main,
limited by paths to the helper's code. It is not a required check.

- It stages `volt-helper.exe` and the harness in `C:\Program Files\Volt` with the inherited ACL,
  the harness named `volt-desktop.exe`. The harness (`volt-elevated-ci`, cargo feature
  `elevated-ci`, a bin of `volt-helper`) is never built by the release workflow or the installer
  and is not part of `pnpm check`.
- It refuses to run unless `GITHUB_ACTIONS=true`, `RUNNER_ENVIRONMENT=github-hosted` and
  `VOLT_ELEVATED_CI=1`. It uses the real `WinElevator` path and the real `runas` launch, runs the
  negative cases and writes a report to the job summary and the `elevated-helper-report` artifact.
- Measured there (elevated-to-elevated launch only): the pipe owner is `S-1-5-32-544`; the launch
  handle has mask 0x001fffff and wait, `GetProcessId` and the exit code work;
  `GetNamedPipeServerProcessId` equals the handle's pid; the helper's working set is 9.9 MiB after
  hello (peak 10.0) and 1.04 MiB after the first purge; the launch call takes 316 to 413 ms and
  launch to hello 347 to 444 ms; the purge works after hardening, twice on one helper.
- The job runs the release helper cases, proves the release helper holds no harness strings
  (`scripts/check-helper-harness.ps1`), then swaps in the `elevated-ci` helper and runs the tweak
  and prompt cases (`crates/volt-helper/ci/prompt_cases.rs`). The `elevated-ci` feature is a
  compile error in a release build. Screenshot: `docs/design/tweaks-administrator-light.png`.
- Not measured: a launch from a filtered medium-integrity token (handle rights across the UAC
  boundary) and the real ACLs of the machine cleaning roots.

### The installer

The NSIS installer (install mode `both`, hooks in `apps/desktop/src-tauri/installer-hooks.nsh`) is the
stock Tauri one plus those hooks. The user picks **All users** (`Program Files\Volt`, a fixed folder,
helper installed, administrator features possible; preselected for an administrator) or **Just me**
(`%LOCALAPPDATA%\Programs\Volt`, no helper, administrator features off). The hooks end a running
helper (5 seconds to leave by itself, then kill), reset the install tree to the inherited Program
Files ACL (skipping `state\`), refuse any other all-users folder and remove the helper from a per-user
install, so a folder the user can write to never holds it. The release helper is built by
`scripts/build-helper.ps1` and bundled only by `scripts/build-installer.ps1` through the
`tauri.helper.conf.json` overlay; the base configuration has no helper, so debug builds and
`pnpm tauri dev` need none. The release workflow runs `build-installer.ps1`. The "Installer" job of
`elevated-helper.yml` installs the real installer on the hosted runner, all users and then per user
(`scripts/installer-checks.ps1`), and asserts the layout, no write access for non-administrators,
Available against the installed folder, a helper byte-identical to the built one, the installed
helper's self-check, an update over a running helper and the uninstall. The "Elevated helper" job
still proves the full serve, purge and bye path with the same helper binary against a staged folder.
Known gaps: the stock Tauri uninstaller copies itself to `TEMP`, and the installer text is not
localised. The consent prompt and the launch from a filtered token are still unmeasured.

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
- **Tray.** One icon with the menu Open, Boost and Quit. A left click (on release) opens the
  tray panel (below); the menu is on the right click. The menu id to action mapping
  (`TrayAction`) is a pure function. **Boost** does not open the window: it runs `boost::boost_of`
  in Rust on a blocking thread (a second click while it runs does nothing), sets the tray tooltip
  and emits `BoostFinished` with the report. An open window navigates to Home and shows the report;
  with no window the run ends with a desktop notification (`tauri-plugin-notification`, driven
  from Rust; the UI is granted no notification capability), besides the activity history and the
  tooltip.
- **Tray panel.** A left click on the icon opens the borderless `tray-panel` window
  (`tauri.conf.json`, `create: false`, the same page with `?view=tray-panel`) with memory, CPU and
  Boost. It is built on a worker thread, placed by `panel::place` on the monitor of the click inside
  its work area, and destroyed on focus loss, Escape or its buttons, so no WebView stays alive. The
  right-click menu is unchanged. Focus loss within 600 ms of showing is ignored (the taskbar or a
  flyout may take focus back); after that an unfocused panel closes, and a tray click within 400 ms
  of a blur-close does not reopen it. The sampler runs while the main window or the panel exists.
  `tray::boost_now` is the one Boost job shared by the tray menu item and the panel (`BoostRunning`
  flag, tooltip, notification when there is no main window, `BoostFinished` event); the panel never
  prompts for elevation. Measured on a release build (private working set of the process tree, idle
  in the tray / panel open / 10 s after close, three runs): 2.5 / 74.5 to 77.2 / 5.1 to 5.3 MB. The
  open panel is 7 processes and after the close 1 (the WebView is gone); about 2.7 MB stays in the
  app process. Screenshot: `docs/design/tray-panel-light.png` (300 x 360 at 2x).
- **Single instance.** `tauri-plugin-single-instance` is registered before every other plugin. A
  second launch focuses (or recreates) the window of the first instance and exits.
- **Launch mode.** A launch with `--background` (what the Start with Windows entry passes) does
  not open a window and stays in the tray; any other launch opens the window.
- **Capabilities.** Both plugins are driven from Rust only, so no plugin permission is granted.
  The default capability grants the window exactly two core permissions, `core:event:allow-listen`
  and `core:event:allow-unlisten`: without them Tauri refuses `listen` from the page ("event.listen
  not allowed"), so no typed event (`app-ready`, `boost-finished`, `metrics-updated`) could
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

### Startup apps

`volt_core::startup` (core), `volt-win` (`list_values`, `WinFilePublisher`) and
`apps/desktop/src/features/startup` (the Startup section). It lists what starts with Windows and
switches entries on and off the way Windows does, so Task Manager and Settings show the same state.

- **What is listed** (`StartupManager::list`, read-only): the Run keys (`HKCU` once, because
  `HKCU\...\Run` is shared by the 64-bit and the 32-bit view, which was checked by reading both
  views on the development machine; `HKLM` in the 64-bit view and in the 32-bit view, which is
  `WOW6432Node` and holds different values) and the per-user and common Startup folders. Every
  `StartupEntry` has a stable id (`<origin>/<name>`), name, origin, the command as written, the
  program of the command, an optional publisher and the state. A Startup folder item is a plain
  file (not a folder, link or `desktop.ini`), and a `.lnk` is shown without its extension. With
  the optional `ShortcutResolver` port (`WinShortcuts`, shell link COM, read-only) a shortcut
  shows its target as the command and the target's publisher; without it, or when the link cannot
  be read, it shows its file path and has no publisher. A source that cannot be read (access
  denied) is a `SourceProblem` in the result; the rest of the list is still valid.
  **Logon scheduled tasks** come from the optional `LogonTasks` port (`WinLogonTasks`, Task
  Scheduler COM, read-only, unreadable tasks skipped) and are returned in `StartupListing.tasks`,
  apart from the switchable entries. They have no switch and are not counted in the health
  startup load. A failed task read is reported as `tasks_problem`, not as a source problem. Both
  COM adapters call `CoInitializeEx` on the calling thread and uninitialise it afterwards.
  `RunOnce`, services and the other autostart places are not listed.
- **The command line** (`parse_command_line`, pure): a quoted program ends at the closing quote;
  an unquoted one ends at the first whitespace after `.exe`, `.com`, `.bat`, `.cmd`, `.lnk` or
  `.scr`, which handles paths with spaces, and otherwise at the first word. Environment variable
  references stay as written (`%ProgramFiles%\...`): the core never reads the environment.
- **Publisher** (`FilePublisher`, optional by type): the company name of the program's version
  information. A failure or a blank answer is `None`, logged at debug level, never an error.
- **Switching** (`StartupManager::set_enabled`, dry run `plan`): Windows keeps the state of an
  entry in a small binary value of the same name under
  `HKCU|HKLM\Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\<kind>`, where
  `<kind>` is `Run` (the Run keys of the 64-bit view), `Run32` (the 32-bit machine-wide Run key) or
  `StartupFolder`. Volt writes that value; it never deletes the Run value or the shortcut.
  - **Value layout** (`startup::approval`): `REG_BINARY`, 12 bytes. The first four bytes are a
    little-endian `u32` of flags: `0x02` or `0x06` enabled, `0x03` disabled. The last eight bytes
    are a little-endian `FILETIME`: when the entry was disabled, zeros when enabled. Disabling
    writes `03 00 00 00` and the current time; enabling writes `02 00 00 00` and zeros, which is
    what Task Manager writes when an entry is switched back on (the value is kept). No value
    means enabled.
  - **Source of the layout:** it is not documented by Microsoft. It follows practitioner write-ups
    of the artefact (Harlan Carvey, "Does autostart really mean autostart?", windowsir blog,
    2022-07-09, and "StartupApproved\Run, pt II", 2022-07-17), and was checked on 2026-11-10 by
    reading the development machine's `StartupApproved\Run`, `Run32` and `StartupFolder` keys
    read-only (`HKCU` and `HKLM`): every value was `REG_BINARY` of 12 bytes; the entries
    switched off in Task Manager had flags `0x03` and a non-zero `FILETIME`, enabled ones `0x02`
    (or `0x06` under `HKLM`) with zeros. One `HKCU` value had flags `0x01`, which no source
    explains. **A record Volt does not recognise reads as enabled and is never reinterpreted**;
    switching such an entry off writes the documented `0x03` record and undo puts the old bytes
    back.
  - **Every change is an Operation** (a registry value write of the approval record) applied
    through the Executor: the previous value is journaled first, including "there was no record",
    the batch is labelled `startup`, and undo (the Undo page, also after a restart) puts back the
    exact previous bytes or removes the record that did not exist. An entry already in the
    requested state is left alone and journals nothing.
  - **Activity summaries** say what happened in plain words. The journal holds a plain registry
    operation, so the activity module recognises an approval record by its key (the last two
    components are `StartupApproved` and `Run`, `Run32` or `StartupFolder`) and writes
    `Disabled startup entry: <name>`, `Enabled startup entry: <name>` and, for an undo,
    `Reverted startup entry: <name> is enabled again` (or `disabled again`). The Undo page shows the
    previous value as `enabled`, `disabled` or `enabled (it had no record)` instead of
    `REG_BINARY (12 bytes)`, and the batch is titled "Startup apps". Per the redaction rule
    the record names the entry and the outcome, never the command line.
- **Machine-wide entries cannot be changed yet.** Changing the `HKLM` Run keys, their approval
  records and the common Startup folder needs administrator rights, and the elevation model is not
  decided. They are listed (marked "Needs administrator rights") but `set_enabled` returns
  `StartupError::NeedsAdministrator` before it reads or writes anything, which reaches the UI as
  `accessDenied` with a message saying so. There is no workaround and no attempt that could be
  half done.
- **Safe ids.** A Startup folder item's name becomes a path, so it must be exactly one plain
  component (no separator, no `..`, no drive): anything else is `invalidInput`. An entry is looked
  up again before every change, so a stale id is `notFound`.
- **Injectable places.** `StartupSources` carries every key and folder.
  `StartupSources::windows(&KnownFolders)` is the real layout;
  `StartupSources::sandboxed_in_hkcu` points all of them below one throwaway `HKCU` key and two
  temp folders with the same shape, which is how the adapter and the integration tests run without
  touching the real Run or `StartupApproved` keys.
- **Known limits.** Task Manager can also show "Startup impact" and entries from places Volt does
  not list; tasks have no switch; the "Task Manager shows the same state" claim rests on the
  layout above and has not been checked by changing a real entry. That check is manual: switch an
  entry off in Volt and see it off in Task Manager's Startup apps tab, then the other way round.

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
  when it ends, so a closed window holds no sampler state, and `latest_metrics`
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
  thread used none. These figures were taken with the earlier performance-counter (PDH) GPU read, which
  was nearly all of the cost (about 8 ms per read in a debug run, against about 0.1 ms for the
  disks and 0.2 ms for CPU and memory together); the D3DKMT read has not been timed separately.
- **CPU after the window closes:** zero. The log shows `live metrics sampling started` when the
  window is created and `live metrics sampling stopped after N samples` at the moment it is
  closed (for example 111 samples over a 111 second window); nothing is read afterwards.
- **Memory.** The earlier PDH read left process-wide counter tables behind after the first use,
  which kept the private working set at about 10 MB with the window open and 8 to 9 MB after it
  was closed. Reading the GPU through D3DKMT removed that. Private working set of the release
  build, before the window / window open 15 s / 10 s after close, three runs each:
  - With PDH: 2.21/10.11/8.45, 2.18/10.38/8.78 and 2.18/10.17/8.50 MB.
  - With D3DKMT: 2.32/4.76/4.69, 2.23/4.55/4.58 and 2.25/4.54/4.46 MB.

  Measured on 2026-11-10 with `pnpm tauri build --no-bundle`, launching `--background`, then a
  second launch to open the window, then `CloseMainWindow`. The 3.8 MB in the idle table was
  measured differently (30 s after close); the remaining difference from it is about 0.8 MB, with
  the window opened once.

## Frontend

- `apps/desktop/src`: React + TypeScript + Tailwind.
- Feature folders (`features/clean`, `features/memory`, ...) on top of `shared/ui` (design system)
  and `shared/ipc` (bindings to the Rust commands). Features do not import each other; a boundary
  rule (dependency-cruiser, `apps/desktop/.dependency-cruiser.cjs`, run by `pnpm check`) enforces
  it and also forbids `shared` importing features, mirroring the crate check.
- The app shell lives in `src/app` (navigation rail, section switching, Settings) and
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
  The screen carries no Sample data marker.
  In Simple mode it shows only Safe, non-admin tweaks; Advanced mode also shows risk, evidence,
  restart and the plan steps. The mode is kept in localStorage (`volt.tweaks.mode`). A risk above
  Safe, or turning off a tweak Volt did not apply, opens a confirmation dialog with the plan. The
  screen has a category filter and search. Screenshot: `docs/design/tweaks-light-460x700.png`
  (Simple mode; the dark variant is not refreshed).
- `features/clean` is the Clean page: scan, a per-rule review, confirm, and the Recycle Bin card.
  Safe rules are ticked by default; browser, shader, chat app and developer caches are not. Rules that need administrator rights
  are shown but cannot be cleaned, and a note names running browsers whose open files are skipped.
  The confirm dialog shows a note when a ticked browser rule has a `locked_note` (that browser is
  running).
  Screenshot: `docs/design/clean-light-460x700.png`. The page has three views, Junk, Large
  files and Duplicates (the tidy finder), both kept mounted so a result survives switching. The Large files view lists the biggest
  files under a chosen folder and moves ticked ones to the Recycle Bin; screenshot:
  `docs/design/clean-large-files-light-460x700.png` (sample data, mocked IPC).
- `shared/i18n` is a foundation module (see Localisation). `Shell` remounts the current page when
  the language changes, so every string is read again.
- `features/memory` is the Memory page (state, and a choice of the five purge areas; the three
  admin-only ones are disabled with the reason). Screenshot: `docs/design/memory-light-460x700.png`.
  Byte formatting lives in `shared/text/size.ts`: `formatBytes` for memory and disk, `formatSize`
  for file sizes.
- `features/home` is the Home dashboard: the health score with its explanation, and four live
  cards. See Home below.
- `features/startup` is the Startup screen (nav entry "Startup", id `startup`). It lists the entries read
  live (name, source badge, publisher or "Publisher unknown", the command shortened in the middle
  with the full text one click away, and a Switch). Loading, empty and error states, a notice for
  sources that could not be read, and a search field. The switch locks while a command runs and
  shows the state the shell reports back; a change shows a toast saying what happened and that it
  can be undone on the Undo page (there is no Undo button on the toast: the Undo page undoes the
  journal entry). Machine-wide rows carry a "Needs administrator rights" badge and a disabled
  switch; a refusal that still arrives (`accessDenied`) is explained in words and the list is
  read again. Screenshots with sample data are `docs/design/startup-light-460x700.png` and
  `startup-dark-460x700.png`.
- `features/processes` is the Processes page (id `processes`). Processes are grouped by exe name
  with CPU, memory and a count, and End task asks first in a dialog; ending `explorer.exe` gets
  its own warning. The hook polls every second at first, then every 2 seconds, and stops on
  unmount or while the window is hidden. Rows are memoised and keyed by id. There is no GPU
  figure per process. Screenshot with sample data: `docs/design/processes-light-460x700.png`.
- `features/undo` is the Undo and activity page, described under Undo page below. A tweak can
  still be undone from the Tweaks screen. Features do not import each other.
- Outside the Volt window (a plain browser) the generated bindings fail with a `TypeError` because
  the Tauri bridge is missing. The wrapper turns that into `IpcUnavailableError`, and screens say
  "not available outside the Volt app" instead of showing the raw error.
- The shell listens for the typed `boost-finished` event and shows Home with the report, so a
  tray Boost lands on Home even when the window is already open on another section.
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
    `export_activity_text`, see Undo page; `preview_undo_entry`, `history_status` and
    `clear_history` came later). They are async and run their work on the
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
    `health_report` is async and read-only: it checks the machine now and returns the score, its
    deductions and what could not be checked (see Home).
    snapshot (see Live metrics). `list_startup_entries() -> StartupList` and
    `set_startup_entry_enabled(id, enabled) -> StartupChangeResult` (see Startup apps) run on the
    blocking pool under the same lock as apply and revert; their transfer types
    (`StartupEntryView`, `StartupProblemView`, `StartupChangeStatus`, ...) are owned by the shell,
    and `StartupError` maps into `IpcError` (a machine-wide entry is `accessDenied`, built from the
    core's redacted reasons, not from port messages).
  - **Cleaning commands** (`src-tauri/src/cleaner.rs`). The plan lives in `CleanSession` inside
    `AppCore` under an id and never crosses to the UI, which sees counts, sizes and rule-relative
    samples. A scan can be cancelled (`cancelCleaningScan`). `scanCleaningRules` scans only the named
    rules; the page uses it for "Scan the rest" after a clean of truncated rules. The commands also
    choose and forget a folder, and `cleanSelectedRules` takes picked items. Rules with `requires_admin` are
    scanned but refused at clean (the elevation model is undecided). The Recycle Bin status and
    empty commands sit beside them.
  - **Apps commands** (`src-tauri/src/apps.rs`). `list_apps`, `uninstall_app` and `stop_waiting_for_uninstaller`; the UI
    sends only an id. The `AppManager` is attached only in `build_real_core` (like the Recycle Bin), never by
    `build_core`. The queue is driven by the UI (one `uninstall_app` call per app); the core allows
    one removal at a time (`Busy`).
  - **Large files commands** (`src-tauri/src/large_files.rs`). The shell keeps the scan in
    `LargeFilesSession` and sends the UI the full paths (the user asked to see them), but takes
    back only the scan id and row numbers. A trash removes the moved rows from the stored scan and
    returns the renumbered list. `reveal_large_file` runs `explorer.exe /select` on a path from the
    stored scan only. The folder dialog is opened from Rust (`pick_scan_folder`); the UI has no
    dialog permission.
  - **Tweak commands.** `get_tweak_plan` returns the plan of a tweak and `reset_tweak` turns off a
    tweak set outside Volt. `TweakInfo` carries `needsAdmin` and `appliedByVolt`;
    `NeedsAdministrator` maps to `IpcError::AccessDenied`.
  - **Memory and Boost commands.** `src-tauri/src/memory.rs` holds the transfer types and the
    `memory_state` and `clean_memory` commands, run off the main thread through
    `commands::blocking`. `boost`, `boost_config` and `set_boost_config` belong to
    `src-tauri/src/boost.rs`.
  - **Events.** Events are structs deriving `tauri_specta::Event` and registered in
    `bindings_builder`; the sample is `AppReady(AppInfo)`, listened to as `events.appReady`.
    `BoostFinished` (`events.boostFinished`, payload the Boost report) is sent when a Boost
    ends, whether it started on Home or in the tray menu. `CleanScanProgress`
    (`events.cleanScanProgress`) is sent before each rule of a cleaning scan. `MetricsUpdated` (`events.metricsUpdated`, payload
    `MetricsView`) is sent about once a second while the window is open. `MemoryTrendUpdated`
    carries the memory-by-list reading once a second under the same condition.
  - **Memory trend and exclusions.** The commands `memory_history`, `memory_exclusions`,
    `add_memory_exclusion` and `remove_memory_exclusion` serve the chart and the
    protected-programs list on the Memory page. Readings come from the sampler, which wraps its
    `MetricsSource` in `TrendSource`; it reads `Memory::lists()` after each metrics read. The cost
    is one read-only call and one event per second while a window is open, and a `MemoryHistory`
    ring of at most 300 points (about 20 KB). With no window there is no sampler, no read and no
    history.
  - **64-bit integers.** `specta-typescript` refuses to export `u64`/`i64` by default because
    JavaScript numbers lose precision above 2^53. A type that exposes one fails the export (and
    so the gate) until it is given an explicit representation, for example a decimal string.
    Registry QWORD values will need that when commands expose them. The tweak and activity
    types avoid 64-bit numbers altogether: record ids are decimal strings (and so is the cursor) and
    times are ISO 8601 UTC strings. The metrics types are the one exception: byte counts cross as
    `f64` (exact up to 2^53, and the UI has to divide and format them), never as `u64`. Cleaning,
    memory and Boost sizes cross the same way.
  - **Transfer types.** Types derive `specta::Type` in the shell, which owns them (`api.rs`: risk,
    evidence, restart, category, state, tweak info, apply and revert results, activity items). The
    domain types are not exposed wholesale, so a change in the core cannot change the UI contract by
    accident, and `volt-core` stays free of the derive.
  - **Errors from the engine.** `TweakError` maps into `IpcError` keeping the kind of the port
    failure underneath (access denied stays access denied); an unknown tweak is `notFound`, and a
    store that cannot be opened is `other`.

### Apps page

`features/apps` is the Apps page (navigation id `apps`, the seventh main section). It lists
installed and packaged apps and uninstalls ticked ones in a queue, one at a time. The queue lives
in the page: leaving the page or closing the window ends it after the running removal. The apps
not finished are kept in `localStorage` and the next open names those still installed. Removals are
not undoable, and the page and the activity record say so. Screenshot:
`docs/design/apps-light-460x700.png`.

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

### Home

`features/home` over the `health_report` command and the live metrics. Home answers how the PC is
doing and what to do about it.

- **The score card.** The score sits in the progress ring with its grade (Good from 80, Fair from
  60, otherwise Poor; the grade comes from the core) and a headline. When no check could run there
  is no ring and no number, only "Not checked". "Check again" reads the machine again and keeps the
  old report on screen until the new one arrives; a failed check keeps the last report and says so.
  The report carries the time of the check, which is shown.
- **The deductions** are listed under "What lowers the score", each with its points, its reason and
  a button to the section that fixes it: Open Cleaner, Open Memory and Open Startup go to Clean,
  Memory and Startup (the screen receives `onNavigate` and does not know the shell's
  sections). Restart Windows has no button: Volt has no restart action, so it is a sentence. A fix
  the UI does not know yet is listed without a button, never with one that does nothing. The
  screen says that the score is 100 minus the listed points. Checks that could not run are named
  in a notice and are not part of the score.
- **GPU list.** One reading per physical card: DXGI adapters are merged by PCI location (the
  D3DKMT adapter address), the listing with statistics wins, and an unknown location means no
  merge.
- **The four live cards** (CPU, memory, GPU, disk) follow `onMetricsUpdated`; the listener is
  registered first and `getLatestMetrics` is called after it, and the older reading never
  overwrites a newer one from an event. Values the shell reports as `null` are honest empty cards:
  a dash and "No reading yet" for the CPU before its second sample, "No GPU detected" on a machine
  without one, "Usage is not available" for a GPU whose driver gives no counter, never a made-up
  number. The disk card shows the system disk named by the report (the first disk, named, when the
  report is not there yet). Each part has its own loading and error state, so a failing report does
  not blank the cards and the other way round.
- **The command** (`src-tauri/src/health.rs`) reads the real registry and one sample of the
  machine on the blocking pool and returns `HealthView`, the shell's own transfer type: score,
  level, deductions (`id`, `points`, `title`, `reason`, `fix`), `unchecked`, `systemMount` and
  `checkedAt`. It does not depend on the tweak engine or on the sampler: it still works when the
  core could not start and in the first second of a window. The system disk is `SystemDrive`
  (`C:` when it is not set).
- **Boost.** One button runs the Boost use case and shows its report (bytes moved, never freed).
  The tray's Boost item runs the same code and shows the report on Home if the window is open.
- Screenshots with sample data (a mocked IPC) are in `docs/design`: `home-light-460x700.png`,
  `home-dark-460x700.png`, `home-light-1100x720.png` and `home-dark-1100x720.png`, and the list of
  deductions in the narrow window as `home-deductions-light-460x700.png` and
  `home-deductions-dark-460x700.png`.

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
- **Titles.** `api.rs` `activity_title` names a record from its label: `memory.clean`,
  `process.end`, `recycle-bin`, a cleaning rule id, a tweak id or `startup`. A standalone row
  shows `<title>: <status>`.
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
- **Two changes to one value.** An undo restores the value the journal recorded before that
  change. Before a single or batch undo, `journal::overwrites_of` looks for a later applied entry
  on the same registry value or service. `preview_undo_entry` and `preview_undo_batch` report it
  and the UI asks for a confirmation that names the later change. The core enforces it:
  `undo_entry` and `undo_batch` take the later-entry ids the caller showed the user, and if any
  overwrite found is not in that list they return `UndoStatus::NeedsAcknowledgement` with the
  overwrites and change nothing. The check runs under the exclusive lock, so a change that appears
  after the preview is refused too. UI flows pass what they displayed. A refused undo
  (`needsAcknowledgement`) reopens the overwrite warning with a fresh preview instead of only a
  notice. History rows are memoised, so a state change redraws only the affected rows.
- **Incomplete history and clearing.** When `history_status` says the history is incomplete the
  page shows a notice. A Clear button (`clear_history`, with a confirmation) removes the history
  within the retention rules above; its result also reports `kept`, counted by paging the activity
  log after the clear. After an undo the refresh pages through the log up to the
  oldest loaded row.
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

## Localisation

The UI is available in English and Vietnamese.

- **Files.** The locale files are in `apps/desktop/src/shared/i18n/locales`: `en.json` (the
  source), `vi.json` and `vi.catalogue.json` (Vietnamese for core text, filed under tweak, rule and
  category ids). The translator is `shared/i18n/translate.ts` (`t`, `tn`, `tx`, `tNodes`) and has
  no dependency.
- **Core text stays English.** `volt-core` and the catalogue TOML hold English only; never put
  Vietnamese there. The UI looks Vietnamese up by id and falls back to the core's English.
- **Phrases.** A sentence built with data is a `Phrase` (a code and parameters) and the English
  text stays in the core as the fallback. The English templates are the `ENGLISH` table in
  `volt-core` `phrase.rs`; Vietnamese is `locales/vi.phrases.json`. `check-i18n` reads the Rust
  table and fails on a missing code or different placeholders. Codes also cover Undo reasons, tweak
  previews and the tray and Boost messages; the shell writes them from the same `vi.phrases.json`
  table as the UI, with English as the fallback. The first paint on a fresh install uses
  `navigator.languages`. The activity table (schema version
  4) has a nullable `phrase` column (JSON); older records have none and show their stored English
  summary. Health
  reasons are translated by deduction id, with the figures read back out of the English sentence.
- **Errors the shell builds.** `IpcError` variants carry `message` (English) and an optional
  `phrase` (code and parameters). A sentence the shell builds uses the `err.*` codes in
  `phrase.rs` `ENGLISH`; the window writes it from `vi.phrases.json` with `say()`, and
  `describeIpcError` capitalises it and adds no prefix. An error with no phrase is a detail from
  an adapter or the OS and keeps the "Kind: message" text; an adapter `PortError` message stays
  English by design. New shell code builds a `Phrase` and calls `IpcError::invalid`, `not_found`
  and so on with it, then adds the code to `ENGLISH` and `vi.phrases.json` (`pnpm check:i18n`
  fails when either is missing). `ApplyResult` and `RevertResult` carry `detailPhrase` and
  `TweakStateView::Unreadable` carries `phrase`.
- **The setting.** The language is System, `en` or `vi`, stored under the settings key
  `ui.language` as `{choice, system}`. What System means is the display language
  from `volt-win` `display_languages()` (`GetUserPreferredUILanguages`, `MUI_LANGUAGE_NAME`); the
  shell reads it on every `language_setting` and `set_language` and the tray reads it too
  (`language.rs`), so the window only guesses from `navigator.languages` before the first answer. `localStorage` keeps
  `volt.language.system` only for the first paint. The window applies a change at once. The tray
  items are kept in managed state (`TrayItems`) and `set_language` rewrites their text at once, as
  it does the tooltip and the Boost notification.
- **Quality gate.** The `i18n` step of `pnpm check`, also run by name in CI
  (`scripts/check-i18n.mjs`, with a test), fails on a key present in only one locale, on
  placeholder mismatches, and on a tweak, rule or category without Vietnamese text.

## Testing strategy

| Level       | What                                       | How                                                                                                                                                                                                                                |
| ----------- | ------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Unit        | Core use cases, tweak engine, clean engine | In-memory fakes of the ports; runs on any OS                                                                                                                                                                                       |
| Adapter     | `volt-win`, `volt-store`                   | Real APIs against a throwaway `HKCU\Software\VoltTest` key, a temp directory and a temp database only                                                                                                                              |
| Integration | The shell's wiring and commands            | The real `WinRegistry` on a throwaway `HKCU\Software\VoltTest` key and a real store in a temp directory (apply, restart, revert, activity, undo of one entry or a batch), then the same wiring over fakes for the failure outcomes |
| UI          | Components and feature state               | Vitest, with the IPC wrapper mocked                                                                                                                                                                                                |
| End to end  | A few flows through the real app           | WebDriver, added once flows exist                                                                                                                                                                                                  |

The startup tests follow the same rule: the integration test (`tests/startup_apps.rs`) uses the
real registry adapter, a real store and two temp folders, with every Run and `StartupApproved` key
redirected to a throwaway `HKCU\Software\VoltTest\Startup-...` key. It lists, disables, reads the
state back, undoes and restarts; the real Run keys, the real approval keys and the real Startup
folders are never written, and no test or script changes a real startup entry.

Waits are bounded. `findBy` and `waitFor` share one 10 s limit in the vitest setup and test
timeouts one 30 s limit in the vitest config. Rust tests that wait on a process, pipe or job must
have a limit (the pipe tests pass a timeout to every call, the uninstaller tests run under a
watchdog). Tests do not assert tight upper bounds on elapsed time: they assert the exit reason or
result, and use a bound far below the configured limit only to show the end came from the rule and
not from the limit.

Tests never apply a real tweak, clean real user data or purge memory on the machine running them.
In particular no test applies or reverts the shipped "Show file extensions" tweak: the end to end
test uses its own catalogue whose every step targets one unique key below `HKCU\Software\VoltTest`,
and removes that key afterwards. The first run of the real flow on a real machine is a manual check
by the owner.

## Quality gates

`pnpm check` (`scripts/check.ps1`) runs one step per gate and CI runs the same script: rustfmt,
clippy, tests, bindings, architecture, privacy, i18n, Biome, typecheck, Vitest and the UI boundary rule
(11 steps);
`cargo-deny` and `typos` join when installed (CI always has them). The `i18n` step (`scripts/check-i18n.mjs`, with a test) also runs by name in CI. It does not stop at the first
failure and lists every failed step at the end. The script prints the count.

## Privacy and offline guarantee

"Volt works offline and sends nothing" is enforced, and the statement of it is
[privacy.md](privacy.md). That page ends in a machine-readable allowlist, and
`scripts/check-privacy.mjs` (the `privacy` step of `pnpm check`, also run by name in CI) compares
the repository with it in both directions, so a change that gives Volt network reach fails until
the statement is updated in the same change. It checks:

- **Capabilities and `tauri.conf.json`:** network-capable permissions (http, websocket, updater,
  upload, opener, shell, creating windows or webviews, names with fetch/connect/download),
  `remote` capabilities, remote windows or `devUrl`, remote IPC access, plugin configuration for a
  network plugin and updater artifacts.
- **Dependency graph:** `cargo metadata` for the Windows target from the workspace members over
  normal and build edges (dev-only crates are not shipped). A denylist of crates by exact name
  (HTTP clients and servers, TLS stacks, websocket, DNS resolvers, telemetry, updater and network
  Tauri plugins), the `windows` features for WinHTTP, WinInet and WebSocket, and the npm
  dependencies of the UI. Every `tauri-plugin-*` crate must be listed, network or not. Type-only
  crates (`http`, `url`) and the async runtime (`tokio`, `mio`, `socket2`, brought in by Tauri) are
  not on the list: they cannot connect by themselves.
- **Content security policy:** it must exist, have `default-src`, and contain no source other than
  keywords, `ipc:`, `asset:`, `data:`, `blob:` and loopback or `.localhost` names (so
  `http://ipc.localhost` passes, `https:`, `*` and any host fail). `devCsp` is checked when set.
- **Frontend source** (`apps/desktop/index.html` and `src`, tests excluded): `fetch`,
  `XMLHttpRequest`, `WebSocket`, `EventSource`, `sendBeacon` and similar, any remote or
  protocol-relative URL (XML namespaces and loopback excepted), imports of network packages.
- **Volt's own Rust source:** `TcpStream`, `TcpListener`, `UdpSocket`, `tokio::net`, WinHTTP,
  WinInet and similar. This is what guards the places the dependency list cannot see.

Comments are not scanned for the source checks. The checks are a net, not a proof: a determined
change can hide a request, but an accidental one cannot slip in unseen.

Facts established for this guarantee (2026-11-10): no network client, TLS or telemetry crate is in
the shipped graph; the default capability grants only event listen and unlisten; the content
security policy was tightened to `default-src 'self'` with `connect-src` limited to the local IPC
channel and `object-src`, `frame-src`, `base-uri` and `form-action` denied; the frontend has no
request API, remote URL, remote font, script or image. `tokio` is compiled with its `net` feature
(it is pulled in by Tauri and its plugins), so the capability exists in the binary; no Volt code uses it, which the
source check enforces. The run-time observation (a release build, window open for 65 s) is
recorded in [privacy.md](privacy.md): `volt-desktop.exe` held no connection, while the WebView2
runtime's own processes opened HTTPS connections to Microsoft addresses. Those are the runtime's,
are not stopped by the page's policy, and are stated as outside Volt's control rather than hidden.
For its own window only, Volt passes the runtime `--disable-component-update`,
`--disable-domain-reliability` and `--no-pings` through `additionalBrowserArgs`; the result and the
method are in privacy.md, and `scripts/observe-webview2.ps1` repeats the observation.

## Release

`.github/workflows/release.yml` builds an NSIS installer (install mode `both`: All users into Program Files\Volt with the helper, or Just me without it; see "The installer" under the helper) on a
`v*` tag or a manual dispatch; pull requests that touch the workflow or the bundle configuration
build the artifact only. A tag creates a draft GitHub release with the installer and a SHA-256
file; nothing is published automatically. There is no signing and no updater. The version comes
from the workspace `Cargo.toml`, which Tauri falls back to. `webviewInstallMode` is `skip`, so
setup makes no network call; Windows 10 without the WebView2 runtime must install it first
(see privacy.md and the README).

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
