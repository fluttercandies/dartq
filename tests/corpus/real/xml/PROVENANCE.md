# Provenance — xml

- **Upstream package:** `xml`
- **Exact version:** 7.1.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/xml-7.1.0`)
- **pub.dev:** https://pub.dev/packages/xml/versions/7.1.0
- **License:** MIT ("The MIT License"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (172 dart files, ~29681 LOC) and `pubspec.yaml`
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
- **Why selected:** top data-format package; parser-heavy face (~30k LOC, 172 files). One upstream contributor-guidance markdown file nested under lib/src/xpath/ is excluded from the copy (not analysis content)
