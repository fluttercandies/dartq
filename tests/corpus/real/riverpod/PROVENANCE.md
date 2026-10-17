# Provenance — riverpod

- **Upstream package:** `riverpod`
- **Exact version:** 3.4.3 (the newest copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/riverpod-3.4.3`)
- **pub.dev:** https://pub.dev/packages/riverpod/versions/3.4.3
- **License:** MIT ("Copyright (c) 2020 Remi Rousselet"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-17 (real-corpus expansion, ticket #2064).
- **Copied:** `lib/` (57 dart files, ~14255 LOC) and `pubspec.yaml`
  (byte-identical to upstream).
- **Modified:** nothing.
- **Not copied:** `test/` (imports package:test — not deps-clean),
  `example/`, `extension/` (DevTools extension carrying a prebuilt
  `build/` asset tree), `doc/`, `dartdoc/`, `CHANGELOG.md`, `README.md`,
  analysis options, `.dart_tool/`.
- **Resolution:** no committed `.dart_tool/` — the sweep
  (`scripts/real-corpus-sweep.sh`) materializes it at run time
  (`dart pub get --offline` when the local cache resolves the dependency
  set, else the hand-written self-referential config documented in the
  corpus README). Same config feeds both faces, so the override is
  comparison-fair.
- **Why selected:** top ecosystem reactive state-management framework core
  (the framework package itself, not the flutter/hooks wrappers);
  generics-heavy provider-graph and async face; two of its nine runtime
  deps (`collection`, `meta`) are vendored corpus packages
