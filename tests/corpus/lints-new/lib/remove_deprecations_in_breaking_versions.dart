// remove_deprecations_in_breaking_versions — differential corpus
// (ticket #1160), NO-VERSION control face.
// Source: upstream Dart SDK source,
// pkg/linter/test/rules/remove_deprecations_in_breaking_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// The rule's registration is gated on the containing package's pubspec
// `version:` (upstream rule source :36-48 — a missing or unparseable
// version never registers). This corpus package deliberately declares NO
// version, so this fixture is the registration-bail control: silent on the
// oracle leg by gate, silent on the dartq leg structurally (pinned in
// STRUCTURALLY_SILENT).
// The breaking face (fires) and non-breaking face (silent under a real
// version) live in the twin packages under packages/version_breaking and
// packages/version_minor.
// Inventory (this file): 1 NEGATIVE control (0 rows on both legs). The
// upstream testVersion matrix's remaining version points (1.0.0, 0.1.0,
// 2.0.0-dev, 0.0.0, 2.0.1, 0.0.1, 0.1.1, 2.0.0+mybuild) are pure
// is_breaking_version gate points covered by the dartq unit pins, not
// re-mountable as ten twin packages — see README.

// NEGATIVE upstream testVersion control (no pubspec version): registration
// bails, no rows either leg.
@deprecated foo() {}
