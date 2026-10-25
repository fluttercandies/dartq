# dartq e2e suite — process-level verification of every CLI verb

One command, after building the release binary:

```bash
cargo build --release -p dartq-cli
tests/e2e/run.sh
```

Exit status is 0 only when every leg passed. Evidence — each leg's
stdout, stderr and exit code, plus the MCP conversation logs — lands
under `target/evidence/e2e/`. Scratch package trees are generated under a
fresh temp root per run and removed at exit; the suite runs with an
isolated `XDG_CACHE_HOME`, so the session registry, the L3 content store
and the test-result cache never touch the invoking user's caches.

## What it covers (and why it exists this way)

`analyze` already has the systematic flag-parity matrix
(`scripts/cli-parity-matrix.sh`) and `test` the byte-parity gates; this
suite covers everything else at the process level — the newer verbs and
the cross-verb situations their unit tests can't see (real detached
processes, real unix sockets, real `git worktree add`, real `dart pub
get`, kill -9 takeovers, concurrent CLI users).

| Family | Legs (each asserts output contract + exit codes) |
| --- | --- |
| `01-packages` | nested checkout + real `git worktree add` linked tree + stale gitdir pointer as `BOUNDARY` rows; two universes side by side; plain copy; fresh clone (unresolved + hint); broken pubspec (`NOTE` row); orphan workspace member (navigation error + note); bare tree usage error |
| `02-session` | start → attach (same pid) → status → stop (process, socket, state gone); two concurrent sessions in two checkouts; `kill -9` stale-socket takeover; LRU eviction past the 5-session cap (data-driven: the evicted root is read from the `evicted` row) + cold recovery; read-only checkout degrades to `standalone` with a stated reason |
| `03-events` | `events session` cursor pagination (`--limit`/`--since`, seq monotonic) over a really-served session; `events progress` on an 800-file tree (files scanned, cursor, current package); a test run's handle through the MCP tools (run → status poll → events) on the real SDK |
| `04-resolve-complete` | `resolve` DECL (7 fields) / REF (13 fields) / MISS at symbol, declaration, past-end, inside-string cursors; `complete` PREFIX/CAND rows and the empty set inside a string; malformed selectors |
| `05-compat` | two-plus SDK-shaped roots (fake minimal layouts): grant-less axis incompatible with `first_failure`, grant-carrying axis compatible, broken root an isolated `sdk_unavailable` row; no-axis usage error |
| `06-fix-rename` | `fix` applies the unused-import removal and re-analyzes clean; changed-under-us refused as `stale` with the concurrent bytes surviving (deterministic trigger: a duplicate import line makes the anchor relocation ambiguous under any timing); `rename --write` rewrites both files; `rename` refuses outside-workspace edit sets |
| `07-mcp` | initialize handshake → initialized → ping → `tools/list` (≥20 tools, every description states use/non-use) → real `tools/call`s (analyze, query, events, rename — the rename stays dry-run) → progressToken bookend notifications → unknown-tool `isError` → workspace-status resource → unknown method `-32601` → clean EOF shutdown; read-only tree degrades with the reason; unresolvable dir usage |
| `08-cross` | an edit between two commands seen by the next command (both directions, one-shot and resident — no stale diagnostics); two concurrent CLI users on one worktree (both succeed, byte-identical output); a live session across a real `dart pub get` that adds a path dependency (session survives, the next `packages`/`analyze`/resident analyze sees the new universe) |
| `09-exit-codes` | the analyze severity ladder 0/1/2/3 (severity remapped hermetically through `analysis_options.yaml`, plus `--fatal-infos`/`--no-fatal-warnings`); usage (64) spot checks across analyze/query/explain/impact/compat/rename/session/events/fix/mcp and the top level; version faces |
| zero-orphans | the #2583 rule: no `dartq session serve`, `dartq mcp` or `dartq test` process outlives the suite |

## Hermetic vs dart-toolchain legs

Everything runs against generated trees plus minimal SDK-shaped roots
(the same stub-SDK shape the daemon/session fixtures use), so the engine
tier needs no Dart toolchain. Three legs additionally exercise the real
world: the MCP test-run handle, the SDK-symbol rename refusal, and the
`dart pub get` cross-situation. They need a real Dart SDK — the fvm
default, or `DARTQ_DART_SDK=<sdk-root>` (the same convention as
`scripts/cli-parity-matrix.sh`). Without it those legs report `SKIP`
(and only those); the suite still gates everything else.

## Conventions

- Shell, not Rust integration tests: this repo's process-level
  verification convention is gate scripts driving the release binary
  (`scripts/*.sh` → evidence under `target/evidence/`); spawning
  daemons, killing pids, racing edits and driving MCP stdio are natural
  there, and the suite adds no compile time to `cargo test`.
- One MCP stdio client (`lib/mcp_driver.py`) drives every MCP
  conversation: scripted JSON-RPC lines, response correlation by id,
  retry-until support for async answers (test runs), and a response
  deadline so a wedged server fails a leg instead of hanging the suite.
- The tracked fixtures under `tests/testface/` are reused where they
  exist (the test-run leg copies `dart_pkg`); everything larger is
  generated per run.
- Fixture builders and assertions live in `lib/common.sh`; `wf`,
  `make_pkg`, `make_stub_sdk`, `gen_files` mirror the shapes the
  upstream unit fixtures use.
