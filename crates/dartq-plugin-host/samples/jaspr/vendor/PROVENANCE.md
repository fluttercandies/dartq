# Provenance

Vendored snapshot of `jaspr_lints` 0.7.2, pruned from the upstream
analysis_server_plugin samples tree
(https://github.com/dart-lang/analysis_server_plugin, samples/jaspr,
snapshot commit b11ddc851e92546adb4a4929a557cf537bf116da, 2026-10-13).

Pruned to `pubspec.yaml`, `lib/`, and `LICENSE` so the fixture is
self-contained on a fresh clone; the sample's `pubspec.yaml` path-depends on
this directory. Everything else (tests, analysis options) was dropped. The
upstream `LICENSE` symlink is resolved to the real file. All Dart sources are
byte-identical to upstream.
