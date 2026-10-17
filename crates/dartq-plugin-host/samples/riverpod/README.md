# riverpod: live-compat overlay for the real `riverpod_lint` plugin

Wires the real `rrousselGit/riverpod` `riverpod_lint` package
(https://github.com/rrousselGit/river_pod, vendored snapshot under
`vendor/riverpod_lint` — see `vendor/PROVENANCE.md`) onto the dartq plugin host
through a thin overlay package. Companion to `samples/compat` (the
in-repo, fully dartq-native sample).

## What is real here

The upstream package is official `analysis_server_plugin`-shaped: a
top-level `plugin` object whose `register` runs, verbatim, 16
`registerWarningRule` + 6 `registerFixForRule` + 12 `registerAssist` calls
against the overlay's bridge — the bulk-registration row of the compat
matrix (34 registrations). The bridge forwards the two kinds the host
mirrors — `registerFixForRule` (official `DiagnosticCode` → the host face's
stable string via `code.uniqueName`) and `registerAssist` — onto a
`PluginRegistryImpl`.

## The adapter gap

`registerLintRule`/`registerWarningRule` have no host mirror: protocol v1
has no rule registry (the engine's diagnostics batch is the plugin's
context), and the upstream rules compute from the analyzer
AST, which the v1 wire does not carry. All sixteen warning rules are
census-only (`lint=0 warning=16 fix=6 assist=12`); the rule semantics land
with the Query API. This is a surface gap, not a
structural one — the `Plugin.register` shape itself maps cleanly.

## Layout

- `dartq_plugin.json` — manifest; capabilities `[lint, fix, assist]`.
  Memory budget (1024 MB) derives from measured overlay residency
  (540-670 MB JIT warm) with headroom.
- `pubspec.yaml` — path deps on `dartq_plugin_host` (`../..`) and the pinned
  upstream clone; needs its own `dart pub get`. The upstream package is
  `resolution: workspace`, so resolution pulls the riverpod repo's
  workspace.
- `bin/riverpod_plugin.dart` — the launchable entrypoint.
- `lib/main.dart` — imports the real `plugin`, registers it through the
  bridge, and serves deterministic census/marker faces.

## Live test

`crates/dartq-plugin/tests/compat_live.rs`, `riverpod_overlay_*` — spawns
this directory end to end, asserting all three negotiated capabilities, the
bulk census, and the fixes face (6 fix generators then 12 assists).
`#[ignore]`d for the serial verification phase.
