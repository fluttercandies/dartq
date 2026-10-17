// Trigger for solid_lints `avoid_non_null_assertion`.
// Shape from the vendored suite
// (vendor/solid_lints-2.0.0/test/src/lints/avoid_non_null_assertion/avoid_non_null_assertion_rule_test.dart,
// `test_reports_non_null_assertion_on_nullable_value`).
void m(int? number) {
  final value = number!;
  print(value);

  final twice = number!.abs();
  print(twice);
}
