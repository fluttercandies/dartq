# Provenance — url_launcher_android

- **Upstream package:** `url_launcher_android`
- **Exact version:** 6.3.33 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/url_launcher_android-6.3.33`)
- **pub.dev:** https://pub.dev/packages/url_launcher_android/versions/6.3.33
- **License:** BSD 3-Clause ("Copyright 2013 The Flutter Authors"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (2 dart files, ~518 LOC) and `pubspec.yaml`
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
- **Why selected:** flutter-plugin face: canonical small MethodChannel plugin (channel name constants, codec wiring); same sdk: flutter offline gap as webview_flutter_android
