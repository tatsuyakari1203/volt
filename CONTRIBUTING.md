# Contributing to Volt

Volt changes registries, services and files on real machines, so the bar for changes is
"explainable, tested and reversible". The full workflow is in the **Devflow** document under
`.backlog/docs/`; this page is the short version.

## Setup

1. Install Rust via `rustup`, Node 22.18 or newer, pnpm 10 and PowerShell 7.
2. Run `pnpm install`. This installs the dev tools and the git hooks.
3. Run `pnpm check`. It should pass on a fresh clone.

## Workflow

1. Every non-trivial change starts from a task in `.backlog/tasks/` (`pnpm board` to browse).
2. Branch from `main`: `type/task-NNN-short-slug`.
3. Keep the layering intact (see `AGENTS.md`); `pnpm check:arch` enforces it.
4. Write tests. Code that touches Windows is tested through fakes of the port traits.
5. `pnpm check` must pass before you push (the pre-push hook runs it).
6. Open a pull request whose title is a Conventional Commit, for example
   `feat(core): add undo journal entries`. PRs are squash-merged.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/). Allowed scopes are listed in
`commitlint.config.mjs`. Reference the task in a footer: `Refs: TASK-012`.

## Adding a tweak or a cleaning rule

A tweak or rule is accepted only if it has: a read-only detect step, an apply step, a revert step,
a risk level, a statement of what it improves and how that was measured (or that it is
anecdotal), and tests for all three steps.

## Reporting security issues

See [SECURITY.md](SECURITY.md). Please do not open public issues for vulnerabilities.
