// remove_deprecations_in_breaking_versions — BREAKING-version twin face
// (pkg/linter remove_deprecations_in_breaking_test.dart, testVersion matrix
// with isProblem: true, mounted at version 2.0.0 = upstream
// test_breakingMajor). The rule reports at each @deprecated/@Deprecated
// annotation name when the containing package's version is a breaking
// release. Expected rows (oracle leg; the rule is structurally silent on
// the dartq leg pending the pubspec-family mount — see README): 2 —
// REMOVE_DEPRECATIONS_IN_BREAKING_VERSIONS keyed
// remove_deprecations_in_breaking_versions.dart, one per annotation name.
@deprecated foo() {}

@Deprecated('Please stop using this before it explodes!') bar() {}
