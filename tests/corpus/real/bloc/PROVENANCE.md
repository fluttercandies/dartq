# Provenance — bloc

- **Upstream package:** `bloc`
- **Exact version:** 9.2.1 (the resolved copy in the local pub cache,
  `~/.pub-cache/hosted/pub.dev/bloc-9.2.1`)
- **pub.dev:** https://pub.dev/packages/bloc/versions/9.2.1
- **License:** MIT ("Copyright (c) 2026 Felix Angelov"); the upstream LICENSE file(s) are vendored next to this
  file.
- **Vendored:** 2026-10-14 (real-corpus expansion, ticket #1136).
- **Copied:** `lib/` (8 dart files, ~1002 LOC) and `pubspec.yaml`
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
- **Why selected:** top ecosystem state-management package; stream/event-transformer face whose only dep (meta) is a vendored corpus package
