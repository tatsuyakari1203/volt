---
id: decision-005
title: Backlog.md as the single source of truth for planning
date: '2026-11-10T03:01:00Z'
status: accepted
---
## Context

The project is built mostly by coding agents across many sessions. Plans kept in chat or in an external tracker are lost between sessions and are invisible to contributors. The owner already uses a `.backlog/` folder in another project.

## Decision

Keep the plan, feature matrix, architecture, devflow, resources, decisions, milestones and tasks in `.backlog/` using Backlog.md, pinned as a dev dependency. Agents use its MCP server (declared in `.mcp.json`); humans use the CLI or `pnpm board`. Task, milestone and document files are changed only through the tool. Decision bodies are edited by hand because the tool cannot edit them.

## Consequences

- The plan is versioned with the code and reviewable in pull requests.
- No external service or account is needed.
- GitHub Issues remain for outside reports; accepted ones become backlog tasks.
- Task titles must stay short: long titles can exceed path limits on Windows.
