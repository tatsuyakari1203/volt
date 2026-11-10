---
id: decision-002
title: Ports-and-adapters crate layout with enforced rules
date: '2026-11-10T03:01:00Z'
status: accepted
---
## Context

A tool that edits registries, services and files must be testable without doing those things for real, and unsafe FFI should be reviewable in one place. Without enforced boundaries, UI and OS concerns leak into the domain within a few weeks.

## Decision

Split the Rust side into `volt-core` (domain, use cases, port traits), `volt-win` (Windows adapters, the only crate with unsafe), `volt-store` (persistence adapters) and composition roots (`volt-desktop`, later `volt-cli`). `scripts/check-arch.mjs` enforces allowed internal dependencies and banned external crates per layer, in the pre-commit hook, `pnpm check` and CI. A crate without a rule fails the check.

## Consequences

- Core logic is unit-tested against in-memory fakes on any OS.
- Adding a crate or loosening a rule requires editing the rule file and recording a decision.
- Some indirection: every OS capability needs a trait before it can be used.
- The check covers direct dependencies only; transitive ones are not audited.
