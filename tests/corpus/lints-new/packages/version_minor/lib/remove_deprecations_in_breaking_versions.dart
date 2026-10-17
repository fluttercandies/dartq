// remove_deprecations_in_breaking_versions — NON-BREAKING-version twin face
// (pkg/linter remove_deprecations_in_breaking_test.dart, testVersion matrix
// with isProblem: false, mounted at version 2.1.0 = upstream
// test_MinorIncrement). Expected rows: 0 on both legs — a non-breaking
// release never registers the rule, so the deprecations stay silent.
@deprecated foo() {}

@Deprecated('Please stop using this before it explodes!') bar() {}
