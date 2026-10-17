// unnecessary_ignore — differential corpus (ticket #1160), LINE face.
// Source: upstream Dart SDK source, pkg/linter/test/rules/unnecessary_ignore_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// Companion of unnecessary_ignore.dart (file face) — split so the
// ignore_for_file comment there cannot mute these line ignores.
// Inventory (this file): 1 POSITIVE + 3 NEGATIVE.
// Expected rows (oracle leg): 1 — UNNECESSARY_IGNORE keyed
// unnecessary_ignore_line.dart at the `// ignore:` comment; dartq leg
// structurally silent (see the file-face companion's header).

// POSITIVE upstream test_line: unused_local_variable is not produced at
// this location, so the line ignore is unnecessary.
// ignore: unused_local_variable
void fLine() {}

// NEGATIVE upstream test_line_necessaryIgnore_sharedName.
// ignore: invalid_null_aware_operator
void fLineNecessaryShared(int? x) => x?.abs()?.isEven;

// NEGATIVE upstream test_line_necessaryIgnore_uniqueName.
// ignore: invalid_null_aware_operator_after_short_circuit
void fLineNecessaryUnique(int? x) => x?.abs()?.isEven;

// NEGATIVE upstream test_line_unrecognizedDiagnostic.
// ignore: undefined_diagnostic_code
void fLineUnrecognized() {}
