# Provenance — webview_flutter_android

- **Upstream package:** `webview_flutter_android`
- **Exact version:** 4.13.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/webview_flutter_android-4.13.0`)
- **pub.dev:** https://pub.dev/packages/webview_flutter_android/versions/4.13.0
- **License:** BSD 3-Clause ("Copyright 2013 The Flutter Authors"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (14 dart files, ~10232 LOC) and `pubspec.yaml`
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
- **Why selected:** flutter-plugin face: pigeon-generated platform-channel API surface (~10k LOC of generated channel code); the pubspec sdk: flutter dependency is unresolvable offline by design — the fallback face documents the gap
