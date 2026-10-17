# Provenance — html

- **Upstream package:** `html`
- **Exact version:** 0.15.7 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/html-0.15.7`)
- **pub.dev:** https://pub.dev/packages/html/versions/0.15.7
- **License:** MIT ("Copyright (c) 2006-2012 The Authors"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (15 dart files, ~26639 LOC) and `pubspec.yaml`
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
- **Why selected:** top HTML parser; token-tree face (~27k LOC) with a small file count and long files
