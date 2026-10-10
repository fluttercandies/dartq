# Provenance — args

- **Upstream package:** `args`
- **Exact version:** 2.7.0 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/args-2.7.0`)
- **pub.dev:** https://pub.dev/packages/args/versions/2.7.0
- **License:** BSD 3-Clause ("Copyright 2013, the Dart project authors");
  the LICENSE file itself was not copied (only `lib/` and `pubspec.yaml`
  were, per corpus policy — every copied `.dart` file keeps its upstream
  header untouched).
- **Upstream commit:** not recorded in the cache (hosted archives carry no
  VCS metadata).
- **Vendored:** 2026-10-11.
- **Copied:** `lib/` (12 files, ~2,295 LOC) and `pubspec.yaml`.
- **Modified:** nothing. `pubspec.yaml` is byte-identical to upstream
  (zero runtime dependencies, no cache-specific fields).
- **Not copied:** `test/` (imports `package:test`, which is not vendored —
  not deps-clean), `CHANGELOG.md`, `README.md`, analysis options.
- **Hand-written resolution:** `.dart_tool/package_config.json` points the
  package at itself (`rootUri: ".."`) with `languageVersion: "3.13"` —
  overridden from the upstream `environment: sdk: ^3.3.0` lower bound to
  satisfy the harness preflight (>= 3.13). Upstream files are untouched;
  both oracle and dartq analyze against the same config, so the override
  is comparison-fair.
