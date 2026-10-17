# Provenance — dio

- **Upstream package:** `dio`
- **Exact version:** 5.11.1 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/dio-5.11.1`)
- **pub.dev:** https://pub.dev/packages/dio/versions/5.11.1
- **License:** MIT; the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (37 dart files, ~5986 LOC plus lib/fix_data/fix.yaml kept byte-faithful) and `pubspec.yaml`
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
- **Why selected:** top ecosystem HTTP client; async/transformer-heavy face with 7 runtime deps
