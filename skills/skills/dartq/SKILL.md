---
name: dartq
description: Use when working in any Dart or Flutter codebase — before reaching for dart analyze, dart test, flutter test, or an IDE — for analyzing diagnostics, navigating or explaining symbols, scoping a change's blast radius, renaming or applying fixes, running or accelerating tests, debugging or hot-reloading a running app, managing resident sessions across worktrees and monorepos, wiring dartq as MCP tools for an agent host, or installing dartq and verifying a setup
---

# dartq

dartq is a drop-in Rust toolchain for Dart and Flutter analysis, testing, and
debugging. AI agents are its first-class users: every output-bearing verb
defaults to LON (the compact canonical encoding of the JSON data model) over
a short snake_case field vocabulary, every face has a stable schema, and one
resident engine per worktree serves CLI, events, and MCP clients alike.

## The Contract (memorize these three lines)

1. **`--format lon` is the default — always.** One canonical LON line (or
   one per row), never sniffing the terminal: a piped run and a TTY run
   print the same bytes. `--format json` asks for the frozen single-line
   JSON shapes (jq consumers); `--format text` the human faces (write
   operations fall back to lon — their output is a record, not prose).
2. **Exit codes follow dart.** `analyze`: 0 clean/info-only, 1 fatal infos,
   2 fatal warnings, 3 errors, 4 internal failure, 64 usage. `test`: dart
   test's own codes (0 pass, 1 fail, 64 usage, 65 data error, 79 no tests).
3. **Commands auto-attach to the worktree's session.** The first command in a
   worktree elects a resident engine (`.dart_tool/dartq/`); later commands
   reuse it. It exits after 15 min with no client, on eviction past the
   5-session-per-user cap, or when a newer dartq binary wins the next
   election — but a daemon holding debug sessions never idle-reaps, and
   every exit (SIGTERM/SIGINT, the election takeover included) runs a
   graceful shutdown: paused isolates are resumed first, a
   `session`/`shutdown` event records what was released, and re-attach
   manifests stay under `.dart_tool/dartq/debug/` (listed as `orphans` by
   `dartq debug status`).

## The Verb Surface

| Verb | One line |
| --- | --- |
| `analyze` | Full-engine diagnostics; `--fast` = parser-only sweep |
| `packages` | Every package root and pruned boundary of the enclosing monorepo |
| `query` | Symbol index: declarations, references, callers, implementations, dependencies, batch |
| `explain` | JSON report for one symbol (sections `declaration bases ifaces ctors methods refs deps tests`) or one diagnostic code |
| `impact` | What a change to one file would affect |
| `rename` | Edit-set rename: the declaration's name token plus the reference rows the index resolves; dry run or `--write` |
| `fix` | Emit or apply anchored fix patches; `--role` picks the candidate kind an apply writes |
| `test` | `dart test` / `flutter test` with a result cache and `--impacted` |
| `session` | Manage the resident engine: start / status / stop |
| `events` | Newest-first session log or cold-analysis progress |
| `debug` | Drive debug sessions: launch/attach, breakpoints, stepping, snapshots, evaluation, hot reload/restart |
| `resolve` / `complete` | One-shot `PATH:LINE:COL` cursor answers |
| `compat` | One workspace under several SDK roots: a compatibility matrix |
| `mcp` | stdio MCP server mirroring the verbs (28 tools) |

Run `dartq <verb> --help` for the full grammar of any face.

## Where the Depth Lives

This file is the whole skill's entry point; each face has a reference doc
under `references/`. Load the one that matches the task:

| When the task is... | Read |
| --- | --- |
| First contact: install, first run, orienting in a repo | [references/getting-started.md](references/getting-started.md) |
| Running tests, speeding up edit-test loops, cache questions | [references/test-driving.md](references/test-driving.md) — depth: [references/cache-and-markers.md](references/cache-and-markers.md) |
| Diagnose → understand → scope → edit → verify on analyzed code | [references/analysis-loop.md](references/analysis-loop.md) — depth: [references/formats-and-tiers.md](references/formats-and-tiers.md) |
| Nested worktrees, monster monorepos, multi-checkout parallel work | [references/sessions-and-monorepos.md](references/sessions-and-monorepos.md) — depth: [references/session-model.md](references/session-model.md) |
| Launching, pausing, inspecting, hot-reloading a running app | [references/debugging.md](references/debugging.md) — the `dartq debug` verb group |
| Wiring dartq into an agent host as MCP tools | [references/mcp.md](references/mcp.md) |

## Defaults That Surprise Newcomers

- `dartq analyze` prints `{v:1 root? diags:[{code sev file line col len msg}]}` —
  one LON document, one flat row per diagnostic; `root` (the absolute
  analysis root) rides only when rows exist, and `file` is root-relative
  inside it, absolute outside (SDK, pub cache). `--format json` asks for the
  oracle envelope (byte parity with `dart analyze --format=json` — absolute
  paths, `documentation` on lint rows); `--format text` the grouped human
  listing under a leading `root: <abs>` line.
- The lon face carries the short field vocabulary everywhere: `v`, `diags`,
  `sev`, `msg`, `doc`, `decls`, `refs`, `off`, `len`, `col`, `sig`, `sym`,
  `deps`, `envs`, `sig_changed`, `new_name`, `age_s` — the same word list the
  daemon and MCP faces speak. The json face keeps the long frozen spellings.
- Engine faces need a resolved workspace: `pubspec.yaml` plus
  `.dart_tool/package_config.json` (run `dart pub get` first). `--fast` and
  `--sdk-path`/`$DART_SDK` control the rest.
- `debug` carries the same tri-axis (lon default, one line per call); the
  sole live session is driven by default (`-s <ID>` picks one when several
  are live), and a domain failure answers one `{"error":{code,msg}}` line
  with a `DARTQ/DEBUG/*` machine code at exit 1. Its flags are kebab-case
  (`--ws-uri`, `--frame-index`, `--breakpoint-id`) and its wire/answer
  fields snake_case (`session_id`, `isolates`, `main`, `ws_uri`).

## Not Sure dartq Is Installed?

```bash
dartq version   # prints "dartq 0.1.0" or similar
```

If it is not, see [references/getting-started.md](references/getting-started.md)
for install and first-run steps.
