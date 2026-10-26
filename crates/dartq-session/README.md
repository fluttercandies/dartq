# dartq-session

The resident session layer: one long-lived analysis server per worktree,
converged on by every face — CLI one-shots, the events stream, MCP
clients — through the same identity triple.

- **The triple** — resolution root realpath, SDK identity (root,
  `version` file, `.fvmrc`), and the running binary's content hash: two
  faces share one engine exactly when their triples agree.
- **Election** — two simultaneous cold starters elect through an
  exclusive `flock` on `.dart_tool/dartq/elect.lock`; the winner
  attaches a live matching server (reaping every other daemon recorded
  on the root — one daemon per root, ever), reaps a stale one (dead
  pid, foreign triple, wedged heartbeat, dead socket: terminate, wait,
  SIGKILL escalation) before reclaiming its socket and registry entry,
  and spawns the replacement under a bring-up placeholder until its
  first heartbeat; the loser waits for the live state and attaches. A
  read-only checkout degrades to standalone — the pre-session behavior,
  never an error.
- **Registry and lifecycle** — a per-user registry (`~/.cache/dartq/
  sessions/`) backs `dartq session status`; the serve loop heartbeats
  `state.json`, reaps itself after an idle window, and the starter
  enforces a live-session cap (LRU eviction; evicted worktrees cold
  restart against their stamp caches) and sweeps orphans from a previous
  binary on every start.
- **Events** — the session's append-only, cursor-paginated event log and
  the cold-analysis progress snapshot (files scanned, current package)
  derived from the engine trace.

The Windows mapping (named pipe at the same layout, `LockFile`
election, no self-pipe — the serve loop's accept-tick replaces it) is
designed; the Unix socket plumbing is `cfg(unix)` and other hosts
degrade to standalone.
