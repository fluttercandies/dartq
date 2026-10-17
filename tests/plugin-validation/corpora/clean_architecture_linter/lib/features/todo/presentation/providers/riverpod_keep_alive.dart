// Trigger for clean_architecture_linter `riverpod_keep_alive` (v2).
// Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_keep_alive_v2_rule_test.dart):
// a notifier provider marked `keepAlive: true`.
class Riverpod {
  const Riverpod({required bool keepAlive});
}

@Riverpod(keepAlive: true)
class TodoListNotifier {}
