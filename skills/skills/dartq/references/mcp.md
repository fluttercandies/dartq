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
  sweeps, batch queries, renames, test runs whose output you pipe, and
  debug loops driven from a shell (`dartq debug …`). Cheaper process,
  exact same engine.
- **Use MCP** when the host should own the interaction: long conversations
  where tool calls beat shell round-trips, or clients that cannot run
  shells.
- Both faces converge on the resident session — mixing them is fine:
  analyze in the shell, debug over MCP (or the reverse), same engine,
  same answers.

## Tool Schemas Are Self-Describing

The server's `tools/list` carries every tool's parameter schema — real,
closed schemas (`additionalProperties: false`, typed properties, the
closed vocabularies like `kind`/`role`/`verbosity`, required keys) —
generated from the same table the dispatch validates against: an
unknown or mistyped parameter answers `INVALID_PARAMS` naming the
parameter. Every tool also carries a when-to-use / not-for description,
including the full debug family (`dartq_debug_*`, the `dartq debug`
verbs' mirror). Tool arguments are snake_case (`new_name`,
`frame_index`, `session_id` where a tool names one).

Relative path arguments (`target` on `dartq_analyze`, `path` on
`dartq_impact`, `paths` on `dartq_fix`, the rename anchor) resolve
against the server's working directory — the directory the client runs
in — falling back to the workspace root only when that spelling does
not exist.

## Every Tool Answers the CLI's Own Bytes

Every `tools/call` answer rides `content[0].text` as **canonical LON** —
the same encoding the CLI's `--format lon` default prints, and the same
*value*: each tool's text is exactly the equivalent CLI command's stdout
(#2657). One LON line per document; the multi-document faces (`dartq_fix`'s
per-file patch documents, `dartq_events`' per-row stream) answer the same
lines the CLI prints, joined by `\n`. `dartq_analyze` answers the CLI's
`dartq analyze` document itself (the `root` key, root-relative files, the
CLI's row order); `dartq_impact` answers the CLI's report, environment
verdicts included; `dartq_rename` answers the CLI's rename document. The
one unit caveat: `dartq_events`' `since` floors in **milliseconds**, the
unit the rows' own `ts` field carries.

Ask the host to list tools instead of loading a catalog here; the live
answer is always current, a copied one only drifts.

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
