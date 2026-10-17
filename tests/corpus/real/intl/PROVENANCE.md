# Provenance — intl

- **Upstream package:** `intl`
- **Exact version:** 0.20.3 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/intl-0.20.3`)
- **pub.dev:** https://pub.dev/packages/intl/versions/0.20.3
- **License:** BSD 3-Clause ("Copyright 2013, the Dart project authors"); the upstream LICENSE and PATENTS files are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (47 dart files, ~37768 LOC plus 243 non-dart locale-data files (.json/.txt) kept byte-faithful) and `pubspec.yaml`
  (byte-identical to upstream).
- **Modified:** nothing.
- **Not copied:** `test/` (imports package:test — not deps-clean),
  `CHANGELOG.md`, `README.md`, analysis options, `.dart_tool/`.
- **Resolution:** no committed `.dart_tool/` — the sweep
  (`scripts/real-corpus-sweep.sh`) materializes it at run time
  (`dart pub get --offline` when the local cache resolves the dependency
  set, else the hand-written self-referential config documented in the
  corpus README). Same config feeds both faces, so the override is
  comparison-fair.
- **Why selected:** top ecosystem i18n package; large single-package face (~38k LOC of dense table-driven code)
