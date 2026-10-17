# Provenance — analyzer

- **Upstream package:** `analyzer`
- **Exact version:** 14.5.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/analyzer-14.5.0`)
- **pub.dev:** https://pub.dev/packages/analyzer/versions/14.5.0
- **License:** BSD 3-Clause ("Copyright 2013, the Dart project authors"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (447 dart files, ~317851 LOC plus 3 non-dart lib assets kept byte-faithful (fix_data.yaml, format.fbs, a README)) and `pubspec.yaml`
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
- **Why selected:** the deep-transitive-dependency-tree face (12 runtime deps, all in the pub cache) and the large-project perf face (~318k LOC, 447 files); also the oracle own codebase as corpus
