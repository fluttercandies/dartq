// Trigger for leancode_lint
// `constructor_parameters_and_fields_should_have_the_same_order`.
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/constructor_parameters_and_fields_should_have_the_same_order_test.dart):
// a constructor whose `this.` parameter order differs from the field
// declaration order.
class Reordered {
  const Reordered(
    this.third,
    this.second,
    this.first,
  );

  final int first;
  final int second;
  final int third;
}
