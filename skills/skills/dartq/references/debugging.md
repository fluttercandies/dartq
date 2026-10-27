# Debugging

**Where debugging lives:** the `dartq debug` verb group — the primary
face for CLI and scripted integrations. Every subcommand is a thin proxy
to the worktree's resident session (the same engine, the same semantics
the MCP tools serve), output rides the shared `--format` axis (one lon
line per call by default), the flags are kebab-case (`--ws-uri`,
`--dart-defines`, …), and the wire/answer fields are snake_case — so the
loop learned in the shell works unchanged in an MCP host.

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
dartq debug breakpoint add --script package:app/main.dart --line 42
dartq debug resume                          # release; the program stops on your terms
```

**The session selector is optional.** Most interactions hold exactly one
debug session, and every verb drives it by default — `dartq debug resume`,
`dartq debug evaluate n --frame-index 0`, `dartq debug step over`. With
zero live sessions the answer is a `DARTQ/DEBUG/NO_SESSIONS` error naming
the way out (launch or attach); with several, a
`DARTQ/DEBUG/AMBIGUOUS_SESSION` error listing every candidate
(`#id kind script`, or `#id attach <ws_uri>`) — then pick one with
`-s <ID>` (long form `--session <ID>`).

Skipping the breakpoint step just means the program runs free after
resume. Launching is not the only entry: `dartq debug attach --ws-uri
ws://…` connects to a running VM service — including one owned by a live
`flutter run`; the tool's own console keeps working.

Exit contract: 0 the answer rode stdout; 1 the engine answered a domain
error (one `{"error":{"code","msg"}}` line — the `DARTQ/DEBUG/*`
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

- `dartq debug detach` — end the debug session, **the
  program keeps running**, and the answer carries the ws URI for
  re-attaching later
- `dartq debug stop` — stop the session **and** the
  launched program; a Flutter app is shut down through the tool's own
  cleanup ladder — no orphaned app processes

Pick stop for launched sessions you own; detach for apps you borrowed.

## Edit → Reload Cycle

- `dartq debug reload` — hot reload (`reloadSources`); on
  a Flutter session this is the tool's own orchestrated reload (source
  push + reassemble), so edits on disk take effect. The answer is
  `{ok isolates_reloaded ms err?}` — a failed reload keeps the session
  alive and `err` carries the VM's own text (the compile error as data);
  fix and reload again.
- `dartq debug restart` — hot restart (Flutter): state
  resets like the tool's `R`, isolates are rebuilt, and the session
  **re-arms its breakpoints on the new isolate** (the answer lists
  them). Use it when edits are too large for reload. Bare dart sessions
  have no tool-registered restart service — reload instead.

## While Stopped

| Command | Answer |
| --- | --- |
| `dartq debug status [-s N]` | always the document `{v:1 sessions:[…] orphans:[…]}`, empty arrays included — the worktree's debug sessions and their stopped states, plus the `orphans` re-attach trail (`{id ws_uri kind since released?}`); with `-s`, that session's breakpoints too |
| `dartq debug snapshot --isolate I` | the stopped frame: why, where, the stack summary, and the top frame's variables with lazy-expansion handles |
| `dartq debug evaluate [--frame-index F] 'expr'` | one Dart expression in a paused frame (`--frame-index`) or the root library (absent) — inspect what the snapshot only names, verify a fix from inside the app |
| `dartq debug step into\|over\|out` | advance one frame's worth |
| `dartq debug pause` | interrupt a running session to inspect state |
| `dartq debug exception-pause none\|uncaught\|all` | break on throws |
| `dartq debug breakpoint add/rm` | arm/clear stops (`--script`+`--line`, or `--function-id`; `--condition` makes it conditional) |

## Streams (cursor-paginated; keep the cursor)

- `dartq debug events [--since C] [--limit L]` — what the
  debugger did (lifecycle, stops, …)
- `dartq debug output [--stream stdout\|stderr] [--since C]` —
  what the program printed (bounded capture)

Both answer one page per call; feed the last line's `nextCursor` back as
`--since` to continue without gaps.

## Rules of Thumb

- Debug sessions live in the worktree's resident session process. It exits
  after 15 min with no client, on eviction past the 5-session-per-user cap,
  or when a newer dartq binary wins the next election — but a daemon
  holding debug sessions never idle-reaps, and SIGTERM/SIGINT (the
  election takeover included) run a graceful shutdown: every paused
  isolate is resumed first, a `session`/`shutdown` event
  `{reason: idle|signal|accept_failed, sessions_released}` is written,
  and each held session leaves a re-attach manifest under
  `.dart_tool/dartq/debug/<id>.json` of the served checkout root (the
  session layout root, beside `session.sock` — not the package
  directory the daemon was started in) — a later `dartq debug status`
  lists them as `orphans`, and a successful `debug attach` to the same ws_uri
  consumes one. The election reap's grace is 8 s + 5 s per held session
  (the daemon advertises the count in its heartbeat), so the resume
  ladder fits inside the window.
- Several sessions at once (two apps, or a launch plus an attach): every
  verb then needs `-s <ID>`; `dartq debug status` lists the ids.
- Long-lived app work: launch once, reload repeatedly, restart only when
  state must reset, stop when done.
- Never assume the app died because a command errored — check
  `dartq debug status`; a detached or crashed program keeps its session
  records until stopped.
- Program output and debugger events are separate streams; read both.
- MCP hosts: the same debug face is exposed as `dartq_debug_*` tools; the
  server's `tools/list` describes them — no need to load a catalog here.
