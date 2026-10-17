// Trigger for clean_architecture_linter `presentation_no_throw` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/presentation_no_throw_rule_test.dart):
// a presentation notifier whose `build` throws.
class TodoException implements Exception {}

class BadNotifier {
  Future<void> build() async {
    throw TodoException();
  }
}
