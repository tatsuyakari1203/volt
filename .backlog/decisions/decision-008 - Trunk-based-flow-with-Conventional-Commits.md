---
id: decision-008
title: Trunk-based flow with Conventional Commits
date: '2026-11-10T03:01:00Z'
status: accepted
---
## Context

A solo owner working with agents needs a flow that is light but leaves a clean, automatable history, and gates that catch problems before they reach `main`.

## Decision

Short-lived branches named `type/task-NNN-slug` off `main`, merged by squash through pull requests whose title is a Conventional Commit. commitlint checks messages; lefthook runs fast checks before commit and the full `pnpm check` before push; CI repeats the full check on Windows and runs cargo-deny and typos. Commits reference their task with a `Refs: TASK-NNN` footer.

## Consequences

- History on `main` is one commit per task, suitable for generated changelogs.
- Every change, including the owner's, goes through a PR and CI.
- The pre-push hook adds time to each push as the test suite grows; it can be narrowed later if it becomes a problem.
