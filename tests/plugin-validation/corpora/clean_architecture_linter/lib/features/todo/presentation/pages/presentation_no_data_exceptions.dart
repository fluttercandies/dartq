// Trigger for clean_architecture_linter `presentation_no_data_exceptions`
// (v2). Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/presentation_no_data_exceptions_v2_rule_test.dart):
// presentation code checking against data-layer exception types.
class NotFoundException implements Exception {}

void render(Object error) {
  if (error is NotFoundException) {
    showError();
  }
}

void showError() {}
