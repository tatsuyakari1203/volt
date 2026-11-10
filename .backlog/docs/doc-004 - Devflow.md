---
id: doc-004
title: Devflow
type: guide
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:01:00Z'
---
How work moves from an idea to `main`. The short version for agents is in `AGENTS.md`; this is the
reference.

## Tools

| Tool | Role | Entry point |
|---|---|---|
| Backlog.md | Plan, tasks, milestones, documents, decisions, stored as Markdown in `.backlog/` | MCP server `backlog`; `pnpm exec backlog`; `pnpm board` |
| codegraph | Local code graph (callers, callees, impact) in `.codegraph/` (not committed) | MCP server `codegraph`; `pnpm exec codegraph` |
| lefthook | Git hooks | `lefthook.yml`, installed by `pnpm install` |
| commitlint | Conventional Commits | `commitlint.config.mjs` |
| Biome | Format and lint JS, TS, JSON, CSS and Markdown | `biome.json` |
| rustfmt, clippy | Format and lint Rust | `rustfmt.toml`, `[workspace.lints]` in `Cargo.toml` |
| check-arch | Crate layering rules | `scripts/check-arch.mjs` |
| cargo-deny, typos | Licenses, advisories, sources, spelling | `deny.toml`, `_typos.toml`; run in CI, and locally if installed |
| GitHub Actions | CI | `.github/workflows/ci.yml` |

Both MCP servers are declared in `.mcp.json` and run from `node_modules` (`cmd /c pnpm exec ...`),
so their versions are pinned by `pnpm-lock.yaml`.

## Lifecycle of a task

1. **Create or pick.** Work that needs thought gets a task. A task states why it exists and has
   testable acceptance criteria; it does not contain an implementation plan until someone starts
   it. Assign it to a milestone and label it by area.
2. **Start.** Set the status to In Progress, create the branch `type/task-NNN-slug`, and write the
   implementation plan into the task after reading the current code.
3. **Explore before editing.** Use codegraph (`codegraph_explore`, `codegraph_callers`,
   `codegraph_impact`) to see what depends on the code you are about to change.
4. **Implement with tests.** Stay inside the task's scope. New findings become new tasks.
5. **Check.** `pnpm check` runs rustfmt, clippy with warnings denied, tests, the architecture
   check and Biome.
6. **Finish.** Tick the acceptance criteria and Definition of Done items, add notes and a final
   summary, update the Feature Matrix or other docs, add a decision record if a decision changed.
7. **Merge.** Open a PR titled as a Conventional Commit; CI must be green; squash-merge; set the
   task to Done.

## Definition of Done (applied to every task)

- `pnpm check` passes locally.
- New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits.
- Anything that modifies the system supports dry-run and is recorded in the undo journal.
- Feature matrix, docs and decision records are updated when behaviour or a decision changed.

## Branches and commits

- Trunk-based: short-lived branches off `main`, squash-merged through PRs.
- Branch names: `feat/task-012-undo-journal`, `fix/task-031-standby-purge-privilege`,
  `chore/...`, `docs/...`.
- Commit format: `type(scope): subject`, with `Refs: TASK-NNN` in the footer.
- Types: the Conventional Commits defaults (`feat`, `fix`, `refactor`, `perf`, `test`, `docs`,
  `build`, `ci`, `chore`, `revert`, `style`).
- Scopes: `core`, `win`, `store`, `desktop`, `ui`, `cli`, `backlog`, `devflow`, `ci`, `deps`,
  `docs`, `release`.

## Hooks

| Hook | Runs |
|---|---|
| pre-commit | rustfmt check (when `.rs` staged), Biome on staged files, architecture check (when a `Cargo.toml` is staged) |
| commit-msg | commitlint |
| pre-push | `pnpm check` (everything) |

## CI

`.github/workflows/ci.yml`:

- **check** (Windows): `pnpm install --frozen-lockfile`, installs the pinned Rust toolchain, runs
  `pnpm check`, and lints the PR title.
- **supply-chain** (Linux): cargo-deny and typos.

Planned, tracked as tasks: frontend typecheck and tests once the UI exists, a release workflow
with `tauri-action`, and changelog automation.

## Decisions

Architecture and process decisions are recorded with `pnpm exec backlog decision create "Title"`.
The CLI and MCP server cannot edit a decision's body, so fill in Context, Decision and
Consequences in the file directly. A decision is needed when adding a crate, changing a
dependency rule, adopting a tool, or choosing between approaches that are costly to reverse.

## Adding a crate

1. Record the decision.
2. Create `crates/<name>` with `[lints] workspace = true`.
3. Add its rule to `RULES` in `scripts/check-arch.mjs` and to the table in the Architecture doc.
4. Add its scope to `commitlint.config.mjs` if it needs one.

## Keeping the tools healthy

- `pnpm exec backlog doctor` checks for duplicate IDs and dependency cycles in the backlog.
- `pnpm exec codegraph status` shows whether the index is current; `pnpm graph:sync` updates it.
  The index also syncs itself while the MCP server is running.
- Tool versions are pinned in `package.json` and `rust-toolchain.toml`; Dependabot proposes
  updates weekly.
