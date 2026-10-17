# Provenance — http

- **Upstream package:** `http`
- **Exact version:** 1.6.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/http-1.6.0`)
- **pub.dev:** https://pub.dev/packages/http/versions/1.6.0
- **License:** BSD 3-Clause ("Copyright 2014, the Dart project authors"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (27 dart files, ~2758 LOC) and `pubspec.yaml`
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
- **Why selected:** the top ecosystem HTTP package; conditional-import (platform-split io/web) resolution face
