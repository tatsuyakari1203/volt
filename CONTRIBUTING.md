# Contributing to Volt

Volt changes registries, services and files on real machines, so the bar for changes is
"explainable, tested and reversible".

## Setup

1. Install Rust via `rustup`, Node 22 or newer, pnpm 10 and PowerShell 7.
2. Run `pnpm install`. This installs the dev tools and the git hooks.
3. Run `pnpm check`. It should pass on a fresh clone.

## Workflow

1. Every non-trivial change starts from a GitHub issue: open one, or comment on an existing one,
   before writing code, so the approach can be agreed first. (Maintainers plan in a private
   backlog; the `TASK-NNN` references in commits point there.)
2. Branch from `main`: `type/short-slug`, for example `feat/standby-purge`.
3. Keep the layering intact (see [docs/architecture.md](docs/architecture.md) and `AGENTS.md`);
   `pnpm check:arch` enforces it.
4. Write tests. Code that touches Windows is tested through fakes of the port traits.
5. `pnpm check` must pass before you push (the pre-push hook runs it).
6. Open a pull request whose title is a Conventional Commit, for example
   `feat(core): add undo journal entries`. PRs are squash-merged.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/). Allowed scopes are listed in
`commitlint.config.mjs`. Reference the issue in a footer: `Refs: #12`.

Contributions are accepted under the MIT license, the same license as the project.

## Adding a tweak or a cleaning rule

A tweak or rule is accepted only if it has: a read-only detect step, an apply step, a revert step,
a risk level, a statement of what it improves and how that was measured (or that it is
anecdotal), and tests for all three steps.

A cleaning rule is a table in `catalogue/cleaning.toml`. Its description must say what the files
are, why removing them is safe and what the user notices afterwards, and it needs a fixture in
`apps/desktop/src-tauri/tests/cleaning_rules.rs` (entries it must take and entries it must leave
alone); a test fails when a rule has none. When in doubt, leave a location out.

## Reporting security issues

See [SECURITY.md](SECURITY.md). Please do not open public issues for vulnerabilities.
