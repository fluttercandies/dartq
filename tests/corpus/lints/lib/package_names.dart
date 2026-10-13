// package_names — official rule faces (upstream Dart SDK source,
// pkg/linter/lib/src/rules/pub/package_names.dart with isValidPackageName
// in pkg/linter/lib/src/utils.dart): a pubspec-face rule that never fires
// on Dart files. The verdict runs on the pubspec `name:` entry only (the
// official visitPackageName reports at the value node when the name fails
// lower_case_with_underscores, is not a legal identifier, or is a
// reserved word), so its row is keyed pubspec.yaml.
//
// The deliberate corpus face is the nested sub-package:
// tests/corpus/lints/nested/pubspec.yaml declares the mixed-case name
// `Lint_corpus_nested` — one PACKAGE_NAMES row keyed nested/pubspec.yaml
// on both sides. This package's own name `lint_corpus` conforms and stays
// silent, pinning the exemption edge of the same face.
//
// Expected rows: 0 in this file — PACKAGE_NAMES is keyed
// nested/pubspec.yaml.
