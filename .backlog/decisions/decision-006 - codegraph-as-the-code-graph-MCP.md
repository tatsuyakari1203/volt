---
id: decision-006
title: codegraph as the code-graph MCP
date: '2026-11-10T03:01:00Z'
status: accepted
---
## Context

The owner asked for a code-graph tool (GitNexus or codegraph) so that changes are made with knowledge of callers and impact from the start. Both were evaluated on 2026-11-10, along with codebase-memory-mcp, Serena and CodeGraphContext; details are in the Resources document.

## Decision

Adopt `@colbymchenry/codegraph` (MIT), pinned as a dev dependency and exposed through `.mcp.json` with the explore, node, search, callers, callees, impact, files and status tools enabled and telemetry disabled. Do not adopt GitNexus for now.

## Consequences

- No license restriction if the project is ever commercialised; GitNexus is PolyForm Noncommercial.
- Small footprint and automatic index sync; the index lives in `.codegraph/` and is not committed.
- GitNexus has tools codegraph lacks (change detection, graph-assisted rename). Revisit if those are missed.
- The quality of Rust, TSX and Tauri `invoke()` resolution is unverified until real code exists.
