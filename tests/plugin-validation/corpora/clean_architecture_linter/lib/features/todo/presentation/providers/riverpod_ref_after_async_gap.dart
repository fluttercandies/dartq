// Trigger for clean_architecture_linter `riverpod_ref_after_async_gap`
// (v2). Shape from the upstream suite
// (vendor/clean_architecture_linter-2.8.0/test/src/rules/presentation_rules/riverpod_ref_after_async_gap_v2_rule_test.dart):
// a notifier reading `ref` after an `await`.
const riverpod = _Riverpod();

class _Riverpod {
  const _Riverpod();
}

@riverpod
class TodoNotifier {
  Future<void> createTodo() async {
    await saveTodo();
    ref.read(todoProvider);
  }

  Ref get ref => Ref();
}

class Ref {
  T read<T>(T provider) => provider;
}

final todoProvider = Object();

Future<void> saveTodo() async {}
