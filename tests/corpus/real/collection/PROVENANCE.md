# Provenance — collection

- **Upstream package:** `collection`
- **Exact version:** 1.19.1 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/collection-1.19.1`)
- **pub.dev:** https://pub.dev/packages/collection/versions/1.19.1
- **License:** BSD 3-Clause ("Copyright 2015, the Dart project authors"),
  vendored verbatim in effect via the upstream headers; the LICENSE file
  itself was not copied (only `lib/` and `pubspec.yaml` were, per corpus
  policy — every copied `.dart` file keeps its upstream header untouched).
- **Upstream commit:** not recorded in the cache (hosted archives carry no
  VCS metadata).
- **Vendored:** 2026-10-11.
- **Copied:** `lib/` (29 files, ~6,050 LOC) and `pubspec.yaml`.
- **Modified:** nothing. `pubspec.yaml` is byte-identical to upstream
  (it declares no runtime dependencies and no cache-specific fields).
- **Not copied:** `test/` (imports `package:test`, which is not vendored —
  not deps-clean), `CHANGELOG.md`, `README.md`, analysis options.
- **Hand-written resolution:** `.dart_tool/package_config.json` points the
  package at itself (`rootUri: ".."`) with `languageVersion: "3.13"` —
  overridden from the upstream `environment: sdk: ^3.4.0` lower bound to
  satisfy the harness preflight (>= 3.13). Upstream files are untouched;
  both oracle and dartq analyze against the same config, so the override
  is comparison-fair.
