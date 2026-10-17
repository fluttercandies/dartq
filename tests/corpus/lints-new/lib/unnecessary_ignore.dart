// unnecessary_ignore — differential corpus (ticket #1160), FILE face.
// Source: upstream Dart SDK source, pkg/linter/test/rules/unnecessary_ignore_test.dart
// (transcribed 2026-10-14, vendored SDK 3.15 main face).
// This file holds the ignore_for_file cases only: an ignore_for_file
// comment governs the whole file, so the upstream file-face and line-face
// tests cannot share a corpus file (they would mute each other). The line
// face lives in unnecessary_ignore_line.dart; the generated-file face in
// test.g.dart.
// Inventory (this file): 1 POSITIVE + 3 NEGATIVE. Skipped corpus-wide:
// the two UnnecessaryIgnoreDisabledTest cases (they demand the rule be
// UNMOUNTED, which an enabling options file cannot express).
// Expected rows (oracle leg): 1 — UNNECESSARY_IGNORE keyed
// unnecessary_ignore.dart at the ignore_for_file comment. The dartq leg is
// structurally silent (IgnoreValidator post-pass not mounted; pinned in
// STRUCTURALLY_SILENT) — see the N5 adjudication families.

// POSITIVE upstream test_file: unused_local_variable is not produced here,
// so the file ignore is unnecessary.
// ignore_for_file: unused_local_variable
void fFile() {}

// NEGATIVE upstream test_file_necessaryIgnore_sharedName: the ignore names
// the shared spelling invalid_null_aware_operator; the diagnostic IS
// produced (shared-name face).
// ignore_for_file: invalid_null_aware_operator
void fNecessaryShared(int? x) => x?.abs()?.isEven;

// NEGATIVE upstream test_file_necessaryIgnore_uniqueName: same code via its
// unique name invalid_null_aware_operator_after_short_circuit.
// ignore_for_file: invalid_null_aware_operator_after_short_circuit
void fNecessaryUnique(int? x) => x?.abs()?.isEven;

// NEGATIVE upstream test_file_unrecognizedDiagnostic: an unknown code name
// stays silent (the rule cannot prove it unnecessary).
// ignore_for_file: undefined_diagnostic_code
void fUnrecognized() {}
