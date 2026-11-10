# Volt: guide for coding agents

Volt is an open-source Windows optimizer (memory, cleanup, startup, reversible tweaks) built with
Tauri 2, a Rust core and a React + TypeScript UI. Status: pre-alpha, foundation phase. The repo
currently holds the devflow, the plan and empty crate skeletons.

## Sources of truth

| What                            | Where                                  | How to access                                         |
| ------------------------------- | -------------------------------------- | ----------------------------------------------------- |
| Plan, tasks, milestones, docs   | `.backlog/` (Backlog.md)               | MCP server `backlog`, or `pnpm exec backlog ...`      |
| Decisions (ADRs)                | `.backlog/decisions/`                  | `pnpm exec backlog decision list`, then read the file |
| Code structure, callers, impact | codegraph index (`.codegraph/`, local) | MCP server `codegraph`, or `pnpm exec codegraph ...`  |
| Quality gates                   | `scripts/check.ps1`                    | `pnpm check`                                          |

Backlog docs to read before non-trivial work: **Product Plan**, **Feature Matrix**,
**Architecture**, **Devflow**, **Resources** (`backlog` MCP `document_list` / `document_view`).

Never edit task, milestone or document files under `.backlog/` by hand: use the MCP tools or the
CLI so IDs and metadata stay consistent. Decision files are the exception: the CLI can only create
them, so their body is edited directly.

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

- English for code, comments, commits, docs and UI strings.
- Rust 2024 edition, toolchain pinned in `rust-toolchain.toml`. Lints live in the workspace
  `Cargo.toml`; do not silence them with `#[allow]` without a reason in a comment.
- No `unwrap()` outside tests. Errors are typed values that reach the UI.
- Never commit secrets, signing keys or `.env` files.
