---
id: TASK-001
title: Bootstrap repo, devflow and backlog
status: Done
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:11:00Z'
labels:
  - devflow
  - ci
  - docs
milestone: m-0
dependencies: []
priority: high
type: chore
ordinal: 1000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The project starts from an empty folder. Before any feature work, the owner wants a disciplined workflow: the plan in a backlog that agents can read through MCP, a code-graph tool, enforced architecture rules and quality gates, so the codebase stays clean from the first commit.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [x] #1 `pnpm check` passes on a fresh clone (rustfmt, clippy, tests, architecture check, Biome)
- [x] #2 The backlog and codegraph MCP servers start from `.mcp.json` and list their tools
- [x] #3 The plan is loaded: five documents, decision records, five milestones and tasks for every Feature Matrix row
- [x] #4 The repository is published on GitHub and CI is green on the first pull request
- [x] #5 `main` is protected (CI required) and private vulnerability reporting is enabled
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [x] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [x] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Plan

<!-- SECTION:PLAN:BEGIN -->
1. Create the public repo tatsuyakari1203/volt and push the bootstrap commit as main.
2. Read the first CI run; fix whatever fails.
3. Give the CI jobs short, stable names (Check, Supply chain) so they can be required checks.
4. Protect main (required checks, linear history, no force pushes), squash-only merges, private vulnerability reporting.
5. Land the fixes through the first pull request and confirm CI is green on it.
<!-- SECTION:PLAN:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10: repo skeleton, hooks, CI workflow, .mcp.json and backlog content created locally on branch chore/bootstrap-devflow. Verified locally: pnpm check passes; check-arch rejects a forbidden dependency; commitlint rejects bad messages; both MCP servers answer tools/list. Not done: nothing is committed or pushed, so the CI workflow, cargo-deny and typos jobs have never run.

2026-11-10: published at https://github.com/tatsuyakari1203/volt. First CI run on main (push) passed both jobs, including cargo-deny and typos which had never run before. Dependabot opened three action-bump PRs straight away; they failed only on the PR-title lint because Dependabot subjects start with a capital (commitlint subject-case). Fixed by skipping the title lint for dependabot[bot]. Jobs renamed to Check and Supply chain. Repo settings applied: squash-only merges with the PR title as commit title, delete branch on merge, private vulnerability reporting on, main protected with both checks required (not strict, so parallel PRs do not need constant rebasing), linear history, no force pushes. No LICENSE yet: TASK-002 is waiting for the owner.
<!-- SECTION:NOTES:END -->

## Final Summary

<!-- SECTION:FINAL_SUMMARY:BEGIN -->
Bootstrapped the repository: Cargo workspace skeleton with enforced crate layering, pinned dev tools, pnpm check as the single gate, lefthook hooks, GitHub Actions CI, Backlog.md and codegraph MCP servers, and the full plan loaded into the backlog. Published as a public repo with main protected by the Check and Supply chain jobs, squash-only merges and private vulnerability reporting. Verified: pnpm check locally, both CI jobs green on main and on the first pull request. Definition of Done items 2 and 3 do not apply (no behaviour and no system-changing code in this task). Open: no LICENSE yet (TASK-002).
<!-- SECTION:FINAL_SUMMARY:END -->
