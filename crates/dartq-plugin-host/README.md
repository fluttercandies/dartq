# dartq_plugin_host

Host-side Dart package of the dartiq plugin protocol (PRD section 14, M4
milestone). The dartiq core spawns one process running
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
  object with PRD section 14.3 defaults (mirrors `manifest.rs`).
- `lib/src/plugin_session.dart` — the server role of the handshake state
  machine (mirrors `session.rs`, the client role).
- `lib/src/plugin.dart`, `lib/src/registry.dart`, `lib/src/edits.dart` — the
  plugin-facing API, mirroring `analysis_server_plugin`'s `Plugin` /
  `PluginRegistry` observable surface (`registerAssist`,
  `registerFixForRule`).
- `lib/src/host.dart`, `bin/dartq_plugin_host.dart` — the stdio main loop and
  the launchable entry point.

## Status

Protocol v1 is complete on both sides (handshake, capability negotiation,
diagnostic notifications, fixes round trip). Serving real fixes requires the
engine Query API and the core-side plugin discovery/lifecycle wiring, which
land later in M4; runtime enforcement of the manifest's timeout and memory
budgets is likewise a later M4 step.
