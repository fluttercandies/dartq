# signals: live-compat overlay for the real `signals_lint` plugin

Wires the real `rodydavis/signals.dart` `signals_lint` package
(https://github.com/rodydavis/signals.dart, vendored snapshot under
`vendor/signals_lint` — see `vendor/PROVENANCE.md`) onto the dartq plugin host
through a thin overlay package. Companion to `samples/compat` (the
in-repo, fully dartq-native sample).

## The adapter gap (structural)

`signals_lint` is **not** `analysis_server_plugin`-shaped: it is a
`custom_lint` plugin — `PluginBase createPlugin()` with
`getLintRules`/`getAssists`. There is no `Plugin.register(PluginRegistry)`
and no `registerFixForRule`; its five change producers are `DartAssist`
subclasses. No host-side registry mirror can absorb that API shape, so the
overlay drives the upstream surface directly and reports the census
(`lint=5 warning=0 fix=0 assist=5`). Porting it onto dartq would mean
rewriting the rules against the engine's rule surface, not adapting a
registry call.

## Layout

- `dartq_plugin.json` — manifest; capabilities `[lint, fix]` (the assists are
  served through `request/fixes`; the wire has no separate assists method).
  Memory budget (1024 MB) derives from measured overlay residency
  (540-670 MB JIT warm) with headroom.
- `pubspec.yaml` — path deps on `dartq_plugin_host` (`../..`) and the pinned
  upstream clone; needs its own `dart pub get` (unlike `samples/compat`,
  which rides the host package's resolution).
- `bin/signals_plugin.dart` — the launchable entrypoint.
- `lib/main.dart` — builds the real upstream plugin object, enumerates its
  rules and assists, and serves deterministic census/marker faces.

## Live test

`crates/dartq-plugin/tests/compat_live.rs`,
`signals_overlay_*` — spawns this directory end to end (discovery, spawn,
handshake, diagnostics batch, census assertion, UTF-16 marker edit applied
to real text past an emoji, graceful shutdown). `#[ignore]`d for the serial
verification phase.
