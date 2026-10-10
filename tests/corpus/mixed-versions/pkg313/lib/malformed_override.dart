// Malformed and inconsistent language-version comments: this file is the
// only face producing the language-version-family codes
// (invalid_language_version_override.*, illegal_language_version_override.*,
// inconsistent_language_version_override.*).
//
// Anchors: scanner recognition of the `@dart` prefix
// (.third/dart-sdk/pkg/_fe_analyzer_shared/lib/src/scanner/abstract_scanner.dart
// :1837) and the first-registered-version rule
// (pkg/front_end/lib/src/source/source_compilation_unit.dart:469-478).
//
// DOCUMENTED EXCLUSION: these family codes sit OUTSIDE the 255-code syntax
// gate universe by design (the gate's codes file excludes them); the
// differential filter accounting handles out-of-set codes. Everything
// else in this corpus stays inside the set.
//
// Expectation: the engine must flag the malformed, out-of-range, and
// inconsistent overrides here — the only intentional diagnostics in the
// whole corpus tree.

// @dart=x.y
// A malformed version comment (non-numeric) — invalid override family.

// @dart=99.0
// An out-of-range version comment — illegal override family.

// @dart=2.9

// @dart=3.0
// Two different well-formed overrides in one file — inconsistent family
// (only the first registered one applies, per the anchor above).

int usable = 1;

void main() {
  print(usable);
}
