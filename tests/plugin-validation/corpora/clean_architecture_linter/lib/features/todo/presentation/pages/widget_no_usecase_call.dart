// Trigger for clean_architecture_linter `widget_no_usecase_call` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/widget_no_usecase_call_v2_rule_test.dart):
// a widget calling a usecase via `ref.read` instead of the proper path.
class TodoPage {
  void build(ref) {
    ref.read(getTodosUseCaseProvider);
  }
}

final getTodosUseCaseProvider = Object();
