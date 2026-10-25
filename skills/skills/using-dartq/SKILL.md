---
name: using-dartq
description: Use when working in any Dart or Flutter codebase where dartq is (or could be) installed — before reaching for dart analyze, dart test, flutter test, or an IDE, pick the dartq skill that matches the task
---

# Using dartq

dartq is a drop-in Rust toolchain for Dart and Flutter analysis, testing, and
debugging. AI agents are its first-class users: piped output is machine output,
every face has a stable schema, and one resident engine per worktree serves
CLI, events, and MCP clients alike.

## The Contract (memorize these three lines)

1. **Piped = machine.** When stdout is not a TTY, commands emit machine faces
   (JSON envelope, pipe-separated rows) with zero prose. An explicit `--format`
   always wins.
2. **Exit codes follow dart.** `analyze`: 0 clean/info-only, 1 fatal infos,
   2 fatal warnings, 3 errors, 4 internal failure, 64 usage. `test`: dart
   test's own codes (0 pass, 1 fail, 64 usage, 65 data error, 79 no tests).
3. **Commands auto-attach to the worktree's session.** The first command in a
   worktree elects a resident engine (`.dart_tool/dartq/`); later commands
   reuse it. Nothing to start, nothing to keep alive.

## The Verb Surface

| Verb | One line |
| --- | --- |
| `analyze` | Full-engine diagnostics; `--fast` = parser-only sweep |
| `packages` | Every package root and pruned boundary of the enclosing monorepo |
| `query` | Symbol index: declarations, references, callers, implementations, dependencies, batch |
| `explain` | One-stop JSON report for one symbol or one diagnostic code |
| `impact` | What a change to one file would affect |
| `rename` | Workspace-wide rename; dry-run edit set or `--write` |
| `fix` | Emit or apply anchored fix patches |
| `test` | `dart test` / `flutter test` with a result cache and `--impacted` |
| `session` | Manage the resident engine: start / status / stop |
| `events` | Cursor-paginated session log or cold-analysis progress |
| `resolve` / `complete` | One-shot `PATH:LINE:COL` cursor answers |
| `compat` | One workspace under several SDK roots: a compatibility matrix |
| `mcp` | stdio MCP server mirroring the verbs (28 tools) |

Run `dartq <verb> --help` for the full grammar of any face.

## Pick the Skill

| When the task is... | Load |
| --- | --- |
| First contact: install, first run, orienting in a repo | `dartq-getting-started` |
| Running tests, speeding up edit-test loops, cache questions | `dartq-test-driving` |
| Diagnose → understand → scope → edit → verify on analyzed code | `dartq-analysis-loop` |
| Nested worktrees, monster monorepos, multi-checkout parallel work | `dartq-sessions-and-monorepos` |
| Launching, pausing, inspecting, hot-reloading a running app | `dartq-debugging` |
| Wiring dartq into an agent host as MCP tools | `dartq-mcp` |

## Defaults That Surprise Newcomers

- `dartq analyze` piped prints the **single-line JSON envelope** (byte parity
  with `dart analyze --format=json`), not human prose. Ask for
  `--format machine` for `SEVERITY|TYPE|CODE|FILE|LINE|COL|LENGTH|MESSAGE`
  rows, or `--format default` for the human grouping.
- `query`, `packages`, and `fix` flip the other way when piped: machine rows
  or JSON documents instead of their TTY tables.
- Engine faces need a resolved workspace: `pubspec.yaml` plus
  `.dart_tool/package_config.json` (run `dart pub get` first). `--fast` and
  `--sdk-path`/`$DART_SDK` control the rest.

## Not Sure dartq Is Installed?

```bash
dartq version   # prints "dartq 0.1.0" or similar
```

If it is not, see `dartq-getting-started` for install and first-run steps.
