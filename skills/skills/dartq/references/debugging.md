# Debugging

**Where debugging lives:** the `dartq debug` verb group — the primary
face for CLI and scripted integrations. Every subcommand is a thin proxy
to the worktree's resident session (the same engine, the same semantics
the MCP tools serve), output is one JSON line per call, and the argument
spellings are the daemon's own (`--sessionId`, `--wsUri`,
`--dartDefines`, …), so a loop learned in the shell works unchanged in
an MCP host.

dartq speaks the Dart VM Service Protocol directly and layers an AI-first
orchestration on top: launch/attach, line and conditional breakpoints,
stepping, exception pause modes, hot reload/restart, lazy object-graph
expansion, and bounded stdout/stderr capture — all without any IDE.

## The Standard Loop (launch, paused-on-start takeover)

`dartq debug launch` **pauses at start** by default — that is the
takeover window:

```bash
dartq debug launch bin/main.dart            # dart script (kind dart is default)
dartq debug launch --kind flutter lib/main.dart   # Flutter app: device/flavor/dartDefines ride along
dartq debug breakpoint add --sessionId 1 --script package:app/main.dart --line 42
dartq debug resume --sessionId 1            # release; the program stops on your terms
```

Skipping the breakpoint step just means the program runs free after
resume. Launching is not the only entry: `dartq debug attach --wsUri
ws://…` connects to a running VM service — including one owned by a live
`flutter run`; the tool's own console keeps working.

Exit contract: 0 the answer rode stdout; 1 the engine answered a domain
error (one `{"error":{"code","message"}}` line — the `DARTQ/DEBUG/*`
machine code names the cause); 64 usage; 4 the proxy never reached the
engine (no resident session, transport).

## Attach to an app another tool manages

An app launcher that surfaces its VM service URI (a device controller, an
E2E driver, a UI-inspection tool — or your own `flutter run` / `dart
--enable-vm-service`) hands you the attach handle directly — read that
URI from the tool's session info and feed it to `dartq debug attach`.
You get full VM-level debugging while the other tool keeps driving the
UI: set breakpoints, tap through the other tool, inspect the paused
frame, hot-reload your fix, detach without killing the app.

Rules that keep the coexistence clean:

- a paused app freezes the other tool's UI actions — resume before
  driving the UI, pause when you need the frame
- both sides may hot-reload; the service multiplexes clients
- when you are done, `dartq debug detach` leaves the app exactly as the
  other tool expects it

## Endings: detach vs stop

- `dartq debug detach --sessionId N` — end the debug session, **the
  program keeps running**, and the answer carries the ws URI for
  re-attaching later
- `dartq debug stop --sessionId N` — stop the session **and** the
  launched program; a Flutter app is shut down through the tool's own
  cleanup ladder — no orphaned app processes

Pick stop for launched sessions you own; detach for apps you borrowed.

## Edit → Reload Cycle

- `dartq debug reload --sessionId N` — hot reload (`reloadSources`); on
  a Flutter session this is the tool's own orchestrated reload (source
  push + reassemble), so edits on disk take effect. A broken edit
  answers the **compile error as data**; the session stays usable — fix
  and reload again.
- `dartq debug restart --sessionId N` — hot restart (Flutter): state
  resets like the tool's `R`, isolates are rebuilt, and the session
  **re-arms its breakpoints on the new isolate** (the answer lists
  them). Use it when edits are too large for reload. Bare dart sessions
  have no tool-registered restart service — reload instead.

## While Stopped

| Command | Answer |
| --- | --- |
| `dartq debug status [--sessionId N]` | the session's debug sessions and their stopped states — orientation after any stop event; with `--sessionId`, that session's breakpoints too |
| `dartq debug snapshot --sessionId N --isolate I` | the stopped frame: why, where, the stack summary, and the top frame's variables with lazy-expansion handles |
| `dartq debug evaluate --sessionId N [--frameIndex F] 'expr'` | one Dart expression in a paused frame (`--frameIndex`) or the root library (absent) — inspect what the snapshot only names, verify a fix from inside the app |
| `dartq debug step --sessionId N into\|over\|out` | advance one frame's worth |
| `dartq debug pause --sessionId N` | interrupt a running session to inspect state |
| `dartq debug exception-pause --sessionId N none\|uncaught\|all` | break on throws |
| `dartq debug breakpoint add/rm` | arm/clear stops (`--script`+`--line`, or `--functionId`; `--condition` makes it conditional) |

## Streams (cursor-paginated; keep the cursor)

- `dartq debug events --sessionId N [--since C] [--limit L]` — what the
  debugger did (lifecycle, stops, …)
- `dartq debug output --sessionId N [--stream stdout\|stderr] [--since C]` —
  what the program printed (bounded capture)

Both answer one page per call; feed the last line's `nextCursor` back as
`--since` to continue without gaps.

## Rules of Thumb

- Long-lived app work: launch once, reload repeatedly, restart only when
  state must reset, stop when done.
- Never assume the app died because a command errored — check
  `dartq debug status`; a detached or crashed program keeps its session
  records until stopped.
- Program output and debugger events are separate streams; read both.
- MCP hosts: the same debug face is exposed as `dartq_debug_*` tools; the
  server's `tools/list` describes them — no need to load a catalog here.
