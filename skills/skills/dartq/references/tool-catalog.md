# MCP Tool Catalog

28 tools, each mirroring a daemon method of the worktree's resident session
(the daemon `dartq/*` method name is listed with each). Descriptions follow
the server's own `tools/list` answers.

## Analysis and Navigation

### dartq_analyze — `dartq/analyze`
Full-engine diagnostics for the workspace (scan, parse, resolution,
semantic/flow/lint). Use for: "what is wrong with this code", pre-commit
checks, whole-file review context. Not for: symbol lookups (`dartq_query`),
why a diagnostic fires (`dartq_explain`), or blast-radius questions
(`dartq_impact`). Arguments: the daemon analyze parameters (verbosity,
context, agent).

### dartq_query — `dartq/query`
Symbol-index lookups: declarations and resolved references by name. Use
for: "where is X declared/used", call sites, implementations,
dependencies. Not for: diagnostics or one-cursor symbol resolution (no CLI
mirror — the daemon resolveAt). Arguments: `{name, kind
(symbol|references|callers|implementations|dependencies), verbosity}`.

### dartq_explain — `dartq/explain`
The one-stop report for one symbol: declaration, shape, references,
related diagnostics. Use for: "tell me everything about this API". Not
for: raw reference rows (`dartq_query` is cheaper) or diagnostics alone.

### dartq_impact — `dartq/impact`
What a change to one file would affect: dependent files, signatures,
tests. Use before: edits whose blast radius is unclear. Not for: current
diagnostics — this answers the counterfactual.

### dartq_fix — `dartq/fix`
Anchored fix proposals/patches for the analyzed workspace. Use for:
applying lint/quick fixes. Not for: rename edits or plugin assists.

## Workspace Edits

### dartq_rename — in-process engine (cold pass per call)
Rename one symbol across the workspace; returns the machine edit set
(dry-run semantics — the caller applies edits). Arguments: `{target:
"PATH:LINE:COL", newName}`. Expect it slower than the session-resident
tools.

## Tests

| Tool | Method | Use for |
| --- | --- | --- |
| `dartq_test` | `dartq/test/run` | running `dart test` through the session (cached, impacted-selectable) |
| `dartq_test_status` | `dartq/test/status` | polling the session's test runs, in-flight and finished |
| `dartq_test_events` | `dartq/test/events` | cursor-paged events of one run — streaming results |
| `dartq_test_cancel` | `dartq/test/cancel` | aborting a run that outlived its value |

## Observability

### dartq_events — session event log / progress snapshot (read locally)
The session's unified event stream or its cold-analysis progress (files
scanned, current package). Use for: "is it still working" during a long
first analysis, and lifecycle reads. Not for: test-run result events.
Arguments: `{handle: "session"|"progress", since, limit}`.

## Debugging (17 tools)

| Tool | Method | One line |
| --- | --- | --- |
| `dartq_debug_launch` | `dartq/debug/launch` | launch dart/flutter; pauses at start — arm breakpoints, then resume |
| `dartq_debug_attach` | `dartq/debug/attach` | attach to a running VM service ws URI, incl. a live `flutter run` |
| `dartq_debug_detach` | `dartq/debug/detach` | end debugging, program keeps running, ws URI returned for re-attach |
| `dartq_debug_stop` | `dartq/debug/stop` | stop session and program (Flutter cleanup ladder — no orphaned app) |
| `dartq_debug_status` | `dartq/debug/status` | sessions and their stopped states — orientation after a stop |
| `dartq_debug_breakpoint_add` | `dartq/debug/breakpointAdd` | add a line breakpoint |
| `dartq_debug_breakpoint_remove` | `dartq/debug/breakpointRemove` | remove one breakpoint |
| `dartq_debug_exception_pause` | `dartq/debug/exceptionPause` | set pause mode: never / uncaught / all |
| `dartq_debug_resume` | `dartq/debug/resume` | continue after any stop |
| `dartq_debug_pause` | `dartq/debug/pause` | interrupt a running session |
| `dartq_debug_step` | `dartq/debug/step` | step into / over / out |
| `dartq_debug_snapshot` | `dartq/debug/snapshot` | stopped frame: variables, scopes |
| `dartq_debug_reload` | `dartq/debug/reload` | hot reload; Flutter: orchestrated push + reassemble; compile errors answer as data |
| `dartq_debug_restart` | `dartq/debug/restart` | hot restart; state resets, isolates rebuilt, breakpoints re-armed |
| `dartq_debug_evaluate` | `dartq/debug/evaluate` | evaluate one expression in a paused frame or the root library |
| `dartq_debug_events` | `dartq/debug/events` | cursor-paged debugger events |
| `dartq_debug_output` | `dartq/debug/output` | cursor-paged program output |

Semantics and the launch loop: [debugging.md](debugging.md).

## Resource

- `dartq://workspace-status` — the bound worktree, its layout, and whether
  the resident session is attached (the degradation reason when not).
