# pyramid: live-compat overlay for the real `pyramid_lint` plugin

Wires the real `charlescyt/pyramid_lint` package (pinned clone under
`.third/aswp-samples/pyramid_lint`, read-only) onto the dartq plugin host
through a thin overlay package. Companion to `samples/compat` (the
in-repo, fully dartq-native sample).

## What is real here

The upstream package is official `analysis_server_plugin`-shaped: a
top-level `plugin` object whose `register` runs, verbatim, 37
`registerLintRule` + 23 `registerFixForRule` + 3 `registerAssist` calls
against the overlay's bridge — the lint-only bulk row of the compat matrix
(63 registrations). The bridge forwards the two kinds the host mirrors —
`registerFixForRule` (official `DiagnosticCode` → the host face's stable
string via `code.uniqueName`) and `registerAssist` — onto a
`PluginRegistryImpl`.

## The adapter gaps

- **Registry surface**: `registerLintRule` has no host mirror — protocol v1
  has no rule registry (the engine's diagnostics batch is the plugin's
  context, PRD section 14.2), and the upstream rules compute from the
  analyzer AST, which the v1 wire does not carry. All 37 rule registrations
  are census-only (`lint=37 warning=0 fix=23 assist=3`); the rule semantics
  land with the Query API (PRD section 14.2). A surface gap, not a
  structural one.
- **Capability posture**: the manifest advertises `[lint]` only, so the core
  never sends `request/fixes` — the fix and assist delegations are provably
  registered yet unserved on this session.

## Layout

- `dartq_plugin.json` — manifest; capabilities `[lint]`.
  Memory budget (1024 MB) derives from measured overlay residency
  (540-670 MB JIT warm) with headroom.
- `pubspec.yaml` — path deps on `dartq_plugin_host` (`../..`) and the pinned
  upstream clone (the pyramid repo root is itself the package); needs its
  own `dart pub get`.
- `bin/pyramid_plugin.dart` — the launchable entrypoint.
- `lib/main.dart` — imports the real `plugin`, registers it through the
  bridge, and serves the census face.

## Live test

`crates/dartq-plugin/tests/compat_live.rs`, `pyramid_overlay_*` — spawns
this directory end to end, asserting the negotiated `[lint]` capability, the
bulk census, and that `request/fixes` is gated off. `#[ignore]`d for the
serial verification phase.
