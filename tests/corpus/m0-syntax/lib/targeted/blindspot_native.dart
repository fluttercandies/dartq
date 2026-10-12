// Targets the native family. Probe-locked against the pinned oracle (dart
// analyze 3.13.1, machine + default format, 2026-10-11): both faces fire on
// every non-SDK library — the resolver-tier `!_isInSystemLibrary` gate
// (analyzer-8.4.1 error_verifier.dart:1324-1329 / :4516-4523) always fails
// open in corpus code. The deprecated-form row
// native_clause_should_be_annotation (TSV:243) never surfaces at the pinned
// oracle (probed silent on both formats; the fasta-tier report is dropped
// before analyzer output), so this fixture pins exactly two (code, file)
// entries.

// native_clause_in_non_sdk_code (TSV:242) — clause span runs from the
// `native` keyword through the closing quote (`native "corpus"`, len 15;
// a bare `native` keyword pins len 6).
class NativeClass native "corpus" {}

// native_function_body_in_non_sdk_code (TSV:244) — body span runs from the
// `native` keyword through the semicolon (`native "corpus";`, len 16);
// without the semicolon no NativeFunctionBody node is built and the oracle
// degrades to missing_function_body.
void nativeFunction() native "corpus";
