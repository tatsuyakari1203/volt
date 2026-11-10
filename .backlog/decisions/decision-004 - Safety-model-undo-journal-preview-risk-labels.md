---
id: decision-004
title: 'Safety model: undo journal, preview, risk labels'
date: '2026-11-10T03:01:00Z'
status: accepted
---
## Context

Optimizer tools have a poor reputation because they apply opaque changes that cannot be undone and promise benefits nobody measured. Volt's reason to exist is to be the trustworthy alternative.

## Decision

Every system change is an operation with a read-only detect step, an apply step and a revert step. The previous state is persisted to an undo journal before apply. Cleaning always produces a preview first and prefers the Recycle Bin. Every tweak carries a risk level (Safe, Caution, Advanced) and an evidence level (Measured, Anecdotal). A restore point is created before Caution and Advanced tweaks. Tests never modify the real system outside a throwaway registry key and temp directories.

## Consequences

- Features take longer: each needs a revert path and tests for it.
- Some popular tweaks will be left out because they cannot be reverted or explained.
- Permanent deletion is marked as not undoable in the Undo page.
- These rules are part of the Definition of Done for every task.
