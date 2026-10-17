// Trigger for clean_architecture_linter `widget_ref_read_then_when` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/widget_ref_read_then_when_v2_rule_test.dart):
// a widget reading a provider then calling `.when` on the result.
class TodoPage {
  void build(ref) {
    ref.read(todoProvider).when(
        data: (_) {}, loading: () {}, error: (_, __) {});
  }
}

final todoProvider = Object();
