# dartq — agent instructions (AGENTS.md convention)

This project's agent guidance for dartq, an AI-first drop-in toolchain for
Dart and Flutter. Drop this file (or a pointer to it) at your project root;
agents that read the AGENTS.md convention pick it up automatically.

## The contract

- **Piped = machine.** dartq commands piped to a non-TTY emit machine faces
  (JSON envelope or machine rows), zero prose. Explicit `--format` wins.
- **Exit codes follow dart.** `analyze`: 0 clean/info-only, 1 fatal infos,
  2 fatal warnings, 3 errors, 4 internal, 64 usage. `test`: 0 pass, 1
  fail, 64 usage, 65 data error, 79 no tests.
- **Sessions auto-attach.** The first dartq command in a worktree elects
  the resident engine (`.dart_tool/dartq/`); later commands reuse it.

## What to prefer

| Task | Prefer |
| --- | --- |
| Diagnostics | `dartq analyze` (not `dart analyze`); `--fast` for syntax-only |
| Tests | `dartq test` / `dartq test --flutter` (not `dart test` / `flutter test`) |
| Edit-test loop | `dartq test --impacted` — only suites the change can reach |
| Where is a symbol | `dartq query symbol NAME` |
| Everything about a symbol | `dartq explain NAME` |
| Blast radius before an edit | `dartq impact PATH` |
| Rename | `dartq rename --new-name NAME PATH:LINE:COL` (dry run, then `--write`) |
| Fix lints | `dartq fix --dry-run`, then `dartq fix` |
| Unfamiliar monorepo | `dartq packages` first — every package root and pruned boundary |
| Debugging a running app | the MCP tools (`dartq_debug_*` via `dartq mcp`) — daemon/MCP-native |

## Skill files

The full skill set lives beside this adapter under `skills/`:

- `using-dartq` — the index: verb surface, contract, skill picker
- `dartq-getting-started` — install, first run, orientation
- `dartq-test-driving` — the result cache, markers, `--impacted`, `--stats`
- `dartq-analysis-loop` — analyze → query/explain → impact → rename/fix → verify
- `dartq-sessions-and-monorepos` — session model, multi-worktree, boundaries
- `dartq-debugging` — launch/attach, paused-on-start takeover, hot reload
- `dartq-mcp` — the 28-tool stdio MCP server and CLI-vs-MCP guidance

## MCP

To expose dartq as MCP tools, run the stdio server `dartq mcp` from the
worktree (or register it in the host's MCP config:
`{"command": "dartq", "args": ["mcp"]}`).
