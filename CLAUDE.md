@AGENTS.md

## Claude Code notes

- Project MCP servers are declared in `.mcp.json`: `backlog` (tasks, milestones, documents) and
  `codegraph` (code graph). Approve them when Claude Code asks on first open. Both run from
  `node_modules`, so run `pnpm install` first.
- Start a session by reading the task you are working on and the Architecture doc, not by
  scanning the repo.
- The codegraph index updates itself while its MCP server runs. After a large refactor or a
  branch switch, run `pnpm graph:sync`.
