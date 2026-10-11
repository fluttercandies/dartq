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
  concept → dartq equivalent) lives in its `README.md`.

## Status

Protocol v1 is complete on both sides (handshake, capability negotiation,
diagnostic notifications, fixes round trip), and registered producers are
executed: `servePlugin` answers `request/fixes` from the registry (assists
at the requested range, rule fixes at a matching engine diagnostic's range
of the latest pushed batch), best priority first. Widening the producer
context beyond the engine batch needs the engine Query API, which lands
later in M4; runtime enforcement of the manifest's timeout and memory
budgets is likewise a later M4 step.
