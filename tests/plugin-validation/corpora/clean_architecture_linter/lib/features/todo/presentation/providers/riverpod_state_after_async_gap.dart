// Trigger for clean_architecture_linter
// `riverpod_state_after_async_gap` (opt-in lint). Shape from the upstream
// suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_state_after_async_gap_v2_rule_test.dart):
// a notifier assigning `state` after an `await`.
abstract class _$TodoNotifier {
  Object? state;
}

class TodoNotifier extends _$TodoNotifier {
  Future<void> load() async {
    await fetchTodo();
    state = AsyncData(todo);
  }

  Object get todo => Object();
}

class AsyncData {
  AsyncData(Object value);
}

Future<void> fetchTodo() async {}
