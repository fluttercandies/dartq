// Trigger for leancode_lint `use_design_system_item_<item>` (config-driven
// code family; `use_design_system_item_LftText` in the upstream suite).
// Shape from the upstream suite
// (vendor/leancode_lint-27.0.0/test/test_cases/use_design_system_item_test.dart):
// usage of a type banned in favor of a design-system replacement.
void test() {
  Text('abc');
}

class Text {
  const Text(String data);
}
