# jaspr: live-compat overlay for the real `jaspr_lints` plugin

Wires the real `schultek/jaspr` `jaspr_lints` package
(https://github.com/schultek/jaspr, vendored snapshot under
`vendor/jaspr_lints` — see `vendor/PROVENANCE.md`) onto the dartq plugin host
through a thin overlay package. Companion to `samples/compat` (the in-repo,
fully dartq-native sample).

## What is real here

The upstream package is official `analysis_server_plugin`-shaped: a
top-level `plugin` object whose `register` runs, verbatim, 3
`registerLintRule` + 2 `registerWarningRule` + 4 `registerFixForRule` + 14
`registerAssist` calls against the overlay's bridge. The bridge forwards the
two kinds the host mirrors — `registerFixForRule` (official `DiagnosticCode`
→ the host face's stable string via `code.uniqueName`) and
`registerAssist` — onto a `PluginRegistryImpl`.

## The adapter gap

`registerLintRule`/`registerWarningRule` have no host mirror: protocol v1
has no rule registry (the engine's diagnostics batch is the plugin's
context), and the upstream rules compute from the analyzer
AST, which the v1 wire does not carry. Those registrations are counted in
the bridge census (`lint=3 warning=2 fix=4 assist=14`) instead of delegated;
the rule semantics land with the Query API. This is a
surface gap, not a structural one — the `Plugin.register` shape itself maps
cleanly.

## Layout

- `dartq_plugin.json` — manifest; capabilities `[lint, fix, assist]`.
  Memory budget (1024 MB) derives from measured overlay residency
  (540-670 MB JIT warm) with headroom.
- `pubspec.yaml` — path deps on `dartq_plugin_host` (`../..`) and the pinned
  upstream clone; needs its own `dart pub get`. The upstream package is
  `resolution: workspace`, so resolution pulls the jaspr repo's workspace.
- `bin/jaspr_plugin.dart` — the launchable entrypoint.
- `lib/main.dart` — imports the real `plugin`, registers it through the
  bridge, and serves deterministic census/marker faces.

## Live test

`crates/dartq-plugin/tests/compat_live.rs`, `jaspr_overlay_*` — spawns this
directory end to end, asserting all three negotiated capabilities, the
census, and the fixes face (fix generators then assists, marker edit applied
to real text past an emoji). `#[ignore]`d for the serial verification phase.
