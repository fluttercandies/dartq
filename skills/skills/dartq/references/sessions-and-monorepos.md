# Sessions and Monorepos

## The Session Model in One Paragraph

Every dartq command auto-attaches to the worktree's **resident session** — a
long-lived engine server keyed on a triple: **(resolution-root realpath, SDK
identity, dartq binary hash)**. Two faces share one engine exactly when
their triples agree; an FVM switch or a dartq upgrade is a different
session, never a stale attach. Artifacts live in the worktree at
`.dart_tool/dartq/` (`session.sock`, `state.json`); a per-user registry
(`~/.cache/dartq/sessions/`) backs `dartq session status`. Nothing needs
manual warm-up — the first command in a cold worktree elects the server,
later commands attach.

Depth (election, idle reaper, cap, standalone degrade):
[session-model.md](session-model.md).

## Managing Sessions

```bash
dartq session start       # elect or attach; print the identity (JSON line)
dartq session status      # your live sessions, grouped by checkout
dartq session stop        # end this worktree's session
dartq session stop --all  # end every live session
```

Output is one row per line (lon by default, `--format json` for JSON
rows). `start` on an unwritable or foreign tree **degrades to
standalone, printed, never an error**: the command proceeds in-process.

## Watching a Cold Analysis

The first analysis of a large tree takes time; later commands wait on it.
Watch progress instead of guessing:

```bash
dartq events progress            # files scanned, current package
dartq events progress --since 4210   # cursor: the last-seen file count
dartq events session             # lifecycle rows, newest first (limit 100 by default)
dartq events session --since 1730034000 --limit 50   # rows at/after a unix-seconds timestamp
```

## Monster Monorepos: `dartq packages`

`packages` inventories the **enclosing universe** — the workspace root or
checkout the discovery ladder finds — not just the directory you stand in:

```bash
dartq packages                  # the default: {v root pkgs bounds}
dartq packages --format json    # one JSON object per line
```

Machine rows come in two families:

- `PKG|path|name|kind|sdk|resolved` — every work point: workspace roots,
  members, independent packages, crossing path dependencies
- `BOUNDARY|path|reason[|detail]` — every boundary the scan **pruned**, and
  why

Read the boundary rows: they tell you exactly why that nested checkout was
not analyzed (a foreign SDK, a pruned vendored tree, ...). A package that
"disappeared" from analysis answers here, not in a bug.

## Multi-Worktree Parallel Development

The model is built for many checkouts of one repo living side by side:

- Sessions are per worktree (per triple) — parallel worktrees each get
  their own engine, electing independently; no cross-talk.
- Content-addressed caches (the L3 store under `~/.cache/dartq/`) and the
  per-user test result store are **shared across checkouts**: a suite green
  in one worktree can replay in another with the same content.
- The user-level session cap is 5 live sessions; past it, the
  least-recently-heartbeat session is evicted, and its next command is a
  cold restart (cheap — stamp caches survive under `.dart_tool/dartq/`).

When coordinating agents across worktrees: give each its worktree and let
the triple do the naming — never share one session from two checkouts.

## Session Hygiene for Agents

| Symptom | Action |
| --- | --- |
| "Another dartq binary generation" behavior after upgrade | expected — a new binary hash is a new session; the old one is swept |
| Analysis feels cold every command | `dartq session status`: is the worktree's session live, or capped out? |
| Worktree deleted without `session stop` | the idle reaper (15 min with no client) ends it; or `stop --all` |
| Read-only checkout | standalone degrade is normal and printed — output is still correct |
