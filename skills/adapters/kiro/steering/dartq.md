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

Skill file (one skill, reference docs per face — load the matching one
before the task):

- `dartq` — the skill: contract, verb surface, and a router into
  `references/` (getting-started, analysis-loop, test-driving,
  sessions-and-monorepos, debugging, mcp)

To expose dartq as MCP tools, register the stdio server in
`.kiro/settings/mcp.json` (workspace) or `~/.kiro/settings/mcp.json`
(user):

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```
