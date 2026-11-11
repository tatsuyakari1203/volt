# Volt: guide for coding agents

Volt is an open-source Windows optimizer (memory, cleanup, startup, reversible tweaks) built with
Tauri 2, a Rust core and a React + TypeScript UI. Status: pre-alpha, foundation phase.

## Sources of truth

| What                            | Where                                         | How to access                                        |
| ------------------------------- | --------------------------------------------- | ---------------------------------------------------- |
| Architecture                    | `docs/architecture.md`                        | Read it before non-trivial work                      |
| Plan, tasks, decisions          | `.backlog/` (Backlog.md), private, not in git | MCP server `backlog`, or `pnpm exec backlog ...`     |
| Code structure, callers, impact | codegraph index (`.codegraph/`, local)        | MCP server `codegraph`, or `pnpm exec codegraph ...` |
| Quality gates                   | `scripts/check.ps1`                           | `pnpm check`                                         |

The backlog is the maintainers' internal plan. It lives in `.backlog/` in the maintainer's main
checkout, is ignored by git and is never committed or quoted wholesale into public files. If
`.backlog/` does not exist in your checkout, you are working as an outside contributor: skip the
backlog steps below and work from the GitHub issue instead.

Never edit task, milestone or document files under `.backlog/` by hand: use the MCP tools or the
CLI so IDs and metadata stay consistent. Decision files are the exception: the CLI can only create
them, so their body is edited directly. In a git worktree there is no `.backlog/`: run the CLI
from the main checkout.

## The loop for every task

1. **Pick**: find the task (`task_list`, `task_search`). No task for non-trivial work? Create one
   first (read `get_backlog_instructions` for the task-creation guide).
2. **Branch**: `type/task-NNN-short-slug` from `main` (for example `feat/task-012-undo-journal`).
3. **Understand**: before changing existing code, ask codegraph what depends on it
   (`codegraph_explore`, `codegraph_callers`, `codegraph_impact`). Do not guess at blast radius.
4. **Plan**: set the task to In Progress and record the implementation plan in the task.
5. **Build**: implement with tests. Keep the change inside the task's scope; new ideas become new
   tasks.
6. **Verify**: `pnpm check` must pass. It runs rustfmt, clippy, tests, the architecture check and
   Biome.
7. **Record**: tick acceptance criteria, add implementation notes and a final summary to the task;
   update docs or add a decision if behaviour or a decision changed.
8. **Commit**: Conventional Commits with a scope from `commitlint.config.mjs` and a
   `Refs: TASK-NNN` footer. Commit only when the owner asks. Merge to `main` through a PR with a
   squash merge; the PR title is a Conventional Commit.

## Keeping the loop fast

- A task delivers something a user can do, across the layers it needs (core, adapter, IPC, UI).
  Do not split one feature into a task per layer.
- While iterating, run targeted commands (`cargo test -p <crate> <filter>`,
  `pnpm --filter @volt/desktop exec vitest run <file>`). Run the full `pnpm check` once, before
  pushing; the pre-push hook runs it as well.
- Agents that work in parallel reuse the worktree slots under `.claude/worktrees/slot-N` (ignored
  by git), so the build cache stays warm. Any other worktree is removed once its pull request is
  merged.
- Parallel changes land on `main` one at a time, each as soon as it is reviewed. Never hand-merge
  lock files or the generated IPC bindings: take the version on `main` and regenerate.

## Architecture rules

```
volt-desktop (Tauri shell), volt-cli      composition roots
        |                |
   volt-win          volt-store            adapters (Windows API, SQLite)
        \                /
           volt-core                       domain, use cases, port traits
```

- `volt-core` depends on no other workspace crate and never on `tauri`, `windows`,
  `windows-sys`, `winreg` or `rusqlite`. It talks to the outside world only through traits.
- `volt-win` and `volt-store` depend on `volt-core` only and implement its traits.
- Only composition roots know every crate. UI code never contains business rules.
- `unsafe` is allowed in `volt-win` only, and every block carries a `// SAFETY:` comment.
- `scripts/check-arch.mjs` enforces the layering. Adding a crate means adding a rule there and
  recording a decision.

## Safety rules for code that changes a system

Volt edits registries, services, files and memory on real machines. Treat that as hazardous.

- Tests never touch the real system. Core logic is tested against in-memory fakes of the ports;
  adapter tests may only use a throwaway key under `HKCU\Software\VoltTest` and a temp directory.
- Never run a tweak, cleaner or memory purge against the developer's machine from a test, a
  script or an ad-hoc command unless the owner asked for that exact run. Dry-run first.
- Every operation has three parts: detect (read-only), apply, revert. Apply records the previous
  value in the undo journal before it changes anything.
- Cleaning shows a preview (files and sizes) before deleting, and prefers the Recycle Bin.
- Every tweak declares a risk level (Safe, Caution, Advanced) and whether its benefit is
  measured or anecdotal. Do not add tweaks whose effect cannot be explained.
- No telemetry, no network calls unless a feature needs them and says so in the UI.
- The elevated CI job (`.github/workflows/elevated-helper.yml`) is the only place where a test may
  write under Program Files, install or start the helper, trigger an elevation or purge memory. It
  runs on GitHub-hosted runners only (throwaway virtual machines), and its harness
  (`volt-elevated-ci`, behind the `elevated-ci` feature) refuses to run anywhere else. Never run
  that harness, or set its switches, on a developer machine.

## Commands

| Command                               | Purpose                         |
| ------------------------------------- | ------------------------------- |
| `pnpm install`                        | Install dev tools and git hooks |
| `pnpm check`                          | Run every gate CI runs          |
| `pnpm format`                         | Auto-format (Biome and rustfmt) |
| `pnpm check:arch`                     | Crate layering check only       |
| `pnpm board`                          | Open the Backlog web UI         |
| `pnpm exec backlog task list --plain` | List tasks from the CLI         |
| `pnpm exec codegraph status`          | Check the code graph index      |

## Conventions

- English is the source language for code, comments, commits, docs and UI strings. Other languages
  live only in locale files (`apps/desktop/src/shared/i18n/locales`): UI text comes from there
  through `t`, and adding a string means adding its key to `en.json` and `vi.json`, nothing else
  (`pnpm check` fails when they differ). Text the core writes stays English in the core; Vietnamese
  for a tweak or cleaning rule goes in `vi.catalogue.json`, filed under its id.
- Rust 2024 edition, toolchain pinned in `rust-toolchain.toml`. Lints live in the workspace
  `Cargo.toml`; do not silence them with `#[allow]` without a reason in a comment.
- No `unwrap()` outside tests. Errors are typed values that reach the UI.
- Never commit secrets, signing keys or `.env` files.
