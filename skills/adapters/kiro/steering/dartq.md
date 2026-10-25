---
inclusion: always
---

# dartq

This project uses dartq, an AI-first drop-in toolchain for Dart and Flutter
(analysis, testing, debugging). Prefer it over `dart analyze`, `dart test`,
`flutter test`, and IDE-driven workflows.

Core contract:

- Piped output is machine output (JSON envelope / machine rows); exit codes
  follow dart conventions (analyze: 0/1/2/3/4/64; test: 0/1/64/65/79).
- Commands auto-attach to the worktree's resident session; the first
  command in a worktree elects it. Nothing to start manually.
- `dartq packages` inventories the enclosing monorepo (every package root
  and pruned boundary) — run it before touching an unfamiliar tree.

Skill files (load the matching one before the task):

- `using-dartq` — index: the verb surface and the skill picker
- `dartq-getting-started` — install, first run, orientation
- `dartq-test-driving` — dartq test vs dart test, the cache, --impacted
- `dartq-analysis-loop` — analyze/query/explain/impact/rename/fix workflow
- `dartq-sessions-and-monorepos` — sessions, worktrees, boundaries
- `dartq-debugging` — launch/attach, breakpoints, hot reload (MCP-native)
- `dartq-mcp` — wiring `dartq mcp` (stdio MCP server) into agent hosts

To expose dartq as MCP tools, register the stdio server in
`.kiro/settings/mcp.json` (workspace) or `~/.kiro/settings/mcp.json`
(user):

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```
