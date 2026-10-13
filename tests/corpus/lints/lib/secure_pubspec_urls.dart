// secure_pubspec_urls — official rule faces (upstream Dart SDK source,
// pkg/linter/lib/src/rules/pub/secure_pubspec_urls.dart): a pubspec-face
// rule that never fires on Dart files. The verdict runs per pubspec URL
// value (homepage/repository/documentation/issue_tracker and the
// git/hosted URLs of every dependencies/dev_dependencies/
// dependency_overrides entry): a value whose URI scheme is `http` or
// `git` reports at the value with the scheme as the message argument, so
// its rows are keyed pubspec.yaml.
//
// The deliberate corpus face is this package's own pubspec.yaml: its
// `homepage: http://lint-corpus.invalid` is the one SECURE_PUBSPEC_URLS
// row keyed pubspec.yaml on both sides. No other URL field exists in
// either pubspec, pinning the field-walk breadth and the https exemption
// edge (the environment/sdk and path entries never enter the rule).
//
// Expected rows: 0 in this file — SECURE_PUBSPEC_URLS is keyed
// pubspec.yaml.
