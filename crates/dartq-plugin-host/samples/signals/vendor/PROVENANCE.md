# Provenance

Vendored snapshot of `signals_lint` 7.0.0, pruned from the upstream
analysis_server_plugin samples tree
(https://github.com/dart-lang/analysis_server_plugin, samples/signals,
snapshot commit b11ddc851e92546adb4a4929a557cf537bf116da, 2026-10-13).

Pruned to `pubspec.yaml`, `lib/`, and `LICENSE` so the fixture is
self-contained on a fresh clone; the sample's `pubspec.yaml` path-depends on
this directory. Everything else (tests, example, screenshots) was dropped.
All Dart sources are byte-identical to upstream. The upstream `CONTEXT.md`
docs inside `lib/` kept their content, with `file:///` links re-pointed to
the upstream signals.dart repository
(https://github.com/rodydavis/signals.dart).
