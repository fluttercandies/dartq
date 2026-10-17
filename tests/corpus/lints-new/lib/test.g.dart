// unnecessary_ignore — differential corpus (ticket #1160), GENERATED-file
// face. Source: upstream Dart SDK source,
// pkg/linter/test/rules/unnecessary_ignore_test.dart,
// UnnecessaryIgnoreGeneratedFileTest (testFileName test.g.dart).
// The .g.dart suffix mounts the analyzer's generated-file lint suppression:
// upstream expects both cases silent here. Whatever each leg does with
// generated files, both must agree — this file is that parity probe.
// Inventory (this file): 2 NEGATIVE.
// Expected rows: 0 on a face where generated-file suppression applies; a
// dartq-only UNNECESSARY_IGNORE row here is the known under-report
// approximation family (see README and the N5 adjudication table).

// NEGATIVE upstream UnnecessaryIgnoreGeneratedFileTest.test_file.
// ignore_for_file: unused_local_variable
void fGeneratedFile() {}

// NEGATIVE upstream UnnecessaryIgnoreGeneratedFileTest.test_line.
// ignore: unused_local_variable
void fGeneratedLine() {}
