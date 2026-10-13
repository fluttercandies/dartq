# dartq_plugin_host

Host-side Dart package of the dartq plugin protocol: newline-delimited
JSON frames over stdio, a versioned handshake with capability negotiation,
and diagnostic/fix notifications between the dartq core (the client) and
one spawned process per plugin (the server). The dartq core spawns one
process running
`bin/dartq_plugin_host.dart` per plugin; the process speaks the wire contract
specified by the Rust crate `dartq-plugin` (`crates/dartq-plugin`, the single
source of truth for the protocol) and hosts third-party plugins behind an
interface shaped like `package:analysis_server_plugin`.

This directory is a **Dart package**, not part of the Cargo workspace: it
contains no `Cargo.toml`, and the workspace member list in the root
`Cargo.toml` deliberately excludes it.

## Layout

- `lib/src/frame.dart` — wire frames and the newline-delimited JSON codec
  (mirrors `crates/dartq-plugin/src/frame.rs`).
- `lib/src/capabilities.dart` — capability vocabulary; the effective set is
  the intersection of both handshake sides (mirrors `capability.rs`).
- `lib/src/plugin_config.dart` — the `dartq_plugin.json` manifest as a value
  object carrying the protocol's manifest defaults (mirrors `manifest.rs`).
- `lib/src/plugin_session.dart` — the server role of the handshake state
  machine (mirrors `session.rs`, the client role).
- `lib/src/plugin.dart`, `lib/src/registry.dart`, `lib/src/edits.dart` — the
  plugin-facing API, mirroring `analysis_server_plugin`'s `Plugin` /
  `PluginRegistry` observable surface (`registerAssist`,
  `registerFixForRule`).
- `lib/src/host.dart`, `bin/dartq_plugin_host.dart` — the stdio main loop
  and the launchable entry point; `servePlugin` is the plugin-bearing
  integration a plugin package's entry point delegates to (registration,
  producers, lifecycle on the same loop; `bin/live_plugin.dart` is the
  reference wiring).
- `bin/echo_plugin.dart`, `bin/live_plugin.dart`, `bin/fix_plugin.dart`,
  `bin/slow_plugin.dart` — test fixtures the live suites spawn.
- `samples/compat/` — the compatibility sample plugin, shaped like an
  official `analysis_server_plugin` package; the porting guide (official-API
  concept → dartq equivalent) lives in its `README.md`. Its `query` launch
  argument arms the T4 query face: one more producer that resolves the
  requested position through `ProducerContext.queries` before shaping its
  change, with `samples/compat/bin/query_probe.dart` as the face's
  self-contained live probe (it spawns the entrypoint and plays the core).

## Status

Ships in 1.0.0: the complete protocol v1 host — newline-delimited JSON
frames over stdio with a 16 MiB frame cap, the versioned handshake with
capability negotiation, diagnostic and fix notifications, and producer
execution via `servePlugin` (assists at the requested range, rule fixes at
a matching engine diagnostic's range of the latest pushed batch, best
priority first). The isolation model is one sidecar Dart process per
plugin: the process never enters the Rust core and only ever speaks this
wire contract, while the core enforces the manifest budgets on the child —
`callback_ms` deadlines on initialize, `request/fixes`, and analyze
demands, `memory_budget_mb` via an RSS monitor, and the crash/restart
budget (pinned by the `dartq-plugin` integration suites, e.g.
`crates/dartq-plugin/tests/memory_budget.rs`). The host package itself
carries 10 Dart test files under `test/` (97 `test(` occurrences) covering
the codec, handshake, registry, and live loop.

Explicitly post-1.0: full resolved-context parity — a producer whose fix
needs the type of an arbitrary expression *without* a query stays out (the
T4 boundary); parsed-level AST context is host-local, and resolution-owned
facts ride the Query API (`ProducerContext.queries`: the index query kinds
plus `element_at` over reverse `request/query`, available when the `query`
capability was negotiated) — the compat sample arms that face behind its
`query` launch argument, with `samples/compat/bin/query_probe.dart` as its
live probe. Also `registerLintRule` breadth: the
shim accepts official-style `registerAssist`/`registerFixForRule`
registration, while `registerLintRule` throws an `UnsupportedError` naming
the anchor (pinned by `test/shim_plugin_test.dart`).
