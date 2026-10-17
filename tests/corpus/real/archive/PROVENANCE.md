# Provenance — archive

- **Upstream package:** `archive`
- **Exact version:** 4.3.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/archive-4.3.0`)
- **pub.dev:** https://pub.dev/packages/archive/versions/4.3.0
- **License:** MIT, dual-licensed (LICENSE + LICENSE-other.md vendored); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (97 dart files, ~22750 LOC) and `pubspec.yaml`
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
- **Why selected:** top compression package; io/codec-heavy face (~23k LOC, binary-format code)
