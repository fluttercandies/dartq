# The Resident Session Model

## Identity — the Triple

A session is keyed on three facts:

1. the resolution root's **realpath**,
2. the **SDK identity**: resolved SDK root, its `version` file bytes, and
   the worktree's `.fvmrc` when present,
3. the running dartq **binary's content hash**.

Two faces agree on one engine exactly when their triples agree. Consequences:
an FVM switch is a different session; a dartq upgrade is a different
session (and a different protocol generation — the starter kills orphans
from a previous binary rather than talking to them).

## Layout

- `.dart_tool/dartq/session.sock` — the daemon protocol over a Unix socket
  (a named pipe at the same path on Windows)
- `.dart_tool/dartq/state.json` — pid, heartbeat, triple
- `.dart_tool/dartq/elect.lock` — the election lock
- `~/.cache/dartq/sessions/` — the per-user registry, one JSON file per
  session keyed by the triple's hash

Writing session artifacts under `.dart_tool` never defines the package
boundary (that is the discovery layer's rule), and a read-only checkout
degrades to standalone — printed, never an error; the caller runs
in-process exactly like the pre-session behavior.

## Election

Two simultaneous cold starters elect through an exclusive `flock` on
`elect.lock`: the winner takes over (a stale socket whose pid is gone is
unlinked and re-spawned) and brings the server up; the loser waits for the
socket and attaches. A live server with a matching triple is attached,
never duplicated.

## Lifecycle Constants

| Fact | Value |
| --- | --- |
| Heartbeat | `state.json` rewritten every 5 s while serving |
| Idle reaper | exits after 15 min with no connected client |
| Cold-start timeout | a listening socket must appear within 30 s |
| Session cap | 5 live sessions per user; the least-recently-heartbeat is evicted past it |

An evicted worktree's next command is a cold restart — its stamp caches
under `.dart_tool/dartq/` keep that restart cheap.

## Who Converges on the Session

- CLI verbs (auto-attach on every command)
- `dartq events` (the session's own log and progress snapshot)
- `dartq mcp` (every MCP tool converges on the worktree's resident session
  — one engine for CLI, events, and MCP clients alike)

`dartq session start` merely pre-elects; commands work without it. The
hidden `dartq daemon <DIR>` verb is the raw one-client JSON-RPC face — the
session layer supersedes it for interactive use.
