# Provenance — meta

- **Upstream package:** `meta`
- **Exact version:** 1.19.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/meta-1.19.0`)
- **pub.dev:** https://pub.dev/packages/meta/versions/1.19.0
- **License:** BSD 3-Clause ("Copyright 2016, the Dart project authors");
  the LICENSE file itself was not copied (only `lib/` and `pubspec.yaml`
  were, per corpus policy — every copied `.dart` file keeps its upstream
  header untouched).
- **Upstream commit:** not recorded in the cache (hosted archives carry no
  VCS metadata; the package ships from dart-lang/sdk, where the cache
  records no hash).
- **Vendored:** 2026-10-11.
- **Copied:** `lib/` (3 files, ~1,216 LOC) and `pubspec.yaml`.
- **Modified:** `pubspec.yaml` only — removed the `resolution: workspace`
  line (a dart-lang/mono-repo workspace marker that pub copied into the
  cache archive; it is meaningless and resolution-hostile for a standalone
  vendored package). No `.dart` file was touched.
- **Not copied:** `test/` (imports `package:test` / `package:lints` —
  not deps-clean), `CHANGELOG.md`, `README.md`.
- **Hand-written resolution:** `.dart_tool/package_config.json` points the
  package at itself (`rootUri: ".."`) with `languageVersion: "3.13"` —
  overridden from the upstream `environment: sdk: ^3.5.0` lower bound to
  satisfy the harness preflight (>= 3.13). Upstream files are untouched;
  both oracle and dartq analyze against the same config, so the override
  is comparison-fair.
