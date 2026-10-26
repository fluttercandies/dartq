# MCP Integration

## One-Line Setup

`dartq mcp` is a stdio MCP server; every tool mirrors a CLI verb and
converges on the worktree's resident session — one engine for CLI, events,
and MCP clients alike:

```bash
dartq mcp          # serve the current worktree (DIR argument optional)
```

Registration per host:

```bash
claude mcp add dartq -- dartq mcp                 # Claude Code
gemini mcp add dartq -- dartq mcp                 # Gemini CLI
```

Config-file hosts (stdio shape is universal):

```json
{ "mcpServers": { "dartq": { "command": "dartq", "args": ["mcp"] } } }
```

One server per worktree: run the registration from inside the worktree (or
pass its path as the `DIR` argument) so tools bind the right session.

## CLI vs MCP: When to Use Which

- **Use the CLI** for one-shot, scriptable, or CI-shaped work — analysis
  sweeps, batch queries, renames, test runs whose output you pipe. Cheaper
  process, exact same engine.
- **Use MCP** when the host should own the interaction: long conversations
  where tool calls beat shell round-trips, or clients that cannot run
  shells (debug orchestration, event polling).
- Both faces converge on the resident session — mixing them is fine:
  analyze in the shell, debug over MCP, same engine, same diagnostics.

## Tool Catalog (28 tools)

Analysis and navigation: `dartq_analyze`, `dartq_query` (kind:
symbol/references/callers/implementations/dependencies), `dartq_explain`,
`dartq_impact`, `dartq_fix`.

Workspace edits: `dartq_rename` — runs a cold in-process analysis pass per
call (a rare operation), expect it slower than the session-resident tools;
returns the machine edit set, the caller applies it.

Tests: `dartq_test` (run), `dartq_test_status` (poll in-flight and finished
runs), `dartq_test_events` (cursor-paged results of one run),
`dartq_test_cancel`.

Observability: `dartq_events` — the session's unified event stream or its
cold-analysis progress (`handle`: `session` | `progress`; `since`, `limit`)
— the "is it still working" tool during a long first analysis.

Debugging (17 tools): `dartq_debug_launch`, `_attach`, `_detach`, `_stop`,
`_status`, `_breakpoint_add`, `_breakpoint_remove`, `_exception_pause`,
`_resume`, `_pause`, `_step`, `_snapshot`, `_reload`, `_restart`,
`_evaluate`, `_events`, `_output`. Semantics: [debugging.md](debugging.md).

The full per-tool descriptions (with use-for / not-for lines):
[tool-catalog.md](tool-catalog.md).

## Resource

The server publishes one resource:

- `dartq://workspace-status` — the bound worktree, its layout, and the
  session attach state (the degradation reason when not attached)

Read it first in a fresh MCP session to confirm the binding.

## Troubleshooting

| Symptom | Diagnosis |
| --- | --- |
| Tools slow on first call | cold analysis — poll `dartq_events` (`progress`) |
| `dartq_rename` much slower than others | expected: cold pass per call |
| Wrong workspace answered | the server bound a different worktree — check `dartq://workspace-status`; restart `dartq mcp` inside the right tree |
| Session degraded to standalone | unwritable or foreign worktree — printed, not an error; see [sessions-and-monorepos.md](sessions-and-monorepos.md) |
