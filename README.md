# Volt

An open-source Windows optimizer: free memory, clean junk, manage startup and apply system tweaks
that can always be undone.

> **Status: pre-alpha.** There is no usable build yet. The repo currently contains the project
> plan, the development workflow and empty crate skeletons. "Volt" is a working name.

## Why

Most "PC optimizer" tools are closed, ad-supported, or apply changes nobody can explain or undo.
Volt aims to be the opposite:

- **Reversible.** Every change records the previous value and can be rolled back.
- **Previewed.** Cleaning lists exactly what will be removed, and how much space it frees, first.
- **Honest.** Each tweak states its risk and whether its benefit is measured or anecdotal.
- **Quiet.** No telemetry, no ads, works offline, and stays small when idle.

## Plan

The plan lives in the repo, managed with [Backlog.md](https://github.com/MrLesk/Backlog.md):

- Documents: [`.backlog/docs/`](.backlog/docs) (product plan, feature matrix, architecture,
  devflow, resources)
- Decisions: [`.backlog/decisions/`](.backlog/decisions)
- Tasks: [`.backlog/tasks/`](.backlog/tasks), or run `pnpm board` for a Kanban view

## Stack

Tauri 2, Rust (domain core plus Windows adapters), React, TypeScript and Tailwind. Windows 10 and
11, x64 first.

## Development

Requirements: Windows, Rust (the pinned toolchain installs itself through `rustup`), Node 22.18 or
newer, pnpm 10, PowerShell 7.

```bash
pnpm install
```

```bash
pnpm check
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) for the workflow and the
architecture rules.

## License

Not chosen yet (tracked as a decision in `.backlog/decisions/`). Until a `LICENSE` file is added,
no license is granted.
