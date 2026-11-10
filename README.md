<div align="center">

# Volt

**An open-source Windows optimizer you can audit and undo.**

Free memory, clean junk, control startup and apply system tweaks, with every change previewed,
journaled and reversible.

[![CI](https://github.com/tatsuyakari1203/volt/actions/workflows/ci.yml/badge.svg)](https://github.com/tatsuyakari1203/volt/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
![Status: pre-alpha](https://img.shields.io/badge/status-pre--alpha-orange)
![Platform: Windows 10 and 11](https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4)
![Built with Tauri 2 and Rust](https://img.shields.io/badge/built%20with-Tauri%202%20%2B%20Rust-24C8DB)

</div>

> [!WARNING]
> Volt is in **pre-alpha**. There is no installable release yet and nothing here should be run
> on a machine you care about. The foundation (domain core, safety model, desktop shell) is being
> built in the open. "Volt" is a working name.

## Why Volt

Most PC optimizers are closed source, ad supported, and apply changes that nobody can explain or
take back. Volt is built on the opposite assumptions:

| Principle      | What it means in practice                                                                                                                                           |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Reversible** | Every change records the previous state before it is applied, and can be rolled back, one change or a whole batch, even after a restart.                            |
| **Previewed**  | Nothing happens blind. Tweaks show current and target values; cleaning lists the files and their size, and moves them to the Recycle Bin.                           |
| **Honest**     | Each tweak carries a risk level (Safe, Caution, Advanced) and states whether its benefit is measured or anecdotal. Tweaks that cannot be explained are not shipped. |
| **Quiet**      | No telemetry, no ads, works offline, and stays small when idle.                                                                                                     |

## Privacy

Volt works offline and sends nothing.

- **What it does:** reads your system state and changes it only when you confirm. Its undo journal,
  settings and a small diagnostic log stay on your machine.
- **What it never sends:** no telemetry, analytics, crash reports, update checks, ads or accounts,
  and no file names, registry values or logs. The app has no network permission and its content
  security policy allows no remote origin.
- **What is outside its control:** the Microsoft WebView2 runtime that draws the window. It has its
  own updater and settings, and in a one-minute test it made a few HTTPS connections to Microsoft
  on its own while Volt's own process made none. Also Windows itself and any build you did not
  make from this source.
- **It is checked, not just promised:** `pnpm check` and CI fail if a network permission, a
  network-capable crate or package, a remote origin or a remote request is added without
  [docs/privacy.md](docs/privacy.md) changing in the same commit. That page has the exact
  statement, the allowlist the check reads and how the running app was observed.

## Planned features

Volt aims to cover what Microsoft PC Manager does and go deeper where it stops.

| Area                      | Scope                                                                                                                                                |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Memory**                | Working-set trim, standby list purge, modified list flush and file cache trim, with thresholds for automatic cleaning                                |
| **Cleanup**               | Temp files, Windows Update leftovers, browser caches per profile, shader caches, and developer and AI caches (npm, pip, cargo, Docker, Hugging Face) |
| **Disk**                  | Fast MFT-based disk analyzer with a treemap, large file and duplicate finders                                                                        |
| **Startup and processes** | Startup apps including hidden entries (scheduled tasks, services), process control with persistent priority and affinity                             |
| **Tweaks**                | A declarative catalogue for performance, privacy, appearance, gaming and network, grouped into profiles                                              |
| **Apps**                  | Batch uninstall, leftover detection, update everything through winget                                                                                |
| **Health**                | An explainable health score, disk SMART data, guided repair tools                                                                                    |

All of the above is planned work. See [Project status](#project-status) for what exists today.

## How it works

Volt keeps the rules about _what_ to change separate from the code that _does_ it, so the logic
can be tested without touching a real system and unsafe Windows calls stay in one place.

```
            desktop app (Tauri shell + React UI)          CLI (planned)
                          │                                    │
            ┌─────────────┴────────────────┬───────────────────┘
            │                              │
        volt-win                      volt-store
   Windows API adapters            SQLite undo journal
   (the only unsafe code)             and settings
            │                              │
            └──────────────┬───────────────┘
                           │
                       volt-core
        domain model, use cases and port traits
```

- **`volt-core`** describes a change as data (an _operation_), groups operations into a _plan_
  with a preview, and applies plans through an executor that enforces write-ahead journaling:
  detect the prior state, persist it, and only then change the system.
- **`volt-win`** and **`volt-store`** implement the core's port traits against Windows and
  SQLite.
- The layering is enforced in CI: a crate that depends on the wrong layer fails the build.

Read [docs/architecture.md](docs/architecture.md) for the full picture.

## Project status

| Milestone  | Goal                                                                                            | State       |
| ---------- | ----------------------------------------------------------------------------------------------- | ----------- |
| Foundation | Domain core, undo journal, Windows adapters, tweak engine, clean engine, desktop shell          | In progress |
| v0.1 MVP   | Boost, deep memory cleaning, core cleanup, startup, processes, first tweak catalogue, Undo page | Planned     |
| v0.2       | Extended cleaning rules, disk analyzer, uninstaller, localisation                               | Planned     |
| v0.3       | Automation, profiles, services, disk health, CLI                                                | Planned     |
| v1.0       | Repair toolkit, installer, auto-update, signed releases                                         | Planned     |

## Tech stack

| Layer             | Technology                                                                          |
| ----------------- | ----------------------------------------------------------------------------------- |
| Shell             | [Tauri 2](https://v2.tauri.app) (WebView2)                                          |
| Core and adapters | Rust 2024, the `windows` crate, SQLite                                              |
| UI                | React, TypeScript, Tailwind CSS, Vite                                               |
| Quality           | rustfmt, clippy (warnings denied), Biome, Vitest, cargo-deny, an architecture check |

## Development

**Requirements:** Windows 10 or 11, [Rust](https://rustup.rs) (the pinned toolchain installs
itself), Node 22 or newer, pnpm 10, PowerShell 7 and the WebView2 runtime (included in
Windows 11).

Install the dev tools and git hooks:

```bash
pnpm install
```

Run the desktop app in development mode:

```bash
pnpm dev
```

Run every gate that CI runs (format, lint, tests, architecture rules):

```bash
pnpm check
```

### Repository layout

```
apps/desktop/        Tauri shell and React UI
crates/volt-core/    domain model, use cases, port traits, in-memory fakes
crates/volt-win/     Windows adapters
crates/volt-store/   persistence adapters
docs/                architecture and design notes
scripts/             quality gates
```

## Contributing

Contributions are welcome once the foundation settles; until then, issues and discussion are the
most useful way to help. Please read [CONTRIBUTING.md](CONTRIBUTING.md) first. The short version:
changes must be explainable, tested and reversible, and `pnpm check` must pass.

Found a vulnerability? Please report it privately as described in [SECURITY.md](SECURITY.md).

## License

Volt is released under the [MIT license](LICENSE).
