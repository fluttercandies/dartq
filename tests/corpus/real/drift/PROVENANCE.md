# Provenance — drift

- **Upstream package:** `drift`
- **Exact version:** 2.34.0 (the newest copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/drift-2.34.0`)
- **pub.dev:** https://pub.dev/packages/drift/versions/2.34.0
- **License:** MIT ("Copyright (c) 2021 Simon Binder"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-17 (real-corpus expansion, ticket #2064).
- **Copied:** `lib/` (211 dart files, ~41839 LOC plus 1 non-dart lib asset
  kept byte-faithful (drift3_preview/README.md)) and `pubspec.yaml`
  (byte-identical to upstream).
- **Modified:** nothing.
- **Not copied:** `test/` (imports package:test — not deps-clean),
  `example/`, `extension/` (DevTools extension carrying a prebuilt
  `build/` asset tree), `tool/`, `web/`, `CHANGELOG.md`, `README.md`,
  analysis options, `.dart_tool/`.
- **Resolution:** no committed `.dart_tool/` — the sweep
  (`scripts/real-corpus-sweep.sh`) materializes it at run time
  (`dart pub get --offline` when the local cache resolves the dependency
  set, else the hand-written self-referential config documented in the
  corpus README). Same config feeds both faces, so the override is
  comparison-fair.
- **Why selected:** top ecosystem reactive SQLite ORM; second-largest dart
  face in the corpus after `analyzer`; type-mapper/stream-query face;
  three of its nine runtime deps (`path`, `collection`, `meta`) are
  vendored corpus packages
