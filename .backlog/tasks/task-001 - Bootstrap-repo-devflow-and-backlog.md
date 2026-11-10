---
id: TASK-001
title: Bootstrap repo, devflow and backlog
status: In Progress
assignee: []
created_date: '2026-11-10T03:01:00Z'
updated_date: '2026-11-10T03:02:00Z'
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
- [ ] #4 The repository is published on GitHub and CI is green on the first pull request
- [ ] #5 `main` is protected (CI required) and private vulnerability reporting is enabled
<!-- AC:END -->

## Definition of Done
<!-- DOD:BEGIN -->
- [ ] #1 `pnpm check` passes locally (fmt, clippy, tests, architecture rules, Biome)
- [ ] #2 New behaviour is covered by tests; OS-touching logic is tested through fakes of the port traits
- [ ] #3 Anything that modifies the system supports dry-run and is recorded in the undo journal
- [ ] #4 Feature matrix, docs and decision records are updated when behaviour or a decision changed
<!-- DOD:END -->

## Implementation Notes

<!-- SECTION:NOTES:BEGIN -->
2026-11-10: repo skeleton, hooks, CI workflow, .mcp.json and backlog content created locally on branch chore/bootstrap-devflow. Verified locally: pnpm check passes; check-arch rejects a forbidden dependency; commitlint rejects bad messages; both MCP servers answer tools/list. Not done: nothing is committed or pushed, so the CI workflow, cargo-deny and typos jobs have never run.
<!-- SECTION:NOTES:END -->
