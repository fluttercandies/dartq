# Provenance

Vendored snapshot of `pyramid_lint` 3.0.0, pruned from the upstream
analysis_server_plugin samples tree
(https://github.com/dart-lang/analysis_server_plugin, samples/pyramid_lint,
snapshot commit b11ddc851e92546adb4a4929a557cf537bf116da, 2026-10-13).

Pruned to `pubspec.yaml`, `lib/`, and `LICENSE` so the fixture is
self-contained on a fresh clone; the sample's `pubspec.yaml` path-depends on
this directory. Everything else (tests, example, docs, screenshots) was
dropped, and the `screenshots:` section was removed from `pubspec.yaml`
because the image files are not vendored. All Dart sources are
byte-identical to upstream.
