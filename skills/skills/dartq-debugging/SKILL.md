---
name: dartq-debugging
description: Use when launching or attaching to a running Dart/Flutter app, setting breakpoints, inspecting paused state, evaluating expressions in a frame, or doing hot reload/restart from an agent session — no IDE in the loop
---

# dartq: Debugging

**Where debugging lives:** the debug surface is **daemon/MCP-native**. There
is no `dartq debug` CLI verb. Debug operations are `dartq/debug/*` methods
served by the worktree's resident session and exposed as MCP tools — use
them through `dartq mcp` (see `dartq-mcp`) or any MCP client. The commands
below are tool calls, not shell commands.

dartq speaks the Dart VM Service Protocol directly and layers an AI-first
orchestration on top: launch/attach, line and conditional breakpoints,
stepping, exception pause modes, hot reload/restart, lazy object-graph
expansion, and bounded stdout/stderr capture — all without any IDE.

## The Standard Loop (launch, paused-on-start takeover)

`dartq_debug_launch` **pauses at start** by design — that is the takeover
window:

1. `dartq_debug_launch` (kind `dart` or `flutter`; `flutter` is the primary
   target — device/flavor/dartDefines ride along) → the app starts paused
2. `dartq_debug_breakpoint_add` — arm the breakpoints you actually want
3. `dartq_debug_resume` — release; the program runs and stops on your terms

Skipping step 2 just means the program runs free after resume. Launching
is not the only entry: `dartq_debug_attach` connects to a running VM
service ws URI — including one owned by a live `flutter run`; the tool's
own console keeps working.

## Endings: detach vs stop

- `dartq_debug_detach` — end the debug session, **the program keeps
  running**, and the answer carries the ws URI for re-attaching later
- `dartq_debug_stop` — stop the session **and** the launched program; a
  Flutter app is shut down through the tool's own cleanup ladder — no
  orphaned app processes

Pick stop for launched sessions you own; detach for apps you borrowed.

## Edit → Reload Cycle

- `dartq_debug_reload` — hot reload (`reloadSources`); on a Flutter session
  this is the tool's own orchestrated reload (source push + reassemble),
  so edits on disk take effect. A broken edit answers the **compile error
  as data**; the session stays usable — fix and reload again.
- `dartq_debug_restart` — hot restart (Flutter): state resets like the
  tool's `R`, isolates are rebuilt, and the session **re-arms its
  breakpoints on the new isolate** (the answer lists them). Use it when
  edits are too large for reload.

## While Stopped

| Tool | Answer |
| --- | --- |
| `dartq_debug_status` | the session's debug sessions and their stopped states — orientation after any stop event |
| `dartq_debug_snapshot` | the stopped frame snapshot: variables, scopes (lazy expansion by handle) |
| `dartq_debug_evaluate` | one Dart expression in a paused frame (`frameIndex`) or the root library (no frameIndex) — inspect what the snapshot only names, verify a fix from inside the app |
| `dartq_debug_step` | into / over / out |
| `dartq_debug_pause` | interrupt a running session to inspect state |
| `dartq_debug_exception_pause` | break on throws: `never` / `uncaught` / `all` |

## Streams (cursor-paginated; keep the cursor)

- `dartq_debug_events` — what the debugger did (lifecycle, stops, ...)
- `dartq_debug_output` — what the program printed (bounded capture)

Both take a `since` cursor; poll with the last line's cursor to replay
without gaps.

## Rules of Thumb

- Long-lived app work: launch once, reload repeatedly, restart only when
  state must reset, stop when done.
- Never assume the app died because a tool call errored — check
  `dartq_debug_status`; a detached or crashed program keeps its session
  records until stopped.
- Program output and debugger events are separate streams; read both.
